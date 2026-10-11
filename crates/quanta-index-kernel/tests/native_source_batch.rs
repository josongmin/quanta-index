//! Bounded source intake against the real object store and ingress catalog.
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    CollectionId, IndexError, ObjectRef, OperationKey, ProducerId, SlotId, SourceInput,
    SourceRecord, StreamId, TenantId, WorkBudget, WorkLimits, WorkspaceKey, WorktreeId,
    ports::ImmutableObjectPort,
};
use quanta_index_kernel::{AdapterRegistry, Kernel, KernelServices};
use quanta_index_storage::FileObjectStore;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn budget(bytes: u64) -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes,
            mutations: 100_000,
        },
        Instant::now() + Duration::from_secs(30),
    )
}

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn operation() -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence: 1,
    }
}

fn read_record(objects: &FileObjectStore, reference: ObjectRef) -> TestResult<SourceRecord> {
    Ok(serde_json::from_slice(
        &objects
            .get(reference, &mut budget(16 * 1024 * 1024))?
            .payload,
    )?)
}

#[test]
fn batch_preserves_single_identity_order_and_deduplicates_content() -> TestResult {
    let directory = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(directory.path().join("objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(
        directory.path(),
        Duration::from_millis(100),
    )?);
    let kernel = Kernel::new(
        KernelServices::new(objects.clone(), objects.clone(), catalog),
        AdapterRegistry::new(),
    );
    let _genesis = kernel.create_workspace(workspace(), 1, &mut budget(16 * 1024 * 1024))?;
    let ingress = kernel.begin_input(operation())?;
    let inputs = vec![
        SourceInput {
            path: "a.rs".into(),
            bytes: b"shared source".to_vec(),
        },
        SourceInput {
            path: "b.rs".into(),
            bytes: b"shared source".to_vec(),
        },
        SourceInput {
            path: "a.rs".into(),
            bytes: b"shared source".to_vec(),
        },
    ];
    let references = kernel.stage_sources(ingress, &inputs, &mut budget(64 * 1024 * 1024))?;
    assert_eq!(references.len(), inputs.len());
    assert_ne!(references[0], references[1]);
    assert_eq!(references[0], references[2]);
    let first = read_record(&objects, references[0])?;
    let second = read_record(&objects, references[1])?;
    assert_eq!(first.path, "a.rs");
    assert_eq!(second.path, "b.rs");
    assert_eq!(first.bytes, second.bytes);
    let body = objects.get(first.bytes, &mut budget(16 * 1024 * 1024))?;
    assert_eq!(body.payload.as_slice(), b"shared source");
    for (input, reference) in inputs.iter().zip(references.iter()) {
        assert_eq!(
            kernel.stage_source(
                ingress,
                &input.path,
                &input.bytes,
                &mut budget(16 * 1024 * 1024),
            )?,
            *reference
        );
    }

    let absent = b"preflight-invalid-unique";
    let absent_ref = objects
        .identify(absent, &[], &mut budget(16 * 1024 * 1024))?
        .reference;
    let invalid = [
        SourceInput {
            path: "valid.rs".into(),
            bytes: absent.to_vec(),
        },
        SourceInput {
            path: "../escape.rs".into(),
            bytes: b"bad".to_vec(),
        },
    ];
    assert!(matches!(
        kernel.stage_sources(ingress, &invalid, &mut budget(16 * 1024 * 1024)),
        Err(IndexError::InvalidInput(_))
    ));
    assert!(matches!(
        objects.get(absent_ref, &mut budget(16 * 1024 * 1024)),
        Err(IndexError::CorruptObject(_))
    ));
    assert_eq!(
        kernel.stage_sources(ingress, &invalid[..1], &mut budget(0)),
        Err(IndexError::ResourceLimited)
    );
    assert!(matches!(
        objects.get(absent_ref, &mut budget(16 * 1024 * 1024)),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}
