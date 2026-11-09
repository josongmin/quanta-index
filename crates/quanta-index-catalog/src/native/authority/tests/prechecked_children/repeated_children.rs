//! Independent SQL/registration/rollback laws for transaction-local child checks.
use super::*;
use crate::native::authority::registration_codec;
use quanta_index_core::index::ObjectInstallScope;
use quanta_index_core::index::ports::ManagedObjectInstallPort;

fn install_child(
    catalog: &SqliteCatalog,
    ingress: IngressRef,
    value: u8,
) -> Result<RegisteredObject, Box<dyn Error>> {
    let child = object(value, vec![]);
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&child),
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        std::slice::from_ref(&child),
        &mut admission_budget(),
    )?;
    Ok(child)
}

fn parents(child: ObjectRef, repeats: usize) -> Vec<RegisteredObject> {
    (50..66)
        .map(|id| object(id, vec![child; repeats]))
        .collect()
}

#[test]
fn repeated_external_children_are_authenticated_once_per_real_transaction() -> TestResult {
    let (_root, catalog, ingress) = managed_adoption_fixture()?;
    let child = install_child(&catalog, ingress, 193)?;
    let attempt = admit_managed_attempt(&catalog, operation(12101, 171))?;
    let scope = ObjectInstallScope::Attempt(attempt);
    let batch = parents(child.reference, 4);
    begin_registration_reads(&catalog)?;
    catalog.reserve_managed(scope, &batch, &mut admission_budget())?;
    // One operation-request read and one complete shared-child authentication.
    check_eq!(end_registration_reads(&catalog)?, (2, 0, 2, 17));
    begin_registration_reads(&catalog)?;
    catalog.confirm_managed(scope, &batch, &mut admission_budget())?;
    // Sixteen parent slots, operation request and shared child are independent.
    check_eq!(end_registration_reads(&catalog)?, (18, 0, 2, 17));
    check_eq!(METADATA_READS.with(std::cell::Cell::get), (18, 16, 0));
    let connection = Connection::open(catalog.path())?;
    for parent in &batch {
        require_object(&connection, parent.reference)?;
        let payload: Vec<u8> = connection.query_row(
            "SELECT payload FROM native_object_v1 WHERE object=?1",
            [object_key(parent.reference)],
            |row| row.get(0),
        )?;
        let stored = registration_codec::decode(&payload)?;
        check_eq!(stored.children, vec![child.reference; 4]);
        let edges: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_edge_v1 WHERE parent=?1",
            [object_key(parent.reference)],
            |row| row.get(0),
        )?;
        check_eq!(edges, 1);
    }
    // A new transaction must authenticate the child again before any transfer.
    let before = managed_install_counts(&catalog)?;
    let old: Vec<u8> = connection.query_row(
        "SELECT row_digest FROM native_object_v1 WHERE object=?1",
        [object_key(child.reference)],
        |row| row.get(0),
    )?;
    check_eq!(
        connection.execute(
            "UPDATE native_object_v1 SET row_digest=zeroblob(32) WHERE object=?1",
            [object_key(child.reference)],
        )?,
        1
    );
    check!(matches!(
        catalog.confirm_managed(scope, &batch, &mut admission_budget()),
        Err(IndexError::Storage(message)) if message == "native catalog row digest mismatch"
    ));
    check_eq!(managed_install_counts(&catalog)?, before);
    check_eq!(
        connection.execute(
            "UPDATE native_object_v1 SET row_digest=?2 WHERE object=?1",
            params![object_key(child.reference), old],
        )?,
        1
    );
    catalog.confirm_managed(scope, &batch, &mut admission_budget())?;
    Ok(())
}

#[test]
fn repeated_child_reservation_bills_only_adoptions_that_are_reached() -> TestResult {
    let mut observed = Vec::new();
    for repeats in [1, 4] {
        let (_root, catalog, ingress) = managed_adoption_fixture()?;
        let child = install_child(&catalog, ingress, 193)?;
        let attempt = admit_managed_attempt(&catalog, operation(12102, 172))?;
        let mut work = admission_budget();
        catalog.reserve_managed(
            ObjectInstallScope::Attempt(attempt),
            &parents(child.reference, repeats),
            &mut work,
        )?;
        observed.push(work.usage().reads);
    }
    // Same parent and distinct-child counts; repeated physical edge entries
    // remain in metadata but add no child SQL/adoption reads in this transaction.
    check_eq!(observed.first(), observed.get(1));
    Ok(())
}

