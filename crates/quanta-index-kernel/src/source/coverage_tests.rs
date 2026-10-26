//! Same-attempt validation custody and snapshot corruption boundaries.

use super::*;
use quanta_index_core::index::{
    AttemptId, CollectionId, Digest, MapEdit, OwnerMutationDigest, OwnerMutationSet,
    RegisteredObject, SlotId, StreamId, TenantId, WorkspaceKey, WorktreeId, ports::ObjectWriteSink,
};
use quanta_index_storage::FileObjectStore;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn attempt() -> AttemptRef {
    AttemptRef {
        operation: OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence: 1,
        },
        attempt: AttemptId([7; 16]),
        fence: 1,
    }
}

#[test]
fn coverage_source_proof_rejects_foreign_attempt_or_cut() -> TestResult {
    let home = tempfile::tempdir()?;
    let store = FileObjectStore::open(home.path().join("objects"))?;
    let source = SourceCut {
        owners: MapRoot(None),
        paths: MapRoot(None),
        frontiers: MapRoot(None),
    };
    let applied = AppliedSource {
        delta: SourceDelta {
            prior: source,
            next: source,
            changed: OwnerMutationSet {
                root: MapRoot(None),
                count: 0,
                digest: OwnerMutationDigest::default().finish(),
            },
        },
        attempt: attempt(),
    };
    let mut foreign = attempt();
    foreign.fence = 2;
    let other_source = SourceCut {
        owners: MapRoot(Some(ObjectRef(Digest([8; 32])))),
        ..source
    };
    for (selected, attempt) in [(source, foreign), (other_source, attempt())] {
        if !matches!(
            CoverageChanges::new(selected, Some(&applied), attempt, &store, &store),
            Err(IndexError::BackendViolation(_))
        ) {
            return Err("foreign attempt/cut reused source validation".into());
        }
    }
    let (mut work, _ledger) = crate::path_staging::tests::budget(u64::MAX)?;
    let mut changes = CoverageChanges::new(source, Some(&applied), attempt(), &store, &store)?;
    if changes.next_mutation(&mut work)?.is_some() {
        return Err("empty validated source produced coverage work".into());
    }
    Ok(())
}

struct Sink;
impl ObjectWriteSink for Sink {
    fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        Ok(())
    }
    fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        Ok(())
    }
    fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
    fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        Ok(())
    }
}

#[test]
fn coverage_snapshot_still_rejects_corrupt_records_and_latches_failure() -> TestResult {
    let home = tempfile::tempdir()?;
    let store = FileObjectStore::open(home.path().join("objects"))?;
    let (mut work, _ledger) = crate::path_staging::tests::budget(u64::MAX)?;
    let corrupt = store.put(b"not a source record", &[], &mut work)?.reference;
    let owner = OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: quanta_index_core::index::StableOwnerId([9; 16]),
    };
    let owners = store.apply(
        MapRoot(None),
        &[MapEdit {
            key: owner_map_key(owner).to_vec(),
            expected: None,
            next: Some(corrupt.into()),
        }],
        &Sink,
        &mut work,
    )?;
    let source = SourceCut {
        owners: owners.root,
        paths: MapRoot(None),
        frontiers: MapRoot(None),
    };
    let mut changes = CoverageChanges::new(source, None, attempt(), &store, &store)?;
    let Err(error) = changes.next_mutation(&mut work) else {
        return Err("snapshot coverage accepted corrupt source metadata".into());
    };
    let (mut fresh, _fresh_ledger) = crate::path_staging::tests::budget(u64::MAX)?;
    if changes.next_mutation(&mut fresh) != Err(error) {
        return Err("snapshot coverage failure was not sticky".into());
    }
    Ok(())
}
