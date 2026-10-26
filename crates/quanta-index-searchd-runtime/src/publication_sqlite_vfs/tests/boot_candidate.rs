//! Candidate selection is an offline read of an existing Published receipt.

use super::*;
use quanta_index_catalog::{catalog_dir, verify_native_boot_candidate};
use quanta_index_core::index::ports::{
    CatalogExactHistoryPort as _, CatalogGenesisPort as _, CatalogPublicationPort as _,
    ImmutableObjectPort, InputStagingPort as _, OperationJournalPort as _, WorkActionCursor,
};
use quanta_index_core::index::{
    AdmissionOutcome, AttemptRef, CatalogView, CollectionId, Digest, ImmutableOperation,
    IndexError, IndexProfile, MapRoot, ObjectBatchInput, ObjectRef, OperationKey, OperationStatus,
    PreparedPublication, ProducerId, Refusal, RegisteredObject, RegisteredObjectBatch, SlotId,
    SourceCut, SourceOperationMode, StoredObject, StreamId, TenantId, ViewRef, WorkAction,
    WorkActionSet, WorkBudget, WorkLimits, WorkspaceKey, WorktreeId,
};
use std::{os::unix::fs::MetadataExt as _, time::Instant};

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