#[test]
fn hash_collisions_and_late_corrupt_children_never_acquire_authentication() -> TestResult {
    let (_root, catalog, ingress) = managed_adoption_fixture()?;
    let first = install_child(&catalog, ingress, 193)?;
    let collision = install_child(&catalog, ingress, 129)?;
    let attempt = admit_managed_attempt(&catalog, operation(12103, 173))?;
    let scope = ObjectInstallScope::Attempt(attempt);
    let batch = vec![
        object(50, vec![first.reference; 4]),
        object(51, vec![collision.reference, first.reference]),
    ];
    begin_registration_reads(&catalog)?;
    catalog.reserve_managed(scope, &batch, &mut admission_budget())?;
    // Full-ref equality: both distinct references are checked once, even when
    // their first digest byte maps to the same initial window slot.
    check_eq!(end_registration_reads(&catalog)?, (3, 0, 3, 4));
    let before = managed_install_counts(&catalog)?;
    let connection = Connection::open(catalog.path())?;
    check_eq!(
        connection.execute(
            "UPDATE native_object_v1 SET row_digest=zeroblob(32) WHERE object=?1",
            [object_key(collision.reference)],
        )?,
        1
    );
    check!(matches!(
        catalog.confirm_managed(scope, &batch, &mut admission_budget()),
        Err(IndexError::Storage(message)) if message == "native catalog row digest mismatch"
    ));
    check_eq!(managed_install_counts(&catalog)?, before);
    // Even the early valid parent remains provisional; no partial registration.
    for parent in &batch {
        let registered: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_v1 WHERE object=?1",
            [object_key(parent.reference)],
            |row| row.get(0),
        )?;
        check_eq!(registered, 0);
    }
    Ok(())
}

#[derive(Default)]
struct MemoryCounts {
    live: AtomicU64,
    invalid_release: std::sync::atomic::AtomicBool,
}
#[derive(Clone)]
struct WindowMemory {
    counts: Arc<MemoryCounts>,
    limit: u64,
}
impl quanta_index_core::index::MemoryLeaseOwner for WindowMemory {
    fn release(&self, bytes: u64) {
        if self
            .counts
            .live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_sub(bytes)
            })
            .is_err()
        {
            self.counts.invalid_release.store(true, Ordering::Release);
        }
    }
}
impl quanta_index_core::index::MemoryScopePort for WindowMemory {
    fn reserve(
        &self,
        bytes: u64,
        lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        if lifetime != quanta_index_core::index::MemoryLifetime::Temporary {
            return Err(IndexError::BackendViolation(
                "child window requires original temporary memory".into(),
            ));
        }
        let _old = self
            .counts
            .live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes).filter(|next| *next <= self.limit)
            })
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(quanta_index_core::index::MemoryLease::new(
            bytes,
            Arc::new(self.clone()),
        ))
    }
}

#[test]
fn child_window_uses_original_memory_and_releases_before_next_phase_or_refusal() -> TestResult {
    let (_root, catalog, ingress) = managed_adoption_fixture()?;
    let child = install_child(&catalog, ingress, 193)?;
    let attempt = admit_managed_attempt(&catalog, operation(12104, 174))?;
    let scope = ObjectInstallScope::Attempt(attempt);
    let batch = parents(child.reference, 4);
    let counts = Arc::new(MemoryCounts::default());
    let mut work = admission_budget();
    // Enough for one bounded window but not overlapping billing/auth windows.
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        WindowMemory {
            counts: Arc::clone(&counts),
            limit: 4096,
        },
    ))));
    catalog.reserve_managed(scope, &batch, &mut work)?;
    check_eq!(counts.live.load(Ordering::Acquire), 0);
    let before = managed_install_counts(&catalog)?;
    let mut denied = admission_budget();
    denied.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        WindowMemory {
            counts: Arc::clone(&counts),
            limit: 1,
        },
    ))));
    check!(matches!(
        catalog.confirm_managed(scope, &batch, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(counts.live.load(Ordering::Acquire), 0);
    check_eq!(managed_install_counts(&catalog)?, before);
    work.request().cancel_handle().cancel();
    check!(matches!(
        catalog.confirm_managed(scope, &batch, &mut work),
        Err(IndexError::Cancelled)
    ));
    check_eq!(managed_install_counts(&catalog)?, before);
    let mut funded = admission_budget();
    funded.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        WindowMemory {
            counts: Arc::clone(&counts),
            limit: 4096,
        },
    ))));
    catalog.confirm_managed(scope, &batch, &mut funded)?;
    check_eq!(counts.live.load(Ordering::Acquire), 0);
    check!(!counts.invalid_release.load(Ordering::Acquire));
    Ok(())
}

fn large_child_batch() -> Vec<RegisteredObject> {
    (100_u8..170).map(|id| object(id, vec![])).collect()
}

