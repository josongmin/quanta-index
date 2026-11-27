//! Candidate selection is an offline read of an existing Published receipt.

use super::*;
use quanta_index_catalog::{
    catalog_dir, verify_native_boot_candidate, verify_native_boot_current_candidate,
    verify_native_boot_scope_selected_view_v1,
    verify_native_rollback_with_source_publications, verify_native_source_root_publications_v1,
};
use quanta_index_core::index::ports::{
    CatalogExactHistoryPort as _, CatalogGenesisPort as _, CatalogPublicationPort as _,
    ImmutableObjectPort, InputStagingPort as _, OperationJournalPort as _, WorkActionCursor,
};
use quanta_index_core::index::{
    AdmissionOutcome, AttemptRef, CatalogView, CollectionId, Digest, ImmutableOperation,
    IndexError, IndexProfile, MapRoot, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
    MemoryScopePort, ObjectBatchInput, ObjectRef, OperationKey, OperationStatus,
    PreparedPublication, ProducerId, Refusal, RegisteredObject, RegisteredObjectBatch, SlotId,
    SourceCut, SourceOperationMode, StoredObject, StreamId, TenantId, ViewRef, WorkAction,
    WorkActionSet, WorkBudget, WorkLimits, WorkspaceKey, WorktreeId,
};
use std::{
    os::unix::fs::MetadataExt as _,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

struct PublicationReadMeter {
    live: AtomicU64,
    limit: AtomicU64,
}

#[test]
fn selected_scope_read_distinguishes_empty_from_mismatched_current_head() -> TestResult {
    let mut seeded = None;
    let fixture = fixture_with_boot_input(|root| {
        seeded = Some(seed_current_native_pair(root)?);
        Ok(16 * 1024 * 1024)
    })?;
    let (_, _, boot) = seeded.ok_or("paired Catalog fixture absent")?;
    let candidate = verify_native_boot_current_candidate(
        &fixture.root, fixture.vfs.as_ref(), key(35), &mut read_budget()?,
    )?;
    let selected = candidate.manifest();
    let before = files(&fixture.root)?;
    let mut work = read_budget()?;
    require(
        verify_native_boot_scope_selected_view_v1(
            &fixture.root, fixture.vfs.as_ref(), selected,
            boot.identity.workspace, Some(&boot), &mut work,
        )?,
        "selected current scope was not observed",
    )?;
    let mut absent = boot.identity.workspace;
    absent.slot = SlotId([9; 16]);
    require(
        !verify_native_boot_scope_selected_view_v1(
            &fixture.root, fixture.vfs.as_ref(), selected, absent, None, &mut work,
        )?,
        "absent configured scope appeared present",
    )?;
    require(
        matches!(
            verify_native_boot_scope_selected_view_v1(
                &fixture.root, fixture.vfs.as_ref(), selected,
                boot.identity.workspace, None, &mut work,
            ),
            Err(IndexError::Conflict)
        ),
        "current scope without a selected view was accepted",
    )?;
    require(files(&fixture.root)? == before, "scope read changed Catalog bytes")?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

impl MemoryLeaseOwner for PublicationReadMeter {
    fn release(&self, bytes: u64) {
        let previous = self.live.fetch_sub(bytes, Ordering::SeqCst);
        assert!(
            previous >= bytes,
            "publication reader refunded unowned memory"
        );
    }
}

struct PublicationReadScope(Arc<PublicationReadMeter>);

impl MemoryScopePort for PublicationReadScope {
    fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let limit = self.0.limit.load(Ordering::SeqCst);
        let _previous_live = self
            .0
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(bytes).filter(|total| *total <= limit)
            })
            .map_err(|_| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 64 * 1024 * 1024,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn read_budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 0,
            bytes: 64 * 1024 * 1024,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn key(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        },
        epoch: 7,
        producer: ProducerId([8; 16]),
        stream: StreamId([9; 16]),
        stream_epoch: 12,
        sequence,
    }
}

fn object(value: u8) -> RegisteredObject {
    RegisteredObject::unscoped(ObjectRef(Digest([value; 32])), 1, vec![])
}

