use super::*;

thread_local! {
    static METADATA_READS: std::cell::Cell<(u64, u64, u64)> = const {
        std::cell::Cell::new((0, 0, 0))
    };
    static FENCE_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static FENCE_WRITES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static RESERVATION_PROBES: std::cell::Cell<(u64, u64, u64)> = const {
        std::cell::Cell::new((0, 0, 0))
    };
    static REGISTRATION_READS: std::cell::Cell<(u64, u64, u64, u64)> = const {
        std::cell::Cell::new((0, 0, 0, 0))
    };
}

fn observe_registration_reads(event: rusqlite::trace::TraceEvent<'_>) {
    let rusqlite::trace::TraceEvent::Stmt(_statement, sql) = event else {
        return;
    };
    if sql.starts_with("SELECT")
        && sql.contains("FROM native_counter_v1 WHERE name='attempt_fence'")
    {
        FENCE_READS.with(|counter| counter.set(counter.get().saturating_add(1)));
    }
    if sql.starts_with("UPDATE native_counter_v1 SET next=?1,row_digest=?2") {
        FENCE_WRITES.with(|counter| counter.set(counter.get().saturating_add(1)));
    }
    METADATA_READS.with(|counter| {
        let mut counts = counter.get();
        if sql.starts_with("SELECT") && sql.contains("THEN payload END") {
            if sql.contains("FROM native_object_v1 WHERE object=?1") {
                counts.0 = counts.0.saturating_add(1);
            } else if sql.contains("FROM native_object_install_v1 WHERE object=?1") {
                counts.1 = counts.1.saturating_add(1);
            }
        } else if sql.starts_with("SELECT payload,row_digest FROM native_object_") {
            counts.2 = counts.2.saturating_add(1);
        }
        counter.set(counts);
    });
    RESERVATION_PROBES.with(|counter| {
        let mut reads = counter.get();
        if sql.starts_with("SELECT")
            && sql.contains("FROM native_object_install_v1 WHERE object=?1")
            && sql.contains("THEN payload END")
        {
            reads.0 = reads.0.saturating_add(1);
        } else if sql.starts_with("SELECT")
            && (sql.contains("FROM native_object_intent_v1")
                || sql.contains("FROM native_ingress_intent_v1"))
        {
            reads.1 = reads.1.saturating_add(1);
        } else if sql == object_install::RESERVE_NEW_SQL {
            reads.2 = reads.2.saturating_add(1);
        }
        counter.set(reads);
    });
    REGISTRATION_READS.with(|counter| {
        let mut reads = counter.get();
        if sql == object_install::REGISTERED_METADATA_SQL {
            reads.0 = reads.0.saturating_add(1);
        } else if sql == "SELECT EXISTS(SELECT 1 FROM native_object_install_v1 WHERE object=?1)" {
            reads.1 = reads.1.saturating_add(1);
        } else if sql.starts_with("SELECT") && sql.contains("FROM native_object_edge_v1") {
            reads.2 = reads.2.saturating_add(1);
        } else if sql.starts_with("SELECT")
            && sql.contains("FROM native_gc_object_v1 WHERE object=?1")
        {
            reads.3 = reads.3.saturating_add(1);
        }
        counter.set(reads);
    });
}

fn begin_registration_reads(catalog: &SqliteCatalog) -> TestResult {
    METADATA_READS.with(|counter| counter.set((0, 0, 0)));
    REGISTRATION_READS.with(|counter| counter.set((0, 0, 0, 0)));
    RESERVATION_PROBES.with(|counter| counter.set((0, 0, 0)));
    FENCE_READS.with(|counter| counter.set(0));
    FENCE_WRITES.with(|counter| counter.set(0));
    let guard = catalog.lock().map_err(|error| error.to_string())?;
    guard.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(observe_registration_reads),
    );
    drop(guard);
    Ok(())
}

fn end_registration_reads(
    catalog: &SqliteCatalog,
) -> Result<(u64, u64, u64, u64), Box<dyn std::error::Error>> {
    let guard = catalog.lock().map_err(|error| error.to_string())?;
    guard.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
    drop(guard);
    Ok(REGISTRATION_READS.with(std::cell::Cell::get))
}

pub(super) fn reader_length_limit(
    catalog: &SqliteCatalog,
    bytes: i32,
) -> Result<i32, Box<dyn std::error::Error>> {
    let guard = catalog.lock().map_err(|error| error.to_string())?;
    let prior = guard.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, bytes)?;
    drop(guard);
    Ok(prior)
}

fn managed_chain(count: u16) -> Vec<RegisteredObject> {
    let mut objects = Vec::with_capacity(usize::from(count));
    for ordinal in 0..count {
        let mut digest = [0xa5; 32];
        for (slot, byte) in digest.iter_mut().zip(ordinal.to_le_bytes()) {
            *slot = byte;
        }
        let children = objects
            .last()
            .map(|prior: &RegisteredObject| vec![prior.reference])
            .unwrap_or_default();
        objects.push(RegisteredObject::unscoped(
            ObjectRef(Digest(digest)),
            1,
            children,
        ));
    }
    objects
}

fn managed_install_counts(catalog: &SqliteCatalog) -> Result<[i64; 6], Box<dyn std::error::Error>> {
    let connection = Connection::open(catalog.path())?;
    Ok(connection.query_row(
        "SELECT (SELECT COUNT(*) FROM native_object_v1),
                (SELECT COUNT(*) FROM native_object_edge_v1),
                (SELECT COUNT(*) FROM native_object_install_v1),
                (SELECT COUNT(*) FROM native_object_install_edge_v1),
                (SELECT COUNT(*) FROM native_ingress_object_v1),
                (SELECT COUNT(*) FROM native_gc_object_v1)",
        [],
        |row| {
            Ok([
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ])
        },
    )?)
}