#[test]
fn large_child_window_bills_distinct_refs_once_and_preserves_physical_order() -> TestResult {
    let mut outcomes = Vec::new();
    for reverse in [false, true] {
        let (_root, catalog, ingress) = managed_adoption_fixture()?;
        let children = large_child_batch();
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(ingress),
            &children,
            &mut admission_budget(),
        )?;
        catalog.confirm_managed(
            ObjectInstallScope::Ingress(ingress),
            &children,
            &mut admission_budget(),
        )?;
        let attempt = admit_managed_attempt(&catalog, operation(12105, 175))?;
        let scope = ObjectInstallScope::Attempt(attempt);
        let mut child_refs = children
            .iter()
            .map(|child| child.reference)
            .collect::<Vec<_>>();
        if reverse {
            child_refs.reverse();
        }
        // Repeats before and after the 64th distinct reference exercise the
        // same complete set. The registered metadata retains physical order.
        let first = *child_refs.first().ok_or("large child fixture empty")?;
        let last = *child_refs.last().ok_or("large child fixture empty")?;
        child_refs.insert(0, last);
        child_refs.push(first);
        child_refs.push(last);
        let parent = object(50, child_refs.clone());

        begin_registration_reads(&catalog)?;
        catalog.reserve_managed(
            scope,
            std::slice::from_ref(&parent),
            &mut admission_budget(),
        )?;
        let reserved_reads = end_registration_reads(&catalog)?;
        check_eq!(reserved_reads, (71, 0, 71, 71));
        begin_registration_reads(&catalog)?;
        catalog.confirm_managed(
            scope,
            std::slice::from_ref(&parent),
            &mut admission_budget(),
        )?;
        let confirmed_reads = end_registration_reads(&catalog)?;
        check_eq!(confirmed_reads, (72, 0, 71, 71));
        outcomes.push((reserved_reads, confirmed_reads));

        let connection = Connection::open(catalog.path())?;
        let payload: Vec<u8> = connection.query_row(
            "SELECT payload FROM native_object_v1 WHERE object=?1",
            [object_key(parent.reference)],
            |row| row.get(0),
        )?;
        check_eq!(registration_codec::decode(&payload)?.children, child_refs);
        let edge_count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_edge_v1 WHERE parent=?1",
            [object_key(parent.reference)],
            |row| row.get(0),
        )?;
        check_eq!(edge_count, 70);

        // A later transaction must authenticate all children anew. An exact
        // corrupt child cannot inherit this transaction's prior success.
        let corrupt = children.get(69).ok_or("last child absent")?.reference;
        let old: Vec<u8> = connection.query_row(
            "SELECT row_digest FROM native_object_v1 WHERE object=?1",
            [object_key(corrupt)],
            |row| row.get(0),
        )?;
        let before = managed_install_counts(&catalog)?;
        check_eq!(
            connection.execute(
                "UPDATE native_object_v1 SET row_digest=zeroblob(32) WHERE object=?1",
                [object_key(corrupt)],
            )?,
            1
        );
        check!(matches!(
            catalog.confirm_managed(scope, std::slice::from_ref(&parent), &mut admission_budget()),
            Err(IndexError::Storage(message)) if message == "native catalog row digest mismatch"
        ));
        check_eq!(managed_install_counts(&catalog)?, before);
        check_eq!(
            connection.execute(
                "UPDATE native_object_v1 SET row_digest=?2 WHERE object=?1",
                params![object_key(corrupt), old],
            )?,
            1
        );
    }
    check_eq!(outcomes.first(), outcomes.get(1));
    Ok(())
}

#[test]
fn large_child_window_refuses_unfunded_original_scope_and_refunds() -> TestResult {
    let (_root, catalog, ingress) = managed_adoption_fixture()?;
    let children = large_child_batch();
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        &children,
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        &children,
        &mut admission_budget(),
    )?;
    let attempt = admit_managed_attempt(&catalog, operation(12106, 176))?;
    let scope = ObjectInstallScope::Attempt(attempt);
    let parent = object(50, children.iter().map(|child| child.reference).collect());
    let counts = Arc::new(MemoryCounts::default());
    let before = managed_install_counts(&catalog)?;
    let mut denied = admission_budget();
    denied.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        WindowMemory {
            counts: Arc::clone(&counts),
            limit: 4096,
        },
    ))));
    check!(matches!(
        catalog.reserve_managed(scope, std::slice::from_ref(&parent), &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(counts.live.load(Ordering::Acquire), 0);
    check_eq!(managed_install_counts(&catalog)?, before);
    let mut funded = admission_budget();
    funded.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        WindowMemory {
            counts: Arc::clone(&counts),
            limit: 64 * 1024,
        },
    ))));
    catalog.reserve_managed(scope, std::slice::from_ref(&parent), &mut funded)?;
    check_eq!(counts.live.load(Ordering::Acquire), 0);
    catalog.confirm_managed(scope, std::slice::from_ref(&parent), &mut funded)?;
    check_eq!(counts.live.load(Ordering::Acquire), 0);
    check!(!counts.invalid_release.load(Ordering::Acquire));
    Ok(())
}