fn view(sequence: u64, profile: ObjectRef) -> Result<CatalogView, IndexError> {
    let mut value = CatalogView {
        identity: ViewRef {
            workspace: key(0).workspace,
            epoch: 7,
            sequence,
            digest: Digest([0; 32]),
        },
        profile,
        source: SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        projections: MapRoot(None),
        members: MapRoot(None),
        coverage: MapRoot(None),
    };
    value.identity.digest = value
        .identity_digest()
        .map_err(|error| IndexError::Storage(error.to_string()))?;
    Ok(value)
}

struct Identity(RegisteredObject);

impl ImmutableObjectPort for Identity {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        budget.checkpoint()?;
        let empty = serde_json::to_vec(&IndexProfile { instances: vec![] })
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        if payload != empty || !children.is_empty() {
            return Err(IndexError::InvalidInput(
                "unexpected genesis profile".into(),
            ));
        }
        Ok(self.0.clone())
    }

    fn put(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation("identity-only fixture".into()))
    }

    fn install(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation("identity-only fixture".into()))
    }

    fn install_batch(
        &self,
        _inputs: &[ObjectBatchInput],
        _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        Err(IndexError::BackendViolation("identity-only fixture".into()))
    }

    fn get(
        &self,
        _reference: ObjectRef,
        _budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        Err(IndexError::BackendViolation("identity-only fixture".into()))
    }
}

struct EmptyActions;

impl WorkActionCursor for EmptyActions {
    fn next_action(&mut self, budget: &mut WorkBudget) -> Result<Option<WorkAction>, IndexError> {
        budget.checkpoint()?;
        Ok(None)
    }
}

fn admit(catalog: &SqliteCatalog, sequence: u64) -> Result<AttemptRef, IndexError> {
    let immutable = ImmutableOperation {
        key: key(sequence),
        request: object(u8::try_from(sequence).map_err(|_error| IndexError::ResourceLimited)?)
            .reference,
        source_mode: SourceOperationMode::Unchanged,
    };
    let ingress = catalog.open_ingress(immutable.key)?;
    let request = RegisteredObject::unscoped(immutable.request, 1, vec![]);
    catalog.reserve_inputs(ingress, std::slice::from_ref(&request))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&request))?;
    catalog.seal_inputs(ingress, immutable.request)?;
    match catalog.admit(immutable, ingress, &[], &mut budget()?)? {
        AdmissionOutcome::New(attempt) => Ok(attempt),
        AdmissionOutcome::Existing(_status) => Err(IndexError::BackendViolation(
            "new fixture operation replayed".into(),
        )),
    }
}

fn publish(
    catalog: &SqliteCatalog,
    attempt: AttemptRef,
    current: &CatalogView,
    next: CatalogView,
) -> Result<OperationStatus, IndexError> {
    catalog.commit_transition(
        PreparedPublication {
            attempt,
            source_mode: SourceOperationMode::Unchanged,
            expected: current.identity,
            next,
            work_actions: WorkActionSet::empty(),
            complete_work: None,
            source_events: vec![],
            projection_events: vec![],
            expected_source: None,
        },
        &mut EmptyActions,
        &mut budget()?,
    )
}

struct Seed {
    selected: OperationKey,
    other: OperationKey,
    pending: OperationKey,
    refused: OperationKey,
    selected_view: ViewRef,
    other_view: ViewRef,
}

