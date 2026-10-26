//! Bounded source intake against the real object store and ingress catalog.
use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    CollectionId, Digest, IndexError, ObjectRef, OperationKey, ProducerId, SlotId, SourceInput,
    SourceRecord, SourceTextAdmission, StreamId, TenantId, WorkBudget, WorkLimits, WorkspaceKey,
    WorktreeId, ports::ImmutableObjectPort,
};
use quanta_index_kernel::{AdapterRegistry, Kernel, KernelServices};
use quanta_index_storage::FileObjectStore;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

macro_rules! check {
    ($condition:expr) => {{
        if $condition {
            Ok::<(), Box<dyn Error>>(())
        } else {
            Err(format!("source batch check failed: {}", stringify!($condition)).into())
        }
    }};
}

macro_rules! check_eq {
    ($actual:expr, $expected:expr) => {{
        let actual = &$actual;
        let expected = &$expected;
        if actual == expected {
            Ok::<(), Box<dyn Error>>(())
        } else {
            Err(format!(
                "source batch {} == {}: actual={actual:?}, expected={expected:?}",
                stringify!($actual),
                stringify!($expected)
            )
            .into())
        }
    }};
}

macro_rules! check_ne {
    ($actual:expr, $expected:expr) => {{
        let actual = &$actual;
        let expected = &$expected;
        if actual != expected {
            Ok::<(), Box<dyn Error>>(())
        } else {
            Err(format!(
                "source batch {} != {}: both={actual:?}",
                stringify!($actual),
                stringify!($expected)
            )
            .into())
        }
    }};
}

fn budget(bytes: u64) -> TestResult<WorkBudget> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("source batch test deadline overflow")?,
    ))
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
            .get(reference, &mut budget(16 * 1024 * 1024)?)?
            .payload,
    )?)
}

struct DenyMemory;

impl quanta_index_core::index::MemoryScopePort for DenyMemory {
    fn reserve(
        &self,
        _bytes: u64,
        _lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        Err(IndexError::ResourceLimited)
    }
}

#[derive(Default)]
struct LiveMemory {
    bytes: AtomicU64,
    peak: AtomicU64,
    invalid_refund: AtomicBool,
}

impl quanta_index_core::index::MemoryLeaseOwner for LiveMemory {
    fn release(&self, bytes: u64) {
        if self
            .bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                live.checked_sub(bytes)
            })
            .is_err()
        {
            self.invalid_refund.store(true, Ordering::Relaxed);
        }
    }
}

struct SourceMemory(Arc<LiveMemory>);

struct DenyRetainedMemory(Arc<LiveMemory>);

impl quanta_index_core::index::MemoryScopePort for DenyRetainedMemory {
    fn reserve(
        &self,
        bytes: u64,
        lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        if lifetime == quanta_index_core::index::MemoryLifetime::Retained {
            return Err(IndexError::ResourceLimited);
        }
        quanta_index_core::index::MemoryScopePort::reserve(
            &SourceMemory(self.0.clone()),
            bytes,
            lifetime,
        )
    }
}