#[test]
fn aggregate_small_parent_edges_keep_one_transaction_child_proof() -> TestResult {
    let (_root, catalog, ingress) = managed_adoption_fixture()?;
    let children = (100_u8..228)
        .map(|id| object(id, vec![]))
        .collect::<Vec<_>>();
    catalog.reserve_managed(
        ObjectInstallScope::Ingress(ingress),
        &children,
        &mut admission_budget(),
    )?;
    catalog.confirm_managed(
        ObjectInstallScope::Ingress(ingress),
        &children,
        &mut admission_budget(),
    )?;
    let child_refs = children
        .iter()
        .map(|child| child.reference)
        .collect::<Vec<_>>();
    let parent_rows = |ids: std::ops::Range<u8>| {
        ids.zip(child_refs.chunks_exact(32))
            .map(|(id, refs)| object(id, refs.to_vec()))
            .collect::<Vec<_>>()
    };
    let mut batch = parent_rows(50..54);
    batch.extend(parent_rows(54..58));
    check_eq!(batch.len(), 8);
    check!(batch.iter().all(|parent| parent.children.len() == 32));
    let attempt = admit_managed_attempt(&catalog, operation(12107, 177))?;
    let scope = ObjectInstallScope::Attempt(attempt);
    let counts = Arc::new(MemoryCounts::default());
    let before = managed_install_counts(&catalog)?;

    let mut denied = admission_budget();
    denied.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        WindowMemory {
            counts: Arc::clone(&counts),
            limit: 4096,
        },
    ))));
    check!(matches!(
        catalog.reserve_managed(scope, &batch, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(counts.live.load(Ordering::Acquire), 0);
    check_eq!(managed_install_counts(&catalog)?, before);

    let mut funded = admission_budget();
    funded.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        WindowMemory {
            counts: Arc::clone(&counts),
            limit: 131_072,
        },
    ))));
    begin_registration_reads(&catalog)?;
    catalog.reserve_managed(scope, &batch, &mut funded)?;
    check_eq!(end_registration_reads(&catalog)?, (129, 0, 129, 136));
    check_eq!(counts.live.load(Ordering::Acquire), 0);

    let reserved = managed_install_counts(&catalog)?;
    let mut denied_confirm = admission_budget();
    denied_confirm.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        WindowMemory {
            counts: Arc::clone(&counts),
            limit: 4096,
        },
    ))));
    check!(matches!(
        catalog.confirm_managed(scope, &batch, &mut denied_confirm),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(counts.live.load(Ordering::Acquire), 0);
    check_eq!(managed_install_counts(&catalog)?, reserved);

    let connection = Connection::open(catalog.path())?;
    let late = children.get(127).ok_or("late child absent")?.reference;
    let old_digest: Vec<u8> = connection.query_row(
        "SELECT row_digest FROM native_object_v1 WHERE object=?1",
        [object_key(late)],
        |row| row.get(0),
    )?;
    check_eq!(
        connection.execute(
            "UPDATE native_object_v1 SET row_digest=zeroblob(32) WHERE object=?1",
            [object_key(late)],
        )?,
        1
    );
    check!(matches!(
        catalog.confirm_managed(scope, &batch, &mut admission_budget()),
        Err(IndexError::Storage(message)) if message == "native catalog row digest mismatch"
    ));
    check_eq!(managed_install_counts(&catalog)?, reserved);
    for parent in &batch {
        let registered: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_v1 WHERE object=?1",
            [object_key(parent.reference)],
            |row| row.get(0),
        )?;
        check_eq!(registered, 0);
    }
    check_eq!(
        connection.execute(
            "UPDATE native_object_v1 SET row_digest=?2 WHERE object=?1",
            params![object_key(late), old_digest],
        )?,
        1
    );

    begin_registration_reads(&catalog)?;
    catalog.confirm_managed(scope, &batch, &mut funded)?;
    check_eq!(end_registration_reads(&catalog)?, (137, 0, 129, 136));
    check_eq!(counts.live.load(Ordering::Acquire), 0);
    check!(!counts.invalid_release.load(Ordering::Acquire));
    let confirmed = managed_install_counts(&catalog)?;
    check_eq!(
        confirmed[0],
        reserved[0].checked_add(8).ok_or("object count overflow")?
    );
    check_eq!(
        confirmed[2],
        reserved[2]
            .checked_sub(8)
            .ok_or("provisional count underflow")?
    );
    Ok(())
}
