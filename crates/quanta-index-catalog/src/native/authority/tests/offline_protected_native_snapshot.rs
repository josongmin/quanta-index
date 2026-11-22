use super::offline_protected_routes::{
    in_second_workspace, protected_budget_v1, route_set, second_workspace, stage_routes,
    two_published_heads,
};
use super::*;
use crate::io_admission::{CatalogIoScope, CatalogReadOnlyIoAdmission};
use quanta_index_core::index::{
    MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

#[derive(Clone, Default)]
struct SnapshotMemory(Arc<AtomicU64>);

impl MemoryLeaseOwner for SnapshotMemory {
    fn release(&self, bytes: u64) {
        assert!(
            self.0
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| live
                    .checked_sub(bytes))
                .is_ok()
        );
    }
}

impl MemoryScopePort for SnapshotMemory {
    fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous_live = self.0.fetch_add(bytes, Ordering::AcqRel);
        Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
    }
}

#[derive(Default)]
struct SnapshotReadIo {
    entered: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
}

struct SnapshotReadScope(Arc<AtomicUsize>);

impl Drop for SnapshotReadScope {
    fn drop(&mut self) {
        assert_eq!(self.0.fetch_sub(1, Ordering::AcqRel), 1);
    }
}

impl CatalogIoScope for SnapshotReadScope {
    fn is_capacity_refusal(&self, _: &rusqlite::Error) -> bool {
        false
    }
}

impl CatalogReadOnlyIoAdmission for SnapshotReadIo {
    fn read_only_vfs_name(&self) -> &str {
        "unix"
    }

    fn enter_read_only_scope(
        &self,
        _: &quanta_index_core::RequestBudgetV1,
        _: u64,
        _: u64,
    ) -> Box<dyn CatalogIoScope> {
        let _previous_entries = self.entered.fetch_add(1, Ordering::AcqRel);
        assert_eq!(self.active.fetch_add(1, Ordering::AcqRel), 0);
        Box::new(SnapshotReadScope(Arc::clone(&self.active)))
    }
}

#[test]
fn public_snapshot_visitor_runs_after_catalog_scope_and_keeps_rows_funded() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir()?;
    let (catalog, primary_operation, primary, source_operation, source) =
        two_published_heads(root.path())?;
    let mut routes = route_set(primary_operation, primary, source_operation, source)?;
    let _primary_outside_protected_set = routes.heads.remove(0);
    routes.validate()?;
    let (ingress, route_ref) = stage_routes(&catalog, &routes, operation(20_013, 215).key)?;
    let selected = catalog.select_native_boot_manifest_with_protected_routes_if_current(
        primary_operation.key,
        route_ref,
        &routes,
        ingress,
        &mut protected_budget_v1(),
    )?;
    let path = catalog.path().to_path_buf();
    catalog
        .lock()?
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    drop(catalog);
    let wal_path = path.with_file_name(format!(
        "{}-wal",
        path.file_name().ok_or("catalog name")?.to_string_lossy()
    ));
    check!(std::fs::metadata(&wal_path).map_or(true, |file| file.len() == 0));
    for input in [&path, &wal_path] {
        if input.exists() {
            std::fs::set_permissions(input, std::fs::Permissions::from_mode(0o600))?;
        }
    }

    let io = SnapshotReadIo::default();
    let memory = SnapshotMemory::default();
    let mut budget = admission_budget();
    budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(memory.clone()))));
    let expected_snapshot = read_only::visit_native_protected_snapshot_rows_v1(
        root.path(),
        &io,
        &selected,
        &routes,
        &mut budget,
        |snapshot, _budget| {
            assert_eq!(io.entered.load(Ordering::Acquire), 1);
            assert_eq!(io.active.load(Ordering::Acquire), 0);
            // The fixture's row backing is small. A live connection alone
            // would retain at least the fixed 2 MiB verification reservation.
            let live = memory.0.load(Ordering::Acquire);
            assert!((1..2 * 1024 * 1024).contains(&live));
            assert_eq!(snapshot.source_routes, route_ref);
            let payload = serde_json::to_vec(snapshot)
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            quanta_index_contract::index::native_immutable_object_ref_v1(&payload, &snapshot.roots)
                .ok_or(IndexError::DigestConflict)
        },
    )?;
    check_eq!(io.active.load(Ordering::Acquire), 0);
    check_eq!(memory.0.load(Ordering::Acquire), 0);
    let witness = read_only::read_native_protected_snapshot_witness_v1(
        root.path(),
        &io,
        &selected,
        &routes,
        &mut budget,
    )?;
    check_eq!(witness.origin_v1().snapshot, expected_snapshot);
    check_eq!(
        witness.object_references_v1().collect::<Vec<_>>(),
        [expected_snapshot]
    );
    check_eq!(io.entered.load(Ordering::Acquire), 2);
    check_eq!(io.active.load(Ordering::Acquire), 0);
    check_eq!(memory.0.load(Ordering::Acquire), 0);
    Ok(())
}