impl quanta_index_core::index::MemoryScopePort for SourceMemory {
    fn reserve(
        &self,
        bytes: u64,
        _lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        let prior = self
            .0
            .bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                live.checked_add(bytes)
            })
            .map_err(|_live| IndexError::ResourceLimited)?;
        let _previous_peak = self.0.peak.fetch_max(
            prior
                .checked_add(bytes)
                .ok_or(IndexError::ResourceLimited)?,
            Ordering::Relaxed,
        );
        Ok(quanta_index_core::index::MemoryLease::new(
            bytes,
            self.0.clone(),
        ))
    }
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
        KernelServices::new(
            objects.clone(),
            objects.clone(),
            catalog,
            Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
                directory.path().join("scratch"),
                16 * 1024 * 1024 * 1024,
            )?),
            Duration::from_secs(30),
        )?,
        AdapterRegistry::new(),
    );
    let _genesis = kernel.create_workspace(workspace(), 1, &mut budget(16 * 1024 * 1024)?)?;
    let ingress = kernel.begin_input(operation())?;
    let inputs = vec![
        SourceInput {
            path: "a.rs".into(),
            bytes: b"shared source".to_vec(),
            text: quanta_index_core::index::SourceTextAdmission::Excluded,
        },
        SourceInput {
            path: "b.rs".into(),
            bytes: b"shared source".to_vec(),
            text: quanta_index_core::index::SourceTextAdmission::Excluded,
        },
        SourceInput {
            path: "a.rs".into(),
            bytes: b"shared source".to_vec(),
            text: quanta_index_core::index::SourceTextAdmission::Excluded,
        },
    ];
    let denied = SourceInput {
        path: "memory-denied.rs".into(),
        bytes: b"never-installed-without-source-preparation-admission".to_vec(),
        text: SourceTextAdmission::Excluded,
    };
    let denied_ref = objects
        .identify(&denied.bytes, &[], &mut budget(16 * 1024 * 1024)?)?
        .reference;
    for scalar in [false, true] {
        let mut denied_budget = budget(16 * 1024 * 1024)?;
        denied_budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(
            Arc::new(DenyMemory),
        )));
        if scalar {
            check!(matches!(
                kernel.stage_source(ingress, &denied.path, &denied.bytes, &mut denied_budget),
                Err(IndexError::ResourceLimited)
            ))?;
        } else {
            check!(matches!(
                kernel.stage_sources(ingress, std::slice::from_ref(&denied), &mut denied_budget),
                Err(IndexError::ResourceLimited)
            ))?;
        }
        check!(matches!(
            objects.get(denied_ref, &mut budget(16 * 1024 * 1024)?),
            Err(IndexError::CorruptObject(_))
        ))?;
    }

    let future_memory = Arc::new(LiveMemory::default());
    let mut future_budget = budget(16 * 1024 * 1024)?;
    future_budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        DenyRetainedMemory(future_memory.clone()),
    ))));
    check!(matches!(
        kernel.stage_sources(ingress, std::slice::from_ref(&denied), &mut future_budget),
        Err(IndexError::ResourceLimited)
    ))?;
    check!(future_memory.peak.load(Ordering::Relaxed) > 0)?;
    check_eq!(future_memory.bytes.load(Ordering::Relaxed), 0)?;
    check!(!future_memory.invalid_refund.load(Ordering::Relaxed))?;
    check!(matches!(
        objects.get(denied_ref, &mut budget(16 * 1024 * 1024)?),
        Err(IndexError::CorruptObject(_))
    ))?;

    let empty = SourceInput {
        path: "empty.rs".into(),
        bytes: Vec::new(),
        text: SourceTextAdmission::Utf8 {
            policy: Digest([30; 32]),
        },
    };
    let empty_memory = Arc::new(LiveMemory::default());
    let mut empty_budget = budget(16 * 1024 * 1024)?;
    empty_budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        SourceMemory(empty_memory.clone()),
    ))));
    let empty_refs =
        kernel.stage_sources(ingress, std::slice::from_ref(&empty), &mut empty_budget)?;
    let empty_ref = *empty_refs.first().ok_or("empty source reference missing")?;
    check_eq!(
        kernel.stage_source_text(
            ingress,
            &empty.path,
            &empty.bytes,
            empty.text,
            &mut empty_budget
        )?,
        empty_ref
    )?;
    check_eq!(
        empty_memory.bytes.load(Ordering::Relaxed),
        u64::try_from(std::mem::size_of::<ObjectRef>())?
    )?;
    let mut empty_iter = empty_refs.into_iter();
    check_eq!(empty_iter.next(), Some(empty_ref))?;
    check!(empty_iter.next().is_none())?;
    // Consuming values does not free the owning iterator's backing.
    check!(empty_memory.bytes.load(Ordering::Relaxed) > 0)?;
    drop(empty_iter);
    check_eq!(empty_memory.bytes.load(Ordering::Relaxed), 0)?;
    check!(empty_memory.peak.load(Ordering::Relaxed) > 0)?;
    check!(!empty_memory.invalid_refund.load(Ordering::Relaxed))?;
    let empty_record = read_record(&objects, empty_ref)?;
    check_eq!(empty_record.byte_len, 0)?;
    check_eq!(empty_record.text, empty.text)?;
    check!(
        objects
            .get(empty_record.bytes, &mut budget(16 * 1024 * 1024)?)?
            .payload
            .is_empty()
    )?;

    let references = kernel.stage_sources(ingress, &inputs, &mut budget(64 * 1024 * 1024)?)?;
    check_eq!(references.len(), inputs.len())?;
    let first_ref = *references.first().ok_or("missing first staged source")?;
    let second_ref = *references.get(1).ok_or("missing second staged source")?;
    let third_ref = *references.get(2).ok_or("missing third staged source")?;
    check_ne!(first_ref, second_ref)?;
    check_eq!(first_ref, third_ref)?;
    let first = read_record(&objects, first_ref)?;
    let second = read_record(&objects, second_ref)?;
    check_eq!(first.path, "a.rs")?;
    check_eq!(second.path, "b.rs")?;
    check_eq!(first.bytes, second.bytes)?;
    check_eq!(first.text, SourceTextAdmission::Excluded)?;
    let body = objects.get(first.bytes, &mut budget(16 * 1024 * 1024)?)?;
    check_eq!(body.payload.as_slice(), b"shared source")?;
    for (input, reference) in inputs.iter().zip(references.iter()) {
        check_eq!(
            kernel.stage_source(
                ingress,
                &input.path,
                &input.bytes,
                &mut budget(16 * 1024 * 1024)?,
            )?,
            *reference
        )?;
    }

    let absent = b"preflight-invalid-unique";
    let absent_ref = objects
        .identify(absent, &[], &mut budget(16 * 1024 * 1024)?)?
        .reference;
    let invalid = [
        SourceInput {
            path: "valid.rs".into(),
            bytes: absent.to_vec(),
            text: quanta_index_core::index::SourceTextAdmission::Excluded,
        },
        SourceInput {
            path: "../escape.rs".into(),
            bytes: b"bad".to_vec(),
            text: quanta_index_core::index::SourceTextAdmission::Excluded,
        },
    ];
    check!(matches!(
        kernel.stage_sources(ingress, &invalid, &mut budget(16 * 1024 * 1024)?),
        Err(IndexError::InvalidInput(_))
    ))?;
    check!(matches!(
        objects.get(absent_ref, &mut budget(16 * 1024 * 1024)?),
        Err(IndexError::CorruptObject(_))
    ))?;

    let admitted_policy = Digest([24; 32]);
    let admitted = SourceInput {
        path: "admitted.rs".into(),
        bytes: "caf\u{e9}".as_bytes().to_vec(),
        text: SourceTextAdmission::Utf8 {
            policy: admitted_policy,
        },
    };
    let admitted_refs = kernel.stage_sources(
        ingress,
        std::slice::from_ref(&admitted),
        &mut budget(16 * 1024 * 1024)?,
    )?;
    check_eq!(admitted_refs.len(), 1)?;
    let admitted_ref = *admitted_refs.first().ok_or("missing admitted source")?;
    check_eq!(read_record(&objects, admitted_ref)?.text, admitted.text)?;

    let unique_body = b"admitted-preflight-unique";
    let unique_ref = objects
        .identify(unique_body, &[], &mut budget(16 * 1024 * 1024)?)?
        .reference;
    let mixed = [
        SourceInput {
            path: "first.rs".into(),
            bytes: unique_body.to_vec(),
            text: SourceTextAdmission::Excluded,
        },
        SourceInput {
            path: "invalid.rs".into(),
            bytes: vec![0xff],
            text: SourceTextAdmission::Utf8 {
                policy: admitted_policy,
            },
        },
    ];
    check!(matches!(
        kernel.stage_sources(ingress, &mixed, &mut budget(16 * 1024 * 1024)?),
        Err(IndexError::InvalidInput(_))
    ))?;
    check!(matches!(
        objects.get(unique_ref, &mut budget(16 * 1024 * 1024)?),
        Err(IndexError::CorruptObject(_))
    ))?;
    check!(matches!(
        kernel.stage_source_text(
            ingress,
            "invalid-single.rs",
            &[0xff],
            admitted.text,
            &mut budget(16 * 1024 * 1024)?,
        ),
        Err(IndexError::InvalidInput(_))
    ))?;
    check!(matches!(
        kernel.stage_sources(ingress, &invalid[..1], &mut budget(0)?),
        Err(IndexError::ResourceLimited)
    ))?;
    check!(matches!(
        objects.get(absent_ref, &mut budget(16 * 1024 * 1024)?),
        Err(IndexError::CorruptObject(_))
    ))?;
    Ok(())
}