#[test]
fn managed_256_chain_confirms_across_old_64_boundary_and_257_refuses_before_mutation() -> TestResult
{
    use quanta_index_core::index::ObjectInstallScope;
    use quanta_index_core::index::ports::ManagedObjectInstallPort;

    let (root, catalog, ingress) = managed_adoption_fixture()?;
    let chain = managed_chain(256);
    check_eq!(chain.len(), 256);
    check_eq!(managed_install_counts(&catalog)?, [1, 0, 0, 0, 0, 1]);

    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        &chain,
        &mut admission_budget(),
    )?;
    check_eq!(managed_install_counts(&catalog)?, [1, 0, 256, 255, 0, 257]);
    begin_registration_reads(&catalog)?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        &chain,
        &mut admission_budget(),
    )?;
    // Observe executed SQLite statements, including cached-statement reuse.
    // Each unique object checks its permanent slot once; same-transaction
    // registration does not re-read it or probe provisional existence again.
    // Each child adoption and combined confirmation/queue transition
    // authenticates one bounded fixed-field row: 255 + 256.
    check_eq!(end_registration_reads(&catalog)?, (256, 0, 0, 511));
    check_eq!(METADATA_READS.with(std::cell::Cell::get), (256, 256, 0));
    begin_registration_reads(&catalog)?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        &chain,
        &mut admission_budget(),
    )?;
    // The guarded edge scan authenticates both shape and digest once per
    // replayed object, then consumes that exact transaction-local result.
    check_eq!(end_registration_reads(&catalog)?, (256, 0, 256, 511));
    check_eq!(METADATA_READS.with(std::cell::Cell::get), (256, 256, 0));
    let duplicate = chain.first().ok_or("managed duplicate missing")?;
    check!(matches!(
        catalog.confirm_managed(
            ObjectInstallScope::Ingress(ingress),
            &[duplicate.clone(), duplicate.clone()],
            &mut admission_budget(),
        ),
        Err(IndexError::InvalidInput(_))
    ));
    // A transaction-local proof must never become a cache for the next call.
    // Corrupt a permanent edge after successful replay; the same exact batch
    // must refuse it, preserving both the corruption and existing custody.
    let parent = chain.last().ok_or("managed replay parent missing")?;
    let child = parent
        .children
        .first()
        .ok_or("managed replay child missing")?;
    let connection = Connection::open(catalog.path())?;
    let original_digest: Vec<u8> = connection.query_row(
        "SELECT row_digest FROM native_object_edge_v1 WHERE parent=?1 AND child=?2",
        params![object_key(parent.reference), object_key(*child)],
        |row| row.get(0),
    )?;
    let prior_limit = reader_length_limit(&catalog, 2048)?;
    // The healthy control fits the same reader limit. An unguarded SELECT of
    // the 4096-byte corrupt digest would fail SQLITE_TOOBIG, not the expected
    // semantic edge-shape refusal, before Rust can perform its own check.
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        &chain,
        &mut admission_budget(),
    )?;
    for width in [32_i64, 4096] {
        check_eq!(
            connection.execute(
                "UPDATE native_object_edge_v1 SET row_digest=zeroblob(?1) WHERE parent=?2 AND child=?3",
                params![width, object_key(parent.reference), object_key(*child)],
            )?,
            1
        );
        let before = managed_install_counts(&catalog)?;
        check!(matches!(
            catalog.confirm_managed(
                ObjectInstallScope::Ingress(ingress),
                &chain,
                &mut admission_budget(),
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(managed_install_counts(&catalog)?, before);
        let corrupt_digest: Vec<u8> = connection.query_row(
            "SELECT row_digest FROM native_object_edge_v1 WHERE parent=?1 AND child=?2",
            params![object_key(parent.reference), object_key(*child)],
            |row| row.get(0),
        )?;
        check_eq!(corrupt_digest, vec![0; usize::try_from(width)?]);
    }
    check_eq!(reader_length_limit(&catalog, prior_limit)?, 2048);
    check_eq!(
        connection.execute(
            "UPDATE native_object_edge_v1 SET row_digest=?1 WHERE parent=?2 AND child=?3",
            params![
                original_digest,
                object_key(parent.reference),
                object_key(*child)
            ],
        )?,
        1
    );
    drop(connection);
    drop(catalog);

    let catalog = open(root.path())?;
    check_eq!(
        managed_install_counts(&catalog)?,
        [257, 255, 0, 0, 256, 257]
    );
    {
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        for object in &chain {
            check!(object_install::confirmed_matches(&guard, object)?);
        }
    }
    // The ordinal chain crosses every former 64-object physical boundary.
    for ordinal in [64_usize, 128, 192, 255] {
        let parent = chain.get(ordinal).ok_or("managed parent missing")?;
        let child = chain
            .get(
                ordinal
                    .checked_sub(1)
                    .ok_or("managed child ordinal underflow")?,
            )
            .ok_or("managed child missing")?;
        check_eq!(parent.children, vec![child.reference]);
        check_eq!(
            provisional_child_rows(&catalog, parent.reference, child.reference)?,
            (0, 0, 1)
        );
    }

    let oversized = managed_chain(257);
    let before = managed_install_counts(&catalog)?;
    check!(matches!(
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(ingress),
            &oversized,
            &mut admission_budget(),
        ),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(managed_install_counts(&catalog)?, before);
    Ok(())
}

#[test]
fn managed_256_late_provisional_corruption_leaves_no_registered_prefix() -> TestResult {
    use quanta_index_core::index::ObjectInstallScope;
    use quanta_index_core::index::ports::ManagedObjectInstallPort;

    let (root, catalog, ingress) = managed_adoption_fixture()?;
    let chain = managed_chain(256);
    let first = chain.first().ok_or("managed first object missing")?;
    let last = chain.last().ok_or("managed last object missing")?;
    let last_child = last.children.first().ok_or("managed last child missing")?;
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        &chain,
        &mut admission_budget(),
    )?;
    let before = managed_install_counts(&catalog)?;
    check_eq!(before, [1, 0, 256, 255, 0, 257]);
    {
        let connection = Connection::open(catalog.path())?;
        check_eq!(
            connection.execute(
                "UPDATE native_object_install_edge_v1 SET row_digest=zeroblob(32) WHERE parent=?1 AND child=?2",
                params![object_key(last.reference), object_key(*last_child)],
            )?,
            1
        );
    }

    {
        let mut guard = catalog.lock().map_err(|error| error.to_string())?;
        let tx = begin(&mut guard)?;
        check!(matches!(
            register_confirmed(&tx, &chain),
            Err(IndexError::Storage(_))
        ));
        // Inspect the failed transaction before rollback. Durable counts alone
        // would not detect a mutated prefix that was later rolled back.
        let inside: (i64, i64, i64, i64) = tx.query_row(
            "SELECT (SELECT COUNT(*) FROM native_object_v1),
                    (SELECT COUNT(*) FROM native_object_edge_v1),
                    (SELECT COUNT(*) FROM native_object_install_v1),
                    (SELECT COUNT(*) FROM native_object_install_edge_v1)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        check_eq!(inside, (1, 0, 256, 255));
        drop(tx);
        drop(guard);
    }

    check!(matches!(
        catalog.confirm_managed(
            ObjectInstallScope::Ingress(ingress),
            &chain,
            &mut admission_budget(),
        ),
        Err(IndexError::Storage(_))
    ));
    check_eq!(managed_install_counts(&catalog)?, before);
    check_eq!(
        provisional_child_rows(&catalog, first.reference, last.reference)?,
        (1, 0, 0)
    );
    check_eq!(
        provisional_child_rows(&catalog, last.reference, *last_child)?,
        (1, 1, 0)
    );
    drop(catalog);

    let catalog = open(root.path())?;
    check_eq!(managed_install_counts(&catalog)?, before);
    check_eq!(
        provisional_child_rows(&catalog, last.reference, *last_child)?,
        (1, 1, 0)
    );
    let connection = Connection::open(catalog.path())?;
    let stored: Vec<u8> = connection.query_row(
        "SELECT row_digest FROM native_object_install_edge_v1 WHERE parent=?1 AND child=?2",
        params![object_key(last.reference), object_key(*last_child)],
        |row| row.get(0),
    )?;
    check_eq!(stored, vec![0; 32]);
    check_eq!(
        connection.execute(
            "UPDATE native_object_install_edge_v1 SET row_digest=zeroblob(4096) WHERE parent=?1 AND child=?2",
            params![object_key(last.reference), object_key(*last_child)],
        )?,
        1
    );
    let prior_limit = reader_length_limit(&catalog, 2048)?;
    let result = catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        &chain,
        &mut admission_budget(),
    );
    check_eq!(reader_length_limit(&catalog, prior_limit)?, 2048);
    match result {
        Err(IndexError::Storage(reason))
            if reason == "provisional child metadata shape mismatch" => {}
        other => return Err(format!("oversized provisional edge guard: {other:?}").into()),
    }
    check_eq!(managed_install_counts(&catalog)?, before);
    let stored: Vec<u8> = connection.query_row(
        "SELECT row_digest FROM native_object_install_edge_v1 WHERE parent=?1 AND child=?2",
        params![object_key(last.reference), object_key(*last_child)],
        |row| row.get(0),
    )?;
    check_eq!(stored, vec![0; 4096]);
    Ok(())
}

#[test]
fn conflicting_duplicate_confirmation_preflights_entire_batch_before_transfer() -> TestResult {
    let root = tempfile::tempdir()?;
    let catalog = open(root.path())?;
    let leaf = object(245, vec![]);
    let leaf_ref = leaf.reference;
    let parent = object(246, vec![leaf.reference]);
    // Both representations satisfy the shape limits, but one digest cannot
    // acquire two different byte lengths in the same confirmation batch.
    let conflicting = RegisteredObject::unscoped(parent.reference, 2, parent.children.clone());

    {
        let mut guard = catalog.lock().map_err(|error| error.to_string())?;
        let tx = begin(&mut guard)?;
        object_install::reserve(&tx, &leaf)?;
        object_install::reserve(&tx, &parent)?;
        tx.commit()?;

        let tx = begin(&mut guard)?;
        let batch = [leaf.clone(), parent.clone(), conflicting];
        match register_confirmed(&tx, &batch) {
            Err(IndexError::CorruptObject(_)) => {}
            other => {
                return Err(format!("conflicting duplicate confirmation: {other:?}").into());
            }
        }
        // Inspect inside the still-open transaction: a rollback alone would
        // mask an implementation that transferred the earlier valid entries.
        let permanent_objects: i64 =
            tx.query_row("SELECT COUNT(*) FROM native_object_v1", [], |row| {
                row.get(0)
            })?;
        let permanent_edges: i64 =
            tx.query_row("SELECT COUNT(*) FROM native_object_edge_v1", [], |row| {
                row.get(0)
            })?;
        let provisional_objects: i64 =
            tx.query_row("SELECT COUNT(*) FROM native_object_install_v1", [], |row| {
                row.get(0)
            })?;
        let provisional_edges: i64 = tx.query_row(
            "SELECT COUNT(*) FROM native_object_install_edge_v1",
            [],
            |row| row.get(0),
        )?;
        if (
            permanent_objects,
            permanent_edges,
            provisional_objects,
            provisional_edges,
        ) != (0, 0, 2, 1)
        {
            return Err("conflicting duplicate changed custody before rejection".into());
        }
        drop(tx);
        drop(guard);
    }

    drop(catalog);
    let catalog = open(root.path())?;
    if provisional_child_rows(&catalog, leaf.reference, parent.reference)? != (1, 0, 0)
        || provisional_child_rows(&catalog, parent.reference, leaf.reference)? != (1, 1, 0)
    {
        return Err("conflicting duplicate changed durable provisional custody".into());
    }
    // The original registrations remain confirmable after the failed batch
    // and a fresh Catalog connection.
    let mut guard = catalog.lock().map_err(|error| error.to_string())?;
    let tx = begin(&mut guard)?;
    register_confirmed(&tx, &[leaf, parent.clone()])?;
    tx.commit()?;
    drop(guard);
    if provisional_child_rows(&catalog, parent.reference, leaf_ref)? != (0, 0, 1) {
        return Err("valid retry did not transfer the durable parent edge".into());
    }
    Ok(())
}

#[test]
fn managed_confirmation_rechecks_external_child_after_reserve_and_reopen() -> TestResult {
    use quanta_index_core::index::ObjectInstallScope;
    use quanta_index_core::index::ports::ManagedObjectInstallPort;

    let (root, catalog, donor) = managed_adoption_fixture()?;
    let external = object(241, vec![object(30, vec![]).reference]);
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(donor),
        std::slice::from_ref(&external),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(donor),
        std::slice::from_ref(&external),
        &mut admission_budget(),
    )?;

    let ingress = catalog.open_ingress(operation(12002, 153).key)?;
    let leaf = object(242, vec![]);
    let parent = object(243, vec![leaf.reference, external.reference]);
    let batch = [leaf.clone(), parent.clone()];
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        &batch,
        &mut admission_budget(),
    )?;

    let connection = Connection::open(catalog.path())?;
    let leaf_gc_before: (Vec<u8>, i64) = connection.query_row(
        "SELECT row_digest,(SELECT COUNT(*) FROM native_gc_queue_v1 WHERE object=?1) \
         FROM native_gc_object_v1 WHERE object=?1",
        rusqlite::params![object_key(leaf.reference)],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let edge_digest: Vec<u8> = connection.query_row(
        "SELECT row_digest FROM native_object_edge_v1 WHERE parent=?1",
        rusqlite::params![object_key(external.reference)],
        |row| row.get(0),
    )?;
    if connection.execute(
        "UPDATE native_object_edge_v1 SET row_digest=zeroblob(32) WHERE parent=?1",
        rusqlite::params![object_key(external.reference)],
    )? != 1
    {
        return Err("external child edge corruption did not reach one row".into());
    }
    drop(connection);
    match catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        &batch,
        &mut admission_budget(),
    ) {
        Err(IndexError::CorruptObject(_)) => {}
        other => return Err(format!("corrupt external edge confirmation: {other:?}").into()),
    }
    if provisional_child_rows(&catalog, leaf.reference, external.reference)? != (1, 0, 0)
        || provisional_child_rows(&catalog, parent.reference, leaf.reference)? != (1, 1, 0)
        || provisional_child_rows(&catalog, parent.reference, external.reference)? != (1, 1, 0)
    {
        return Err("failed managed confirmation changed provisional custody".into());
    }
    let connection = Connection::open(catalog.path())?;
    let committed: i64 = connection.query_row(
        "SELECT COUNT(*) FROM native_object_v1 WHERE object IN (?1,?2)",
        rusqlite::params![object_key(leaf.reference), object_key(parent.reference)],
        |row| row.get(0),
    )?;
    if committed != 0 {
        return Err("failed managed confirmation registered a batch prefix".into());
    }
    let confirmed_ingress: i64 = connection.query_row(
        "SELECT COUNT(*) FROM native_ingress_object_v1 WHERE object IN (?1,?2)",
        rusqlite::params![object_key(leaf.reference), object_key(parent.reference)],
        |row| row.get(0),
    )?;
    let leaf_gc_after: (Vec<u8>, i64) = connection.query_row(
        "SELECT row_digest,(SELECT COUNT(*) FROM native_gc_queue_v1 WHERE object=?1) \
         FROM native_gc_object_v1 WHERE object=?1",
        rusqlite::params![object_key(leaf.reference)],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if confirmed_ingress != 0 || leaf_gc_after != leaf_gc_before {
        return Err("failed managed confirmation changed ingress or GC custody".into());
    }
    if connection.execute(
        "UPDATE native_object_edge_v1 SET row_digest=?1 WHERE parent=?2",
        rusqlite::params![edge_digest, object_key(external.reference)],
    )? != 1
    {
        return Err("external child edge repair did not reach one row".into());
    }
    drop(connection);
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        &batch,
        &mut admission_budget(),
    )?;
    if provisional_child_rows(&catalog, parent.reference, external.reference)? != (0, 0, 1) {
        return Err("repaired child confirmation did not transfer custody".into());
    }

    // A later confirmation has its own transaction; the prior successful
    // child proof must not survive the Catalog reopen or cover this mutation.
    drop(catalog);
    let catalog = open(root.path())?;
    let later = object(244, vec![external.reference]);
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&later),
        &mut admission_budget(),
    )?;
    let connection = Connection::open(catalog.path())?;
    if connection.execute(
        "UPDATE native_object_v1 SET row_digest=zeroblob(32) WHERE object=?1",
        rusqlite::params![object_key(external.reference)],
    )? != 1
    {
        return Err("external child payload corruption did not reach one row".into());
    }
    drop(connection);
    match catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&later),
        &mut admission_budget(),
    ) {
        Err(IndexError::Storage(_)) => {}
        other => return Err(format!("corrupt external payload confirmation: {other:?}").into()),
    }
    if provisional_child_rows(&catalog, later.reference, external.reference)? != (1, 1, 0) {
        return Err("new-transaction refusal changed provisional child custody".into());
    }
    let connection = Connection::open(catalog.path())?;
    let committed: i64 = connection.query_row(
        "SELECT COUNT(*) FROM native_object_v1 WHERE object=?1",
        rusqlite::params![object_key(later.reference)],
        |row| row.get(0),
    )?;
    if committed != 0 {
        return Err("new-transaction refusal registered the later object".into());
    }
    Ok(())
}