fn seed(root: &Path, retire_other: bool) -> Result<Seed, Box<dyn std::error::Error>> {
    let catalog = SqliteCatalog::open(root, Duration::from_secs(1))?;
    let profile = object(30);
    let genesis = view(0, profile.reference)?;
    let stage = catalog.begin_bootstrap(
        genesis.identity.workspace,
        genesis.identity.epoch,
        &Identity(profile.clone()),
        &mut budget()?,
    )?;
    catalog.reserve_bootstrap(stage, &profile)?;
    catalog.confirm_bootstrap(stage, &profile)?;
    require(
        catalog.publish_bootstrap(stage, genesis.clone(), &mut budget()?)? == genesis.identity,
        "genesis view mismatch",
    )?;
    let first_attempt = admit(&catalog, 31)?;
    let first = view(1, profile.reference)?;
    require(
        publish(&catalog, first_attempt, &genesis, first.clone())?
            == OperationStatus::Published(first.identity),
        "first publication did not finish",
    )?;
    let selected = catalog.select_native_boot_manifest(key(31), &mut budget()?)?;
    require(
        selected.publication == first,
        "selected first view mismatch",
    )?;
    let second_attempt = admit(&catalog, 32)?;
    let second = view(2, profile.reference)?;
    require(
        publish(&catalog, second_attempt, &first, second.clone())?
            == OperationStatus::Published(second.identity),
        "second publication did not finish",
    )?;
    let _pending_attempt = admit(&catalog, 33)?;
    let refused_attempt = admit(&catalog, 34)?;
    require(
        catalog.refuse(refused_attempt, Refusal::Conflict)?
            == OperationStatus::Refused(Refusal::Conflict),
        "fixture refusal did not finish",
    )?;
    if retire_other {
        let third_attempt = admit(&catalog, 35)?;
        let third = view(3, profile.reference)?;
        require(
            publish(&catalog, third_attempt, &second, third)?
                == OperationStatus::Published(view(3, profile.reference)?.identity),
            "third publication did not finish",
        )?;
        let floor = catalog.advance_exact_history_floor(
            key(31).workspace,
            key(31).epoch,
            3,
            &mut budget()?,
        )?;
        require(floor == 3, "history floor failed to advance")?;
        let retirement = catalog.retire_exact_history_page(
            key(31).workspace,
            key(31).epoch,
            None,
            8,
            &mut budget()?,
        )?;
        require(
            retirement.retired >= 2,
            "retired view fixture kept old rows",
        )?;
    }
    drop(catalog);
    let directory = catalog_dir(root);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    for entry in fs::read_dir(directory)? {
        fs::set_permissions(entry?.path(), fs::Permissions::from_mode(0o600))?;
    }
    Ok(Seed {
        selected: key(31),
        other: key(32),
        pending: key(33),
        refused: key(34),
        selected_view: first.identity,
        other_view: second.identity,
    })
}

type CatalogFile = (PathBuf, u64, i64, i64, Vec<u8>);