#[test]
fn selected_native_snapshot_exports_terminal_ledger_and_survives_retired_request() -> TestResult {
    let root = tempfile::tempdir()?;
    let (catalog, primary_operation, primary, prior_source_operation, prior_source) =
        two_published_heads(root.path())?;

    let mut source_operation = operation(20_010, 210);
    source_operation.key.workspace = second_workspace();
    source_operation.source_mode = SourceOperationMode::Advance;
    let event = SourceEvent {
        producer: source_operation.key.producer,
        stream: source_operation.key.stream,
        stream_epoch: source_operation.key.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: digest_id(211),
    };
    let attempt = admit_new_events(&catalog, source_operation, &[event])?;
    let frontier = object(212, vec![]);
    reserve_confirm(&catalog, attempt, std::slice::from_ref(&frontier))?;
    let mut current = in_second_workspace(view(2, prior_source.profile, None));
    current.source.frontiers = MapRoot(Some(frontier.reference));
    current.identity.digest = current.identity_digest()?;
    check_eq!(
        commit_empty(
            &catalog,
            PreparedPublication {
                attempt,
                expected: prior_source.identity,
                next: current.clone(),
                work_actions: WorkActionSet::empty(),
                complete_work: None,
                source_events: vec![event],
                projection_events: vec![],
                expected_source: Some(prior_source.source),
                source_mode: SourceOperationMode::Advance,
            },
        )?,
        OperationStatus::Published(current.identity)
    );
    let mut refused = operation(20_011, 213);
    refused.key.workspace = second_workspace();
    refused.source_mode = SourceOperationMode::Advance;
    let refused_events = (20_u8..70)
        .map(|stream| SourceEvent {
            producer: refused.key.producer,
            stream: StreamId(id(stream)),
            stream_epoch: refused.key.stream_epoch,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(stream),
        })
        .collect::<Vec<_>>();
    let refused_attempt = admit_new_events(&catalog, refused, &refused_events)?;
    check_eq!(
        catalog.refuse(refused_attempt, Refusal::InvalidInput)?,
        OperationStatus::Refused(Refusal::InvalidInput)
    );
    let direct = Connection::open(catalog.path())?;
    let retained_row_bytes: i64 = direct.query_row(
        "SELECT length(payload) FROM native_operation_v1 WHERE operation=?1",
        params![operation_key(refused.key)?],
        |row| row.get(0),
    )?;
    check!(retained_row_bytes > 4096);
    check_eq!(
        direct.execute(
            "DELETE FROM native_operation_object_v1 WHERE operation_id=(SELECT operation_id FROM native_operation_v1 WHERE operation=?1) AND object=?2",
            params![operation_key(refused.key)?, object_key(refused.request)],
        )?,
        1
    );
    check_eq!(
        direct.execute(
            "DELETE FROM native_object_v1 WHERE object=?1",
            params![object_key(refused.request)],
        )?,
        1
    );
    drop(direct);

    let mut routes = route_set(
        primary_operation,
        primary.clone(),
        source_operation,
        current,
    )?;
    let _primary_outside_protected_set = routes.heads.remove(0);
    routes.validate()?;
    let (ingress, route_ref) = stage_routes(&catalog, &routes, operation(20_012, 214).key)?;
    let selected = catalog.select_native_boot_manifest_with_protected_routes_if_current(
        primary_operation.key,
        route_ref,
        &routes,
        ingress,
        &mut protected_budget_v1(),
    )?;
    let root_nonce = catalog.native_root_nonce(&mut protected_budget_v1())?;
    let guard = catalog.lock()?;
    check_eq!(
        read_only::verify_native_published_view_connection_v1(
            &guard,
            prior_source.identity,
            Some(&routes),
            &mut protected_budget_v1(),
        )?,
        prior_source
    );
    check!(
        read_only::verify_native_published_view_connection_v1(
            &guard,
            prior_source.identity,
            None,
            &mut protected_budget_v1(),
        )
        .is_err()
    );
    check!(matches!(
        read_only::visit_native_protected_snapshot_connection_v1(
            &guard,
            root_nonce,
            &selected,
            &routes,
            &mut admission_budget(),
            |_, _| Ok(()),
        ),
        Err(IndexError::ResourceLimited)
    ));
    let snapshot = read_only::visit_native_protected_snapshot_connection_v1(
        &guard,
        root_nonce,
        &selected,
        &routes,
        &mut protected_budget_v1(),
        |snapshot, budget| {
            budget.checkpoint()?;
            if snapshot.source_root_nonce != Digest(root_nonce)
                || snapshot.source_routes != route_ref
                || snapshot.operation_rows.len() != 3
                || snapshot.event_claims.len() != 51
                || snapshot.frontiers.len() != 1
                || snapshot.source_authorities.len() != 1
                || !snapshot.roots.contains(&route_ref)
                || snapshot.roots.contains(&refused.request)
            {
                return Err(IndexError::DigestConflict);
            }
            let keys = snapshot
                .operation_rows
                .iter()
                .map(|bytes| decode::<OperationRow>(bytes).map(|row| row.immutable.key))
                .collect::<Result<Vec<_>, _>>()?;
            if !keys.contains(&prior_source_operation.key)
                || !keys.contains(&source_operation.key)
                || !keys.contains(&refused.key)
            {
                return Err(IndexError::DigestConflict);
            }
            // The fixture owns this copy after the funded visitor returns.
            Ok(snapshot.clone())
        },
    )?;
    // Independent V1 order oracle: read the held SQL rows with the original
    // payload-bearing ORDER BY queries, not the key-only snapshot scanner.
    let scope = workspace_key(second_workspace())?;
    let ordered_operations = {
        let mut statement = guard.prepare(
            "SELECT payload FROM native_operation_v1 WHERE workspace=?1 ORDER BY operation",
        )?;
        let rows = statement.query_map(params![&scope], |row| row.get::<_, Vec<u8>>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let ordered_claims = {
        let mut statement = guard.prepare(
            "SELECT c.payload FROM native_event_claim_v1 c \
             JOIN native_operation_v1 o ON c.operation=o.operation \
             WHERE o.workspace=?1 ORDER BY c.identity",
        )?;
        let rows = statement.query_map(params![&scope], |row| row.get::<_, Vec<u8>>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    check_eq!(snapshot.operation_rows, ordered_operations);
    check_eq!(snapshot.event_claims, ordered_claims);
    check_eq!(
        read_only::compare_native_protected_snapshot_connection_v1(
            &guard,
            &selected,
            &routes,
            &snapshot,
            &mut protected_budget_v1(),
        ),
        Ok(())
    );
    let mut missing_operation = operation(20_014, 216).key;
    missing_operation.workspace = second_workspace();
    let orphan_event = SourceEvent {
        producer: missing_operation.producer,
        stream: StreamId(id(250)),
        stream_epoch: missing_operation.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: digest_id(217),
    };
    let orphan = EventClaim {
        operation: missing_operation,
        event: orphan_event,
        state: EventClaimState::Pending,
    };
    let orphan_identity = event_identity(missing_operation, orphan_event)?;
    let orphan_payload = encode(&orphan)?;
    let foreign_keys: bool = guard.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    guard.pragma_update(None, "foreign_keys", false)?;
    check_eq!(
        guard.execute(
            "INSERT INTO native_event_claim_v1(identity,operation,payload,row_digest)
             VALUES (?1,?2,?3,?4)",
            params![
                &orphan_identity,
                operation_key(missing_operation)?,
                &orphan_payload,
                digest(b"qidx.native.eventclaim.v1", &orphan_payload).to_vec(),
            ],
        )?,
        1
    );
    guard.pragma_update(None, "foreign_keys", foreign_keys)?;
    check_eq!(
        read_only::compare_native_protected_snapshot_connection_v1(
            &guard,
            &selected,
            &routes,
            &snapshot,
            &mut protected_budget_v1(),
        ),
        Err(IndexError::DigestConflict)
    );
    check_eq!(
        guard.execute(
            "DELETE FROM native_event_claim_v1 WHERE identity=?1",
            params![&orphan_identity],
        )?,
        1
    );
    let mut changed_frontier = snapshot.clone();
    let first_frontier = changed_frontier
        .frontiers
        .first_mut()
        .ok_or("missing fixture frontier")?;
    first_frontier.sequence += 1;
    check_eq!(
        read_only::compare_native_protected_snapshot_connection_v1(
            &guard,
            &selected,
            &routes,
            &changed_frontier,
            &mut protected_budget_v1(),
        ),
        Err(IndexError::DigestConflict)
    );
    let mut reordered = snapshot.clone();
    reordered.operation_rows.swap(0, 1);
    check_eq!(
        read_only::compare_native_protected_snapshot_connection_v1(
            &guard,
            &selected,
            &routes,
            &reordered,
            &mut protected_budget_v1(),
        ),
        Err(IndexError::DigestConflict)
    );
    let mut missing = snapshot.clone();
    check!(missing.operation_rows.pop().is_some());
    check_eq!(
        read_only::compare_native_protected_snapshot_connection_v1(
            &guard,
            &selected,
            &routes,
            &missing,
            &mut protected_budget_v1(),
        ),
        Err(IndexError::DigestConflict)
    );
    let mut extra = snapshot.clone();
    let first_operation = snapshot
        .operation_rows
        .first()
        .ok_or("expected carried operation")?;
    extra.operation_rows.push(first_operation.clone());
    check_eq!(
        read_only::compare_native_protected_snapshot_connection_v1(
            &guard,
            &selected,
            &routes,
            &extra,
            &mut protected_budget_v1(),
        ),
        Err(IndexError::DigestConflict)
    );
    drop(guard);

    let guard = catalog.lock()?;
    let original_frontier_digest: Vec<u8> = guard.query_row(
        "SELECT row_digest FROM native_frontier_v1 WHERE workspace=?1",
        params![&scope],
        |row| row.get(0),
    )?;
    let _changed = guard.execute(
        "UPDATE native_frontier_v1 SET row_digest=zeroblob(32) WHERE workspace=?1",
        params![workspace_key(second_workspace())?],
    )?;
    check!(matches!(
        read_only::visit_native_protected_snapshot_connection_v1(
            &guard,
            root_nonce,
            &selected,
            &routes,
            &mut protected_budget_v1(),
            |_, _| Ok(()),
        ),
        Err(IndexError::DigestConflict)
    ));
    check!(matches!(
        read_only::compare_native_protected_snapshot_connection_v1(
            &guard,
            &selected,
            &routes,
            &snapshot,
            &mut protected_budget_v1(),
        ),
        Err(IndexError::DigestConflict)
    ));
    check_eq!(
        guard.execute(
            "UPDATE native_frontier_v1 SET row_digest=?2 WHERE workspace=?1",
            params![&scope, original_frontier_digest],
        )?,
        1
    );
    let visit_rows = || {
        read_only::visit_native_protected_snapshot_connection_v1(
            &guard,
            root_nonce,
            &selected,
            &routes,
            &mut protected_budget_v1(),
            |_, _| Ok(()),
        )
    };
    let limit = rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH;
    let previous_limit = guard.set_limit(limit, 128 * 1024)?;
    check_eq!(visit_rows(), Ok(()));
    let _tight_limit = guard.set_limit(limit, previous_limit)?;
    for table in ["native_frontier_v1", "native_source_authority_v1"] {
        let original_digest: Vec<u8> = guard.query_row(
            &format!("SELECT row_digest FROM {table} WHERE workspace=?1"),
            params![&scope],
            |row| row.get(0),
        )?;
        check_eq!(
            guard.execute(
                &format!("UPDATE {table} SET row_digest=zeroblob(256 * 1024) WHERE workspace=?1"),
                params![&scope],
            )?,
            1
        );
        // A direct projection exceeds SQLite's length limit before the Rust
        // decoder runs. A bounded SQL projection yields the typed refusal.
        let _previous_limit = guard.set_limit(limit, 128 * 1024)?;
        let observed = visit_rows();
        let _tight_limit = guard.set_limit(limit, previous_limit)?;
        check_eq!(observed, Err(IndexError::DigestConflict));
        check_eq!(
            guard.execute(
                &format!("UPDATE {table} SET row_digest=?2 WHERE workspace=?1"),
                params![&scope, original_digest],
            )?,
            1
        );
    }
    Ok(())
}