fn admit_managed_attempt(
    catalog: &SqliteCatalog,
    op: ImmutableOperation,
) -> Result<AttemptRef, Box<dyn Error>> {
    use quanta_index_core::index::ObjectInstallScope;
    use quanta_index_core::index::ports::ManagedObjectInstallPort;
    let request = RegisteredObject::unscoped(op.request, 1, vec![]);
    let ingress = catalog.open_ingress(op.key)?;
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.seal_inputs(ingress, op.request)?;
    match catalog.admit(op, ingress, &[], &mut admission_budget())? {
        AdmissionOutcome::New(attempt) => Ok(attempt),
        other @ AdmissionOutcome::Existing(_) => {
            Err(format!("expected fresh managed attempt: {other:?}").into())
        }
    }
}

#[test]
fn managed_attempt_chain_elides_internal_adoption_but_preserves_queue_integrity() -> TestResult {
    use quanta_index_core::index::ObjectInstallScope;
    use quanta_index_core::index::ports::ManagedObjectInstallPort;

    let (root, catalog, _) = managed_adoption_fixture()?;
    let op = operation(12003, 154);
    let attempt = admit_managed_attempt(&catalog, op)?;
    let chain = managed_chain(256);
    let scope = ObjectInstallScope::Attempt(attempt);
    let mut counter_connection = Connection::open(catalog.path())?;
    let first_fence: i64 = counter_connection.query_row(
        "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
        [],
        |row| row.get(0),
    )?;
    let mut scalar_issued = Vec::new();
    {
        let reference = counter_connection.transaction()?;
        for _ordinal in 0..256 {
            scalar_issued.push(next_fence(&reference)?);
        }
        reference.rollback()?;
    }
    begin_registration_reads(&catalog)?;
    catalog.reserve_managed(scope, &chain, &mut admission_budget())?;
    // 256 lifecycle reads; normalization and queueing share each exact row.
    // The attempt also authenticates its one operation request registration.
    check_eq!(end_registration_reads(&catalog)?, (1, 0, 1, 256));
    check_eq!(RESERVATION_PROBES.with(std::cell::Cell::get), (0, 0, 256));
    check_eq!(FENCE_READS.with(std::cell::Cell::get), 1);
    check_eq!(FENCE_WRITES.with(std::cell::Cell::get), 1);
    let after_fences = first_fence.checked_add(256).ok_or("fence gold overflow")?;
    for (ordinal, object) in chain.iter().enumerate() {
        let generation: i64 = counter_connection.query_row(
            "SELECT generation FROM native_gc_object_v1 WHERE object=?1",
            params![object_key(object.reference)],
            |row| row.get(0),
        )?;
        check_eq!(
            u64::try_from(generation)?,
            *scalar_issued
                .get(ordinal)
                .ok_or("missing independent scalar fence")?
        );
    }
    check_eq!(
        counter_connection.query_row(
            "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        after_fences
    );
    drop(counter_connection);
    // All 256 permanent absence probes remain inside the conditional inserts.
    // Existing slots must still read every provisional header and scoped intent.
    begin_registration_reads(&catalog)?;
    catalog.reserve_managed(scope, &chain, &mut admission_budget())?;
    check_eq!(end_registration_reads(&catalog)?, (257, 0, 1, 256));
    check_eq!(
        RESERVATION_PROBES.with(std::cell::Cell::get),
        (256, 256, 256)
    );
    check_eq!(FENCE_READS.with(std::cell::Cell::get), 0);
    check_eq!(FENCE_WRITES.with(std::cell::Cell::get), 0);
    let connection = Connection::open(catalog.path())?;
    let first = chain.first().ok_or("missing chain first")?;
    let last = chain.last().ok_or("missing chain last")?;
    let before = managed_install_counts(&catalog)?;
    // A corrupt pre-existing scope row must survive unchanged, and roll back
    // both a freshly inserted provisional prefix and its fresh intent binding.
    let first_key = object_key(first.reference);
    let intent_digest: Vec<u8> = connection.query_row("SELECT row_digest FROM native_object_intent_v1 WHERE operation=?1 AND fence=?2 AND object=?3", params![operation_key(attempt.operation)?, int(attempt.fence)?, &first_key], |row| row.get(0))?;
    check_eq!(connection.execute("UPDATE native_object_intent_v1 SET row_digest=zeroblob(32) WHERE operation=?1 AND fence=?2 AND object=?3", params![operation_key(attempt.operation)?, int(attempt.fence)?, &first_key])?, 1);
    let prefix = object(249, vec![]);
    check!(
        matches!(catalog.reserve_managed(scope, &[prefix.clone(), first.clone()], &mut admission_budget()), Err(IndexError::Storage(message)) if message == "native catalog row digest mismatch")
    );
    check_eq!(managed_install_counts(&catalog)?, before);
    check_eq!(
        connection.query_row(
            "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        after_fences
    );
    let prefix_intents: i64 = connection.query_row(
        "SELECT COUNT(*) FROM native_object_intent_v1 WHERE object=?1",
        params![object_key(prefix.reference)],
        |row| row.get(0),
    )?;
    check_eq!(prefix_intents, 0);
    let corrupt: Vec<u8> = connection.query_row("SELECT row_digest FROM native_object_intent_v1 WHERE operation=?1 AND fence=?2 AND object=?3", params![operation_key(attempt.operation)?, int(attempt.fence)?, &first_key], |row| row.get(0))?;
    check_eq!(corrupt, vec![0; 32]);
    check_eq!(connection.execute("UPDATE native_object_intent_v1 SET row_digest=?4 WHERE operation=?1 AND fence=?2 AND object=?3", params![operation_key(attempt.operation)?, int(attempt.fence)?, &first_key, &intent_digest])?, 1);
    for (object, confirming) in [(first, false), (last, true)] {
        let key = object_key(object.reference);
        let digest: Vec<u8> = connection.query_row(
            "SELECT row_digest FROM native_gc_queue_v1 WHERE object=?1",
            params![&key],
            |row| row.get(0),
        )?;
        check_eq!(
            connection.execute(
                "UPDATE native_gc_queue_v1 SET row_digest=zeroblob(32) WHERE object=?1",
                params![&key]
            )?,
            1
        );
        let result = if confirming {
            catalog.confirm_managed(scope, &chain, &mut admission_budget())
        } else {
            catalog.reserve_managed(scope, &chain, &mut admission_budget())
        };
        check!(
            matches!(result, Err(IndexError::Storage(message)) if message == "reclaim queue digest mismatch")
        );
        // A late queue failure must roll back registration, transfer, and GC kind.
        check_eq!(managed_install_counts(&catalog)?, before);
        let registered: i64 = connection.query_row("SELECT SUM(kind) FROM native_gc_object_v1 WHERE object IN (SELECT object FROM native_object_install_v1)", [], |row| row.get(0))?;
        check_eq!(registered, 0);
        let corrupt: Vec<u8> = connection.query_row(
            "SELECT row_digest FROM native_gc_queue_v1 WHERE object=?1",
            params![&key],
            |row| row.get(0),
        )?;
        check_eq!(corrupt, vec![0; 32]);
        check_eq!(
            connection.execute(
                "UPDATE native_gc_queue_v1 SET row_digest=?2 WHERE object=?1",
                params![&key, &digest]
            )?,
            1
        );
    }
    drop(connection);
    begin_registration_reads(&catalog)?;
    catalog.confirm_managed(scope, &chain, &mut admission_budget())?;
    check_eq!(end_registration_reads(&catalog)?, (257, 0, 1, 511));
    drop(catalog);
    let catalog = open(root.path())?;
    let guard = catalog.lock().map_err(|error| error.to_string())?;
    for object in &chain {
        check!(object_install::confirmed_matches(&guard, object)?);
        let registered_and_queued: (i64, i64) = guard.query_row(
            "SELECT kind,(SELECT COUNT(*) FROM native_gc_queue_v1 WHERE object=?1) FROM native_gc_object_v1 WHERE object=?1", params![object_key(object.reference)], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        check_eq!(registered_and_queued, (1, 1));
    }
    drop(guard);
    Ok(())
}

#[test]
fn allocation_intent_readers_refuse_oversized_and_nonblob_fields_before_copy() -> TestResult {
    use quanta_index_core::index::ObjectInstallScope;
    use quanta_index_core::index::ports::ManagedObjectInstallPort;

    let (_root, catalog, _) = managed_adoption_fixture()?;
    let op = operation(12004, 155);
    let request = RegisteredObject::unscoped(op.request, 1, vec![]);
    let ingress = catalog.open_ingress(op.key)?;
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&request),
        &mut admission_budget(),
    )?;
    catalog.seal_inputs(ingress, op.request)?;
    let attempt = match catalog.admit(op, ingress, &[], &mut admission_budget())? {
        AdmissionOutcome::New(attempt) => attempt,
        AdmissionOutcome::Existing(_) => {
            return Err("expected fresh allocation-intent attempt".into());
        }
    };
    let object = object(248, vec![]);
    catalog.reserve_managed(
        ObjectInstallScope::Attempt(attempt),
        std::slice::from_ref(&object),
        &mut admission_budget(),
    )?;
    let other_ingress = catalog.open_ingress(operation(12005, 156).key)?;
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(other_ingress),
        std::slice::from_ref(&object),
        &mut admission_budget(),
    )?;
    let payload = encode(&object)?;
    let key = object_key(object.reference);
    let connection = Connection::open(catalog.path())?;
    for (table, domain, is_attempt) in [
        (
            "native_object_intent_v1",
            b"qidx.native.intent.v1".as_slice(),
            true,
        ),
        (
            "native_ingress_intent_v1",
            b"qidx.native.ingressintent.v1".as_slice(),
            false,
        ),
    ] {
        let checksum = digest(domain, &payload).to_vec();
        for (column, invalid) in [
            ("payload", "zeroblob(1048577)"),
            ("payload", "zeroblob(0)"),
            ("payload", "CAST('invalid' AS TEXT)"),
            ("row_digest", "zeroblob(4097)"),
            (
                "row_digest",
                "CAST('01234567890123456789012345678901' AS TEXT)",
            ),
        ] {
            check_eq!(
                connection.execute(
                    &format!("UPDATE {table} SET {column}={invalid} WHERE object=?1"),
                    params![&key]
                )?,
                1
            );
            // The stored row exceeds this limit in the oversized cases. A raw
            // BLOB read fails in SQLite; the guarded readers must reach their
            // explicit shape refusal without materializing that BLOB.
            let prior_limit = reader_length_limit(&catalog, 4096)?;
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            if invalid == "zeroblob(1048577)" || invalid == "zeroblob(4097)" {
                let raw: rusqlite::Result<Vec<u8>> = guard.query_row(
                    &format!("SELECT {column} FROM {table} WHERE object=?1"),
                    params![&key],
                    |row| row.get(0),
                );
                check!(
                    matches!(raw, Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::TooBig)
                );
            }
            let results = if is_attempt {
                [
                    require_reserved_intent(&guard, attempt, &object, &payload),
                    require_intent(&guard, attempt, &object),
                ]
            } else {
                [
                    require_reserved_ingress_intent(
                        &guard,
                        other_ingress.operation,
                        &object,
                        &payload,
                    ),
                    ingress_intent(&guard, other_ingress.operation, &object).map(|_payload| ()),
                ]
            };
            drop(guard);
            let _restored_limit = reader_length_limit(&catalog, prior_limit)?;
            for result in results {
                check!(
                    matches!(result, Err(IndexError::Storage(message)) if message.contains("native allocation intent shape invalid"))
                );
            }
            check_eq!(
                connection.execute(
                    &format!("UPDATE {table} SET payload=?2,row_digest=?3 WHERE object=?1"),
                    params![&key, &payload, &checksum]
                )?,
                1
            );
        }
    }
    let guard = catalog.lock().map_err(|error| error.to_string())?;
    require_reserved_intent(&guard, attempt, &object, &payload)?;
    require_intent(&guard, attempt, &object)?;
    require_reserved_ingress_intent(&guard, other_ingress.operation, &object, &payload)?;
    check_eq!(
        ingress_intent(&guard, other_ingress.operation, &object)?,
        Some(payload)
    );
    Ok(())
}