fn files(root: &Path) -> Result<Vec<CatalogFile>, Box<dyn std::error::Error>> {
    let mut rows = vec![];
    for entry in fs::read_dir(catalog_dir(root))? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        rows.push((
            entry.path(),
            metadata.ino(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            fs::read(entry.path())?,
        ));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(rows)
}

#[test]
fn candidate_and_selected_are_distinct_in_one_read_without_catalog_mutation() -> TestResult {
    let mut seeded = None;
    let fixture = fixture_with_boot_input(|root| {
        seeded = Some(seed(root, false)?);
        Ok(16 * 1024 * 1024)
    })?;
    let seeded = seeded.ok_or("candidate fixture missing seed")?;
    let before = files(&fixture.root)?;
    let mut work = read_budget()?;
    let result =
        verify_native_boot_candidate(&fixture.root, fixture.vfs.as_ref(), seeded.other, &mut work)?;
    require(
        result
            .snapshot()
            .selected
            .as_ref()
            .map(|value| value.operation.key)
            == Some(seeded.selected),
        "candidate replaced the persisted selection",
    )?;
    require(
        result.manifest().operation.key == seeded.other
            && result.manifest().publication.identity == seeded.other_view
            && result
                .snapshot()
                .selected
                .as_ref()
                .map(|value| value.publication.identity)
                == Some(seeded.selected_view),
        "candidate and selected publication identities were conflated",
    )?;
    require(
        work.usage().writes == 0 && work.usage().mutations == 0,
        "candidate inspection spent a write credit",
    )?;
    require(
        matches!(
            verify_native_boot_current_candidate(
                &fixture.root,
                fixture.vfs.as_ref(),
                seeded.other,
                &mut work,
            ),
            Err(IndexError::Conflict)
        ),
        "current candidate with pending work was allowed to proceed to BOOT",
    )?;
    require(
        files(&fixture.root)? == before,
        "candidate read mutated Catalog bytes",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn candidate_refuses_pending_refused_retired_and_unfunded_reads() -> TestResult {
    let mut seeded = None;
    let fixture = fixture_with_boot_input(|root| {
        seeded = Some(seed(root, true)?);
        Ok(16 * 1024 * 1024)
    })?;
    let seeded = seeded.ok_or("candidate fixture missing seed")?;
    let before = files(&fixture.root)?;
    for operation in [seeded.pending, seeded.refused] {
        require(
            matches!(
                verify_native_boot_candidate(
                    &fixture.root,
                    fixture.vfs.as_ref(),
                    operation,
                    &mut read_budget()?
                ),
                Err(IndexError::Conflict)
            ),
            "unpublished operation was accepted as a candidate",
        )?;
    }
    require(
        matches!(
            verify_native_boot_candidate(
                &fixture.root,
                fixture.vfs.as_ref(),
                seeded.other,
                &mut read_budget()?
            ),
            Err(IndexError::SnapshotUnavailable)
        ),
        "retired unselected publication was reconstructed",
    )?;
    let selected = verify_native_boot_candidate(
        &fixture.root,
        fixture.vfs.as_ref(),
        seeded.selected,
        &mut read_budget()?,
    )?;
    require(
        selected.manifest().publication.identity == seeded.selected_view,
        "retired selected publication lost its persisted binding",
    )?;
    let mut denied = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("test deadline overflow")?,
    );
    require(
        matches!(
            verify_native_boot_candidate(
                &fixture.root,
                fixture.vfs.as_ref(),
                seeded.selected,
                &mut denied
            ),
            Err(IndexError::ResourceLimited)
        ),
        "unfunded candidate was accepted",
    )?;
    require(
        files(&fixture.root)? == before,
        "refused read mutated Catalog bytes",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

/// Original FULL, current Native, and later BOOT are three real rows. No
/// pending work, no second verifier connection and no synthetic Native root.
fn seed_current_native_pair(
    root: &Path,
) -> Result<(CatalogView, CatalogView, CatalogView), Box<dyn std::error::Error>> {
    let catalog = SqliteCatalog::open(root, Duration::from_secs(1))?;
    let profile = object(30);
    let genesis = view(0, profile.reference)?;
    let stage = catalog.begin_bootstrap(
        genesis.identity.workspace,
        genesis.identity.epoch,
        &Identity(profile.clone()),
        &mut budget()?,
    )?;
    catalog.reserve_bootstrap(stage, &profile)?;
    catalog.confirm_bootstrap(stage, &profile)?;
    require(
        catalog.publish_bootstrap(stage, genesis.clone(), &mut budget()?)? == genesis.identity,
        "paired Catalog fixture lost its genesis",
    )?;
    let full_attempt = admit(&catalog, 31)?;
    let full = view(1, profile.reference)?;
    require(
        publish(&catalog, full_attempt, &genesis, full.clone())?
            == OperationStatus::Published(full.identity),
        "paired Catalog fixture lost FULL",
    )?;
    let current_attempt = admit(&catalog, 32)?;
    let current = view(2, profile.reference)?;
    require(
        publish(&catalog, current_attempt, &full, current.clone())?
            == OperationStatus::Published(current.identity),
        "paired Catalog fixture lost current Native",
    )?;
    let boot_attempt = admit(&catalog, 35)?;
    let boot = view(3, profile.reference)?;
    require(
        publish(&catalog, boot_attempt, &current, boot.clone())?
            == OperationStatus::Published(boot.identity),
        "paired Catalog fixture lost selected BOOT",
    )?;
    let selected = catalog.select_native_boot_manifest(key(35), &mut budget()?)?;
    require(
        selected.publication.identity == boot.identity,
        "paired Catalog fixture did not select later BOOT",
    )?;
    let refused_attempt = admit(&catalog, 34)?;
    require(
        catalog.refuse(refused_attempt, Refusal::Conflict)?
            == OperationStatus::Refused(Refusal::Conflict),
        "paired Catalog fixture lost a terminal non-Published row",
    )?;
    drop(catalog);
    let directory = catalog_dir(root);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    for entry in fs::read_dir(directory)? {
        fs::set_permissions(entry?.path(), fs::Permissions::from_mode(0o600))?;
    }
    Ok((full, current, boot))
}

#[test]
fn published_visitor_reads_predecessors_from_its_own_snapshot() -> TestResult {
    let mut seeded = None;
    let fixture = fixture_with_boot_input(|root| {
        seeded = Some(seed_current_native_pair(root)?);
        Ok(16 * 1024 * 1024)
    })?;
    let (full, current, boot) = seeded.ok_or("paired Catalog fixture absent")?;
    let before = files(&fixture.root)?;
    let mut visited = Vec::new();
    let mut work = read_budget()?;
    const MEMORY_LIMIT: u64 = 64 * 1024 * 1024;
    let meter = Arc::new(PublicationReadMeter {
        live: AtomicU64::new(0),
        limit: AtomicU64::new(MEMORY_LIMIT),
    });
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(PublicationReadScope(
        Arc::clone(&meter),
    )))));
    let mut point_reads = None;
    let mut reclaimed_reads = None;
    let result = verify_native_source_root_publications_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        key(35).workspace,
        &mut work,
        |snapshot, reader, head, operation, publication, budget| {
            assert_eq!(
                snapshot
                    .selected
                    .as_ref()
                    .map(|selected| selected.operation.key),
                Some(key(35))
            );
            assert_eq!(head.identity, boot.identity);
            assert_eq!(
                reader.published_operation(operation.key, budget)?,
                *publication
            );
            assert_eq!(
                reader.published_view(publication.identity, budget)?,
                *publication
            );
            assert_eq!(reader.published_operation(key(31), budget)?, full);
            assert_eq!(reader.published_view(full.identity, budget)?, full);
            assert_eq!(reader.published_operation(key(32), budget)?, current);
            let before_lookup = budget.usage().reads;
            assert_eq!(reader.published_view(current.identity, budget)?, current);
            if point_reads.is_none() {
                point_reads = Some(budget.usage().reads - before_lookup);
            }
            let mut forged = current.identity;
            forged.digest = Digest([99; 32]);
            assert!(matches!(
                reader.published_view(forged, budget),
                Err(IndexError::SnapshotUnavailable)
            ));
            let mut foreign = current.identity;
            foreign.workspace.tenant = TenantId([99; 16]);
            assert!(matches!(
                reader.published_view(foreign, budget),
                Err(IndexError::SnapshotUnavailable)
            ));
            assert!(matches!(
                reader.published_view(
                    ViewRef {
                        sequence: 99,
                        ..current.identity
                    },
                    budget
                ),
                Err(IndexError::SnapshotUnavailable)
            ));
            assert!(matches!(
                reader.published_operation(key(99), budget),
                Err(IndexError::SnapshotUnavailable)
            ));
            if reclaimed_reads.is_none() {
                // A mandatory allocation cannot fit until the optional
                // index is released. The next lookup must still work.
                let live = meter.live.load(Ordering::SeqCst);
                meter.limit.store(live, Ordering::SeqCst);
                let required = budget.reserve_memory(1, MemoryLifetime::Temporary);
                meter.limit.store(MEMORY_LIMIT, Ordering::SeqCst);
                let required = required?.ok_or(IndexError::ResourceLimited)?;
                assert!(meter.live.load(Ordering::SeqCst) < live);
                drop(required);
                let reads = budget.usage().reads;
                assert_eq!(reader.published_view(current.identity, budget)?, current);
                reclaimed_reads = Some(budget.usage().reads - reads);
            }
            visited.push((operation.key, publication.identity));
            Ok(())
        },
    )?;
    assert_eq!(result.current_head.identity, boot.identity);
    require(
        point_reads.is_some_and(|reads| reads <= 4),
        "funded publication view lookup rescanned the workspace",
    )?;
    require(
        reclaimed_reads.is_some_and(|reads| reads > 4),
        "evicted publication view lookup did not use the canonical scan",
    )?;
    assert_eq!(
        visited,
        vec![
            (key(31), full.identity),
            (key(32), current.identity),
            (key(35), boot.identity),
        ]
    );
    require(
        work.usage().writes == 0 && work.usage().mutations == 0,
        "publication visitor spent mutation credit",
    )?;
    drop(result);
    require(
        meter.live.load(Ordering::SeqCst) == 0,
        "funded publication reader retained its index loan",
    )?;
    let mut fallback_work = read_budget()?;
    let mut fallback_seen = Vec::new();
    let fallback = verify_native_source_root_publications_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        key(35).workspace,
        &mut fallback_work,
        |_snapshot, reader, _head, operation, publication, budget| {
            assert_eq!(reader.published_view(full.identity, budget)?, full);
            assert_eq!(reader.published_view(current.identity, budget)?, current);
            assert_eq!(reader.published_view(boot.identity, budget)?, boot);
            let mut forged = current.identity;
            forged.digest = Digest([99; 32]);
            assert!(matches!(
                reader.published_view(forged, budget),
                Err(IndexError::SnapshotUnavailable)
            ));
            assert!(matches!(
                reader.published_operation(key(99), budget),
                Err(IndexError::SnapshotUnavailable)
            ));
            fallback_seen.push((operation.key, publication.identity));
            Ok(())
        },
    )?;
    assert_eq!(fallback.current_head.identity, boot.identity);
    assert_eq!(fallback_seen, visited);
    require(
        fallback_work.usage().writes == 0 && fallback_work.usage().mutations == 0,
        "unscoped publication reader spent mutation credit",
    )?;
    require(
        files(&fixture.root)? == before,
        "publication visitor changed Catalog bytes",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn published_full_and_current_native_share_selected_snapshot_without_aliasing() -> TestResult {
    let mut seeded = None;
    let fixture = fixture_with_boot_input(|root| {
        seeded = Some(seed_current_native_pair(root)?);
        Ok(16 * 1024 * 1024)
    })?;
    let (full, current, boot) = seeded.ok_or("paired Catalog fixture absent")?;
    let before = files(&fixture.root)?;
    let candidate = verify_native_boot_current_candidate(
        &fixture.root,
        fixture.vfs.as_ref(),
        key(35),
        &mut read_budget()?,
    )?;
    require(
        candidate.manifest().publication.identity == boot.identity,
        "quiescent current candidate lost its BOOT identity",
    )?;
    drop(candidate);
    let mut work = read_budget()?;
    let actual = verify_native_rollback_with_source_publications(
        &fixture.root,
        fixture.vfs.as_ref(),
        key(31),
        key(32),
        &mut work,
    )?;
    require(
        actual
            .snapshot()
            .selected
            .as_ref()
            .map(|selected| selected.operation.key)
            == Some(key(35))
            && actual
                .snapshot()
                .selected
                .as_ref()
                .map(|selected| selected.publication.identity)
                == Some(boot.identity),
        "selected BOOT differs from its own later publication",
    )?;
    require(
        actual
            .published_for(key(31))
            .is_some_and(|(operation, publication)| {
                operation.key == key(31) && publication.identity == full.identity
            })
            && actual
                .published_for(key(32))
                .is_some_and(|(operation, publication)| {
                    operation.key == key(32) && publication.identity == current.identity
                })
            && actual.published_for(key(35)).is_none(),
        "FULL/current Native Published rows were conflated or invented",
    )?;
    require(
        work.usage().writes == 0 && work.usage().mutations == 0,
        "paired read spent mutation credit",
    )?;
    drop(actual);
    let mut deduplicated = read_budget()?;
    let same = verify_native_rollback_with_source_publications(
        &fixture.root,
        fixture.vfs.as_ref(),
        key(32),
        key(32),
        &mut deduplicated,
    )?;
    require(
        same.published_for(key(32))
            .is_some_and(|(_, publication)| publication.identity == current.identity)
            && same.published_for(key(31)).is_none()
            && deduplicated.usage().reads < work.usage().reads,
        "equal-key current Native caused another row read or historical alias",
    )?;
    drop(same);
    require(
        files(&fixture.root)? == before,
        "paired Catalog read changed source bytes",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn legacy_history_recovery_observes_every_intermediate_publication() -> TestResult {
    use quanta_index_catalog::verify_native_legacy_history_recovery_publications_v1;

    let mut seeded = None;
    let fixture = fixture_with_boot_input(|root| {
        seeded = Some(seed_current_native_pair(root)?);
        Ok(16 * 1024 * 1024)
    })?;
    let (_, intermediate, _) = seeded.ok_or("history recovery fixture absent")?;
    let before = files(&fixture.root)?;

    let observed = verify_native_legacy_history_recovery_publications_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        key(31),
        key(31),
        &mut read_budget()?,
    )?;
    let [step] = observed.snapshot().history_predecessors() else {
        return Err("expected one intermediate publication".into());
    };
    require(
        step.operation.key == key(32) && step.publication == intermediate,
        "wrong operation observed between anchor and selected BOOT",
    )?;
    drop(observed);

    let direct = verify_native_legacy_history_recovery_publications_v1(
        &fixture.root,
        fixture.vfs.as_ref(),
        key(31),
        key(32),
        &mut read_budget()?,
    )?;
    require(
        direct.snapshot().history_predecessors().is_empty(),
        "one transition incorrectly required intermediate publications",
    )?;
    drop(direct);

    require(
        matches!(
            verify_native_legacy_history_recovery_publications_v1(
                &fixture.root,
                fixture.vfs.as_ref(),
                key(31),
                key(35),
                &mut read_budget()?,
            ),
            Err(IndexError::DigestConflict)
        ),
        "the selected BOOT was accepted as its own anchor",
    )?;
    require(
        files(&fixture.root)? == before,
        "history boot read wrote bytes",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}

#[test]
fn published_current_native_refuses_missing_foreign_cancelled_and_unfunded_rows() -> TestResult {
    let fixture = fixture_with_boot_input(|root| {
        let _seed = seed_current_native_pair(root)?;
        Ok(16 * 1024 * 1024)
    })?;
    let before = files(&fixture.root)?;
    require(
        matches!(
            verify_native_rollback_with_source_publications(
                &fixture.root,
                fixture.vfs.as_ref(),
                key(31),
                key(33),
                &mut read_budget()?
            ),
            Err(IndexError::SnapshotUnavailable)
        ),
        "unpublished current Native row was invented",
    )?;
    require(
        matches!(
            verify_native_rollback_with_source_publications(
                &fixture.root,
                fixture.vfs.as_ref(),
                key(31),
                key(34),
                &mut read_budget()?
            ),
            Err(IndexError::Conflict)
        ),
        "terminal refused row was accepted as Published current Native",
    )?;
    let mut foreign = key(32);
    foreign.workspace.tenant = TenantId([99; 16]);
    require(
        matches!(
            verify_native_rollback_with_source_publications(
                &fixture.root,
                fixture.vfs.as_ref(),
                key(31),
                foreign,
                &mut read_budget()?
            ),
            Err(IndexError::DigestConflict)
        ),
        "foreign workspace was read as current Native",
    )?;
    let mut cancelled = read_budget()?;
    cancelled.request().cancel_handle().cancel();
    require(
        matches!(
            verify_native_rollback_with_source_publications(
                &fixture.root,
                fixture.vfs.as_ref(),
                key(31),
                key(32),
                &mut cancelled
            ),
            Err(IndexError::Cancelled)
        ),
        "cancelled current Native read was admitted",
    )?;
    let mut denied = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("test deadline overflow")?,
    );
    require(
        matches!(
            verify_native_rollback_with_source_publications(
                &fixture.root,
                fixture.vfs.as_ref(),
                key(31),
                key(32),
                &mut denied
            ),
            Err(IndexError::ResourceLimited)
        ),
        "unfunded current Native read was admitted",
    )?;
    require(
        files(&fixture.root)? == before,
        "refused Catalog read changed source bytes",
    )?;
    fixture.vfs.drain_retirements()?;
    Ok(())
}