#[test]
fn fence_sequence_preserves_exact_scalar_order_and_one_final_update() -> TestResult {
    let (_root, catalog, _) = managed_adoption_fixture()?;
    let mut guard = catalog.lock().map_err(|error| error.to_string())?;
    let start: i64 = guard.query_row(
        "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
        [],
        |row| row.get(0),
    )?;
    let mut actual = Vec::new();
    let mut work = admission_budget();
    FENCE_WRITES.with(|counter| counter.set(0));
    guard.trace_v2(
        rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
        Some(observe_registration_reads),
    );
    {
        let tx = begin(&mut guard)?;
        fence_counter::ReservationSequence::run(&tx, 256, &mut work, |sequence, budget| {
            for _ordinal in 0..256 {
                actual.push(sequence.next(budget)?);
            }
            Ok(())
        })?;
        check_eq!(work.usage().reads, 1);
        check_eq!(FENCE_WRITES.with(std::cell::Cell::get), 1);
        check_eq!(
            tx.query_row(
                "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            start.checked_add(256).ok_or("counter gold overflow")?
        );
        tx.rollback()?;
    }
    FENCE_WRITES.with(|counter| counter.set(0));
    let mut scalar = Vec::new();
    {
        let tx = begin(&mut guard)?;
        for _ordinal in 0..256 {
            scalar.push(next_fence(&tx)?);
        }
        check_eq!(FENCE_WRITES.with(std::cell::Cell::get), 256);
        tx.rollback()?;
    }
    guard.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
    check_eq!(actual, scalar);
    check_eq!(actual.first().copied(), Some(u64::try_from(start)?));
    check_eq!(
        guard.query_row(
            "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        start
    );
    Ok(())
}

#[test]
fn fence_sequence_final_cas_rejects_other_writer_and_rolls_back_prefix() -> TestResult {
    let (root, catalog, _) = managed_adoption_fixture()?;
    let before = managed_install_counts(&catalog)?;
    let prefix = object(247, vec![]);
    let mut guard = catalog.lock().map_err(|error| error.to_string())?;
    let start: i64 = guard.query_row(
        "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
        [],
        |row| row.get(0),
    )?;
    {
        let tx = begin(&mut guard)?;
        let mut work = admission_budget();
        let result =
            fence_counter::ReservationSequence::run(&tx, 2, &mut work, |sequence, budget| {
                let _first = sequence.next(budget)?;
                object_install::reserve(&tx, &prefix)?;
                // This valid same-transaction scalar writer must defeat the
                // retained snapshot at the final CAS, not be overwritten.
                let _independent = next_fence(&tx)?;
                let _second = sequence.next(budget)?;
                Ok(())
            });
        check!(
            matches!(result, Err(IndexError::Storage(message)) if message == "native catalog advance fence counter affected 0 rows")
        );
        tx.rollback()?;
    }
    check_eq!(
        guard.query_row(
            "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        start
    );
    drop(guard);
    drop(catalog);
    let reopened = open(root.path())?;
    check_eq!(managed_install_counts(&reopened)?, before);
    let guard = reopened.lock().map_err(|error| error.to_string())?;
    let leaked: i64 = guard.query_row(
        "SELECT COUNT(*) FROM native_object_install_v1 WHERE object=?1",
        params![object_key(prefix.reference)],
        |row| row.get(0),
    )?;
    drop(guard);
    check_eq!(leaked, 0);
    Ok(())
}

#[test]
fn fence_sequence_ignored_bound_error_cannot_flush() -> TestResult {
    let (_root, catalog, _) = managed_adoption_fixture()?;
    let mut guard = catalog.lock().map_err(|error| error.to_string())?;
    let start: i64 = guard.query_row(
        "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
        [],
        |row| row.get(0),
    )?;
    let tx = begin(&mut guard)?;
    let mut work = admission_budget();
    let result: Result<(), IndexError> = fence_counter::ReservationSequence::run(
        &tx,
        1,
        &mut work,
        |sequence, budget| {
            let _first = sequence.next(budget)?;
            let exhausted = sequence.next(budget);
            if !matches!(exhausted, Err(IndexError::BackendViolation(message)) if message == "fence sequence exceeded admitted inputs")
            {
                return Err(IndexError::Storage("sequence bound oracle failed".into()));
            }
            let poisoned = sequence.next(budget);
            if !matches!(poisoned, Err(IndexError::BackendViolation(message)) if message == "fence sequence already refused")
            {
                return Err(IndexError::Storage("sequence poison oracle failed".into()));
            }
            Ok(())
        },
    );
    check!(
        matches!(result, Err(IndexError::BackendViolation(message)) if message == "fence sequence already refused")
    );
    check_eq!(work.usage().reads, 1);
    tx.rollback()?;
    check_eq!(
        guard.query_row(
            "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        start
    );
    Ok(())
}

#[test]
fn fence_sequence_partial_failure_and_late_cancel_leave_no_reopened_prefix() -> TestResult {
    let (root, catalog, _) = managed_adoption_fixture()?;
    let before = managed_install_counts(&catalog)?;
    let mut guard = catalog.lock().map_err(|error| error.to_string())?;
    let start: i64 = guard.query_row(
        "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
        [],
        |row| row.get(0),
    )?;
    for (ordinal, cancel_after_closure) in [(245_u8, false), (246_u8, true)] {
        let prefix = object(ordinal, vec![]);
        let tx = begin(&mut guard)?;
        let mut work = admission_budget();
        let result: Result<(), IndexError> =
            fence_counter::ReservationSequence::run(&tx, 1, &mut work, |sequence, budget| {
                object_install::reserve(&tx, &prefix)?;
                let _issued = sequence.next(budget)?;
                if cancel_after_closure {
                    budget.request().cancel_handle().cancel();
                    Ok(())
                } else {
                    Err(IndexError::Cancelled)
                }
            });
        check_eq!(result, Err(IndexError::Cancelled));
        tx.rollback()?;
    }
    check_eq!(
        guard.query_row(
            "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        start
    );
    drop(guard);
    drop(catalog);
    let reopened = open(root.path())?;
    check_eq!(managed_install_counts(&reopened)?, before);
    let guard = reopened.lock().map_err(|error| error.to_string())?;
    for ordinal in [245_u8, 246_u8] {
        let prefix = object(ordinal, vec![]);
        let leaked: i64 = guard.query_row(
            "SELECT COUNT(*) FROM native_object_install_v1 WHERE object=?1",
            params![object_key(prefix.reference)],
            |row| row.get(0),
        )?;
        check_eq!(leaked, 0);
    }
    drop(guard);
    Ok(())
}

#[test]
fn fence_sequence_overflow_refuses_without_counter_rewrite() -> TestResult {
    let (_root, catalog, _) = managed_adoption_fixture()?;
    let mut guard = catalog.lock().map_err(|error| error.to_string())?;
    let original: (i64, Vec<u8>) = guard.query_row(
        "SELECT next,row_digest FROM native_counter_v1 WHERE name='attempt_fence'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let tx = begin(&mut guard)?;
    check_eq!(
        tx.execute(
            "UPDATE native_counter_v1 SET next=?1,row_digest=?2 WHERE name='attempt_fence'",
            params![
                i64::MAX,
                fields_digest(b"qidx.native.counter.v1", &("attempt_fence", i64::MAX))?
            ],
        )?,
        1
    );
    let mut work = admission_budget();
    let result: Result<(), IndexError> =
        fence_counter::ReservationSequence::run(&tx, 1, &mut work, |sequence, budget| {
            let _issued = sequence.next(budget)?;
            Ok(())
        });
    check_eq!(result, Err(IndexError::ResourceLimited));
    let overflow_row: (i64, Vec<u8>) = tx.query_row(
        "SELECT next,row_digest FROM native_counter_v1 WHERE name='attempt_fence'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    check_eq!(overflow_row.0, i64::MAX);
    tx.rollback()?;
    let restored: (i64, Vec<u8>) = guard.query_row(
        "SELECT next,row_digest FROM native_counter_v1 WHERE name='attempt_fence'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    drop(guard);
    check_eq!(restored, original);
    Ok(())
}

#[test]
fn canonical_fence_reader_refuses_corruption_domain_and_overflow_without_rewrite() -> TestResult {
    use rusqlite::types::Value;
    let (_root, catalog, _) = managed_adoption_fixture()?;
    let mut guard = catalog.lock().map_err(|error| error.to_string())?;
    let cases = [
        (
            1,
            Value::Blob(vec![0; 32]),
            "native counter digest mismatch",
        ),
        (
            1,
            Value::Blob(vec![0; 4097]),
            "native counter checksum shape invalid",
        ),
        (
            1,
            Value::Text("01234567890123456789012345678901".into()),
            "native counter checksum shape invalid",
        ),
        (
            0,
            Value::Blob(fields_digest(
                b"qidx.native.counter.v1",
                &("attempt_fence", 0_i64),
            )?),
            "native counter must be positive",
        ),
        (
            -1,
            Value::Blob(fields_digest(
                b"qidx.native.counter.v1",
                &("attempt_fence", -1_i64),
            )?),
            "native counter must be positive",
        ),
        (
            i64::MAX,
            Value::Blob(fields_digest(
                b"qidx.native.counter.v1",
                &("attempt_fence", i64::MAX),
            )?),
            "overflow",
        ),
    ];
    for (next, checksum, reason) in cases {
        let tx = begin(&mut guard)?;
        tx.execute_batch("PRAGMA ignore_check_constraints=ON")?;
        check_eq!(
            tx.execute(
                "UPDATE native_counter_v1 SET next=?1,row_digest=?2 WHERE name='attempt_fence'",
                params![next, checksum]
            )?,
            1
        );
        tx.execute_batch("PRAGMA ignore_check_constraints=OFF")?;
        let before: (i64, String, i64) = tx.query_row("SELECT next,typeof(row_digest),length(row_digest) FROM native_counter_v1 WHERE name='attempt_fence'", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let prior_limit = tx.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 2048)?;
        let result = next_fence(&tx);
        let _restored = tx.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, prior_limit)?;
        if reason == "overflow" {
            check_eq!(result, Err(IndexError::ResourceLimited));
        } else {
            check!(matches!(result, Err(IndexError::Storage(message)) if message == reason));
        }
        let after: (i64, String, i64) = tx.query_row("SELECT next,typeof(row_digest),length(row_digest) FROM native_counter_v1 WHERE name='attempt_fence'", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        check_eq!(after, before);
        tx.rollback()?;
    }
    drop(guard);
    Ok(())
}

/// Guard metadata with real `SQLite` length limits.
///
/// Both custody ledgers use the same reader; every malformed row remains byte-for-byte untouched.
#[test]
fn metadata_reader_guards_shape_before_materialization_and_admission() -> TestResult {
    use rusqlite::types::Value;
    let (_root, catalog, _) = managed_adoption_fixture()?;
    let leaf = object(249, vec![]);
    let canonical_payload = encode(&leaf)?;
    let connection = Connection::open(catalog.path())?;
    for (table, ledger, domain) in [
        (
            "native_object_v1",
            object_install::MetadataLedger::Registered,
            b"qidx.native.object.v1".as_slice(),
        ),
        (
            "native_object_install_v1",
            object_install::MetadataLedger::Provisional,
            b"qidx.native.install.v1".as_slice(),
        ),
    ] {
        let original_digest = digest(domain, &canonical_payload).to_vec();
        let _changed = connection.execute(
            &format!("INSERT INTO {table}(object,payload,row_digest) VALUES(?1,?2,?3)"),
            params![
                object_key(leaf.reference),
                &canonical_payload,
                &original_digest
            ],
        )?;
        let cases = [
            (Value::Blob(vec![]), Value::Blob(original_digest.clone())),
            (
                Value::Blob(vec![0; 1_048_577]),
                Value::Blob(original_digest.clone()),
            ),
            (
                Value::Text(String::from_utf8(canonical_payload.clone())?),
                Value::Blob(original_digest.clone()),
            ),
            (
                Value::Blob(canonical_payload.clone()),
                Value::Blob(vec![0; 4096]),
            ),
            (
                Value::Blob(canonical_payload.clone()),
                Value::Text("x".repeat(32)),
            ),
        ];
        for (payload, checksum) in cases {
            let _changed = connection.execute(
                &format!("UPDATE {table} SET payload=?1,row_digest=?2 WHERE object=?3"),
                params![&payload, &checksum, object_key(leaf.reference)],
            )?;
            let prior_limit = reader_length_limit(&catalog, 2048)?;
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            if matches!(&payload, Value::Blob(bytes) if bytes.len() > 1_048_576)
                || matches!(&checksum, Value::Blob(bytes) if bytes.len() > 2048)
            {
                let raw: rusqlite::Result<(Vec<u8>, Vec<u8>)> = guard.query_row(
                    &format!("SELECT payload,row_digest FROM {table} WHERE object=?1"),
                    params![object_key(leaf.reference)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                );
                check!(
                    matches!(raw, Err(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::TooBig)
                );
            }
            let mut admissions = 0;
            let result = object_install::read_metadata(
                &guard,
                leaf.reference,
                ledger,
                || IndexError::CorruptObject("metadata shape gold".into()),
                |_bytes| {
                    admissions += 1;
                    Ok(())
                },
            );
            drop(guard);
            check_eq!(reader_length_limit(&catalog, prior_limit)?, 2048);
            check!(
                matches!(result, Err(IndexError::CorruptObject(reason)) if reason == "metadata shape gold")
            );
            check_eq!(admissions, 0);
            let unchanged: (Value, Value) = connection.query_row(
                &format!("SELECT payload,row_digest FROM {table} WHERE object=?1"),
                params![object_key(leaf.reference)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            check_eq!(unchanged, (payload, checksum));
        }
        // A valid shape must fund decoding before JSON is inspected. Its CRC
        // is also checked afresh on every call, including the next transaction.
        for bytes in [b"{".as_slice(), canonical_payload.as_slice()] {
            let _changed = connection.execute(
                &format!("UPDATE {table} SET payload=?1,row_digest=?2 WHERE object=?3"),
                params![
                    bytes,
                    digest(domain, bytes).to_vec(),
                    object_key(leaf.reference)
                ],
            )?;
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            let result = object_install::read_metadata(
                &guard,
                leaf.reference,
                ledger,
                || IndexError::CorruptObject("metadata shape gold".into()),
                |charged| {
                    if charged
                        != u64::try_from(bytes.len())
                            .map_err(|_error| IndexError::ResourceLimited)?
                            + 32
                    {
                        return Err(IndexError::BackendViolation(
                            "metadata admission byte gold".into(),
                        ));
                    }
                    Ok(())
                },
            );
            if bytes == canonical_payload.as_slice() {
                check_eq!(result?, Some(leaf.clone()));
            } else {
                check!(matches!(result, Err(IndexError::Storage(_))));
            }
            let denied = object_install::read_metadata(
                &guard,
                leaf.reference,
                ledger,
                || IndexError::CorruptObject("metadata shape gold".into()),
                |_bytes| Err(IndexError::ResourceLimited),
            );
            check!(matches!(denied, Err(IndexError::ResourceLimited)));
            drop(guard);
        }
        let _changed = connection.execute(
            &format!("UPDATE {table} SET row_digest=zeroblob(32) WHERE object=?1"),
            params![object_key(leaf.reference)],
        )?;
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        check!(matches!(
            object_install::read_metadata(
                &guard,
                leaf.reference,
                ledger,
                || IndexError::CorruptObject("metadata shape gold".into()),
                |_bytes| Ok(()),
            ),
            Err(IndexError::Storage(_))
        ));
        drop(guard);
        let _changed = connection.execute(
            &format!("DELETE FROM {table} WHERE object=?1"),
            params![object_key(leaf.reference)],
        )?;
    }
    Ok(())
}

mod repeated_children;
