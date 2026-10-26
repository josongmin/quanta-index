//! Native end-to-end application tests: real catalog, COW storage and literal
//! adapter. These do not qualify daemon transport or large-corpus latency.
#![expect(
    clippy::disallowed_methods,
    reason = "test-only synchronous driver executes the Kernel future; production runtime has its own async boundary"
)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration assertions are independent behavior oracles inside Result-returning test functions"
)]
#[path = "common/native_catalog_profile.rs"]
mod native_catalog_profile;
#[path = "native_lifecycle/searchable_preparation.rs"]
mod searchable_preparation;
use futures::executor::block_on;
use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    AdmissionOutcome, CollectionId, Digest, GroupActivation, ImmutableOperation, IndexError,
    IndexInstanceId, IndexKind, IndexProfile, IndexQueryPort, IngressRef, InstanceSpec,
    MemberBinding, ObjectRef, OperationKey, OperationStatus, OwnedReadyOutcome, OwnedWorkBegin,
    OwnedWorkDefer, OwnedWorkGrant, OwnedWorkTerminal, OwnerKey, OwnerMutation, OwnerMutationSet,
    ProducerId, PublicationGroupId, QueryReadContext, Refusal, SchemaId, SlotId, SourceCut,
    SourceEvent, SourceInput, SourceRecord, SourceTransition, StableOwnerId, StorageFormatId,
    StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection,
    WorkBudget, WorkLimits, WorkspaceKey, WorktreeId,
    effort::ResourceNeed,
    owner_map_key,
    ports::{ImmutableObjectPort, InputStagingPort, OperationJournalPort, PersistentMapPort},
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_lexical::native::{
    LiteralQuery, LiteralReadContext, LiteralReadRequest, NativeLiteralIndex, NativeLiteralReader,
    NativeTextHitSource,
};
use quanta_index_repomap::native::{
    FileMapReadContext, FilePathQuery, NativeFileMap, NativePathReader, PathReadRequest,
    query_paths,
};
use quanta_index_storage::FileObjectStore;
use sha2::{Digest as _, Sha256};
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn require_owned_grant(outcome: OwnedWorkBegin, label: &str) -> TestResult<OwnedWorkGrant> {
    match outcome {
        OwnedWorkBegin::Granted(grant) => Ok(grant),
        other @ (OwnedWorkBegin::Deferred(_) | OwnedWorkBegin::Terminal(_)) => {
            Err(format!("{label}: {other:?}").into())
        }
    }
}

fn durable_text_source(source: NativeTextHitSource) -> TestResult<ObjectRef> {
    match source {
        NativeTextHitSource::Durable(reference) => Ok(reference),
        NativeTextHitSource::Overlay { .. } => Err("overlay hit in durable lifecycle test".into()),
    }
}

fn score_within_16_ulps(actual: f64, expected: f64) -> bool {
    actual.is_finite() && actual > 0.0 && actual.to_bits().abs_diff(expected.to_bits()) <= 16
}

#[expect(
    clippy::expect_used,
    reason = "A fixed test deadline that cannot be represented must fail fixture setup"
)]
fn budget() -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .expect("test deadline fits Instant"),
    )
}

#[expect(
    clippy::expect_used,
    reason = "A fixed test deadline that cannot be represented must fail fixture setup"
)]
fn large_fixture_budget() -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads: 20_000_000,
            writes: 20_000_000,
            bytes: 2 * 1024 * 1024 * 1024,
            mutations: 20_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(600))
            .expect("test deadline fits Instant"),
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

fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: StableOwnerId([id; 16]),
    }
}

fn large_owner(id: u32) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: StableOwnerId(u128::from(id).to_be_bytes()),
    }
}

fn spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([7; 16]),
        kind: IndexKind([8; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: quanta_index_core::index::InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([11; 32]),
        format: StorageFormatId([12; 16]),
    }
}

fn file_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([17; 16]),
        kind: IndexKind([18; 16]),
        format: StorageFormatId([19; 16]),
        ..spec()
    }
}

fn kernel(
    root: &std::path::Path,
) -> TestResult<(
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
)> {
    kernel_with_scratch_cap(root, 16 * 1024 * 1024 * 1024)
}

fn kernel_with_scratch_cap(
    root: &std::path::Path,
    scratch_bytes: u64,
) -> TestResult<(
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
)> {
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.join("scratch"),
        scratch_bytes,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.join("native-objects"),
        scratch.clone(),
    )?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        NativeLiteralIndex,
        |delta| Ok(delta.clone()),
    )?;
    registry.register(
        AdapterDescriptor {
            kind: file_spec().kind,
            schema: file_spec().schema,
            format: file_spec().format,
        },
        NativeFileMap,
        |delta| Ok(delta.clone()),
    )?;
    let services = KernelServices::new(
        objects.clone(),
        objects.clone(),
        catalog.clone(),
        scratch,
        Duration::from_secs(30),
    )?;
    Ok((Kernel::new(services, registry), objects, catalog))
}

#[test]
fn small_delta_uses_one_small_shared_scratch_capacity_and_preserves_old_view() -> TestResult {
    // This is a real total-space allowance, shared by source staging, native
    // adapters and COW map construction. It is not a per-allocation allowance.
    const SHARED_BYTES: u64 = 8 * 1024 * 1024;
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel_with_scratch_cap(directory.path(), SHARED_BYTES)?;
    let empty = configured(&engine, &objects)?;
    let first = stage(&objects, "small.rs", b"before unique")?;
    let initial = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            empty,
            source(&engine, empty)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(first),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let pinned = engine.pin_view(ViewSelection::Exact(initial), &budget())?;
    let next = stage(&objects, "small.rs", b"after unique")?;
    let updated = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            initial,
            source(&engine, initial)?,
            3,
            2,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(first),
                next: Some(next),
            }],
        )?,
        &mut budget(),
    ))?)?;
    assert_eq!(
        hits(
            &engine,
            initial,
            &objects,
            &member(&engine, &objects, initial)?,
            b"before"
        )?,
        vec![owner(1)]
    );
    assert!(
        hits(
            &engine,
            updated,
            &objects,
            &member(&engine, &objects, updated)?,
            b"before"
        )?
        .is_empty()
    );
    assert_eq!(
        hits(
            &engine,
            updated,
            &objects,
            &member(&engine, &objects, updated)?,
            b"after"
        )?,
        vec![owner(1)]
    );
    assert_eq!(pinned.view().identity, initial);
    drop(pinned);
    drop(engine);
    drop(objects);
    drop(catalog);
    let (reopened, objects, _catalog) = kernel_with_scratch_cap(directory.path(), SHARED_BYTES)?;
    assert_eq!(
        hits(
            &reopened,
            updated,
            &objects,
            &member(&reopened, &objects, updated)?,
            b"after"
        )?,
        vec![owner(1)]
    );
    Ok(())
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => Err(format!("expected published, got {other:?}").into()),
    }
}

#[test]
fn read_ticket_retains_session_guard_after_kernel_drop() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (kernel, objects, catalog) = kernel(directory.path())?;
    let view = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let mut ticket = kernel.pin_view(ViewSelection::Exact(view), &budget())?;
    drop(kernel);
    drop(objects);
    drop(catalog);
    ticket.validate()?;
    ticket.renew()?;
    assert_eq!(ticket.view().identity, view);
    ticket.release()?;
    Ok(())
}

fn configured(kernel: &Kernel<SqliteCatalog>, _objects: &FileObjectStore) -> TestResult<ViewRef> {
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = kernel.begin_input(operation(1))?;
    let profile = kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![spec(), file_spec()],
        },
        &mut budget(),
    )?;
    published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)
}

#[test]
fn status_and_pin_observe_original_budget_without_losing_terminal_reconciliation() -> TestResult {
    use quanta_index_core::index::ports::OperationInspectAccess;

    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(directory.path())?;
    let published = configured(&engine, &objects)?;
    let prior_pins = native_pin_count(&catalog)?;
    let original = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
    original.cancel_handle().cancel();
    let denied = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        original,
    );
    assert_eq!(
        engine.status(operation(1), OperationInspectAccess::Request(&denied)),
        Err(IndexError::Cancelled),
    );
    assert!(matches!(
        engine.pin_view(ViewSelection::Exact(published), &denied),
        Err(IndexError::Cancelled)
    ));
    assert_eq!(native_pin_count(&catalog)?, prior_pins);
    assert_eq!(
        engine.status(operation(1), OperationInspectAccess::PostFailureReconcile)?,
        OperationStatus::Published(published),
    );
    Ok(())
}

#[test]
fn retained_pin_does_not_keep_originating_request_trace_alive() -> TestResult {
    use quanta_index_core::diagnostic_trace::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
        DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticTerminalV1,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Debug, Default)]
    struct TraceClock(AtomicU64);
    impl DiagnosticClockV1 for TraceClock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }

    #[derive(Debug)]
    struct TraceSink;
    impl DiagnosticEventSinkV1 for TraceSink {
        fn try_emit(&self, _event: DiagnosticEmitV1) -> bool {
            true
        }
    }

    let directory = tempfile::tempdir()?;
    let (engine, objects, _catalog) = kernel(directory.path())?;
    let view = configured(&engine, &objects)?;
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([98; 16]),
        Arc::new(TraceClock::default()),
        Arc::new(TraceSink),
    );
    assert_eq!(context.strong_count(), 1);
    let root = context.enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(1))?;
    let child = context.child_context(&root);
    let original = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30))
        .with_diagnostic_context(child.clone());
    let work = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        original,
    );
    let ticket = engine.pin_view(ViewSelection::Exact(view), &work)?;
    drop(work);
    drop(child);
    root.finish(DiagnosticTerminalV1::Ok)?;
    // The pin outlives its request without retaining a trace producer.
    assert_eq!(context.strong_count(), 1);
    ticket.release()?;
    Ok(())
}

fn stage(objects: &FileObjectStore, path: &str, bytes: &[u8]) -> TestResult<ObjectRef> {
    // Legacy physical-prestage test fixture only. Production producers stage
    // through Kernel ingress before allocating objects; online GC is disabled.
    let blob = objects.put(bytes, &[], &mut budget())?.reference;
    let record = SourceRecord {
        path: path.into(),
        bytes: blob,
        byte_len: u64::try_from(bytes.len())?,
        text: quanta_index_core::index::SourceTextAdmission::Excluded,
    };
    Ok(objects
        .put(&serde_json::to_vec(&record)?, &[blob], &mut budget())?
        .reference)
}

fn adopt_fixture_tree(
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    ingress: IngressRef,
    reference: ObjectRef,
    seen: &mut std::collections::BTreeSet<ObjectRef>,
) -> TestResult {
    if !seen.insert(reference) {
        return Ok(());
    }
    let stored = objects.get(reference, &mut budget())?;
    for child in stored.registration.children.iter().copied() {
        adopt_fixture_tree(catalog, objects, ingress, child, seen)?;
    }
    catalog.reserve_inputs(ingress, std::slice::from_ref(&stored.registration))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&stored.registration))?;
    Ok(())
}

fn stage_manifest(
    kernel: &Kernel<SqliteCatalog>,
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    key: OperationKey,
    mutations: Vec<OwnerMutation>,
) -> TestResult<OwnerMutationSet> {
    let ingress = kernel.begin_input(key)?;
    let mut seen = std::collections::BTreeSet::new();
    for reference in mutations.iter().filter_map(|mutation| mutation.next) {
        adopt_fixture_tree(catalog, objects, ingress, reference, &mut seen)?;
    }
    Ok(kernel.stage_mutations(ingress, mutations.into_iter().map(Ok), &mut budget())?)
}

fn source(kernel: &Kernel<SqliteCatalog>, view: ViewRef) -> TestResult<SourceCut> {
    Ok(kernel
        .pin_view(ViewSelection::Exact(view), &budget())?
        .view()
        .source)
}

fn request(
    kernel: &Kernel<SqliteCatalog>,
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    view: ViewRef,
    source: SourceCut,
    operation_sequence: u64,
    event_sequence: u64,
    owners: Vec<OwnerMutation>,
) -> TestResult<UpdateRequest> {
    let key = operation(operation_sequence);
    let mut transition = SourceTransition {
        expected_source: source,
        owners: stage_manifest(kernel, catalog, objects, key, owners)?,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("event sequence must start at one")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    Ok(UpdateRequest {
        operation: key,
        expected_view: view,
        kind: UpdateKind::SourceAdvance(transition),
    })
}

#[test]
fn direct_staged_record_cannot_publish_false_utf8_admission() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(directory.path())?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let body = objects.put(&[0xff], &[], &mut budget())?.reference;
    let record = SourceRecord {
        path: "invalid.rs".into(),
        bytes: body,
        byte_len: 1,
        text: quanta_index_core::index::SourceTextAdmission::Utf8 {
            policy: Digest([21; 32]),
        },
    };
    let record_ref = objects
        .put(&serde_json::to_vec(&record)?, &[body], &mut budget())?
        .reference;
    let original = source(&engine, genesis)?;
    let outcome = block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            genesis,
            original,
            1,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(record_ref),
            }],
        )?,
        &mut budget(),
    ))?;
    assert_eq!(
        outcome,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput))
    );
    assert_eq!(source(&engine, genesis)?, original);
    Ok(())
}

fn member(
    kernel: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<MemberBinding> {
    let ticket = kernel.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&spec().instance.0);
    let reference = objects
        .lookup_object(ticket.view().members, &key, &mut budget())?
        .ok_or("required member absent")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

fn hits(
    engine: &Kernel<SqliteCatalog>,
    view: ViewRef,
    objects: &FileObjectStore,
    member: &MemberBinding,
    query: &[u8],
) -> TestResult<Vec<OwnerKey>> {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let page = NativeLiteralIndex.search(
        member,
        LiteralQuery {
            literal: query,
            after: None,
            limit: 100,
        },
        LiteralReadContext {
            ticket: ticket.record(),
            input: member.input,
            objects,
            maps: objects,
            budget: &mut budget(),
        },
    )?;
    Ok(page.hits.into_iter().map(|hit| hit.owner).collect())
}

fn path_owner(objects: &FileObjectStore, cut: SourceCut, path: &str) -> TestResult<OwnerKey> {
    let mut key = b"path\0".to_vec();
    key.extend_from_slice(path.as_bytes());
    let marker = objects
        .lookup(cut.paths, &key, &mut budget())?
        .ok_or("source path marker absent")?;
    Ok(quanta_index_core::index::owner_from_path_map_value(
        marker,
        &budget(),
    )?)
}

fn map_count(
    objects: &FileObjectStore,
    root: quanta_index_core::index::MapRoot,
) -> TestResult<u64> {
    let mut after = None;
    let mut count = 0_u64;
    let mut work = large_fixture_budget();
    loop {
        let page = objects.scan(root, after.as_deref(), 128, &mut work)?;
        count = count
            .checked_add(u64::try_from(page.entries.len())?)
            .ok_or("map count overflow")?;
        after = page.entries.last().map(|(key, _)| key.clone());
        if !page.has_more {
            return Ok(count);
        }
        if page.entries.is_empty() {
            return Err("empty map continuation page".into());
        }
    }
}

#[test]
fn source_path_cycle_crosses_mutation_cursor_pages() -> TestResult {
    const OWNERS: u8 = 130;
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(directory.path())?;
    let configured = configured(&engine, &objects)?;
    let mut old_records = Vec::new();
    let mut initial = Vec::new();
    for id in 1..=OWNERS {
        let record = stage(
            &objects,
            &format!("file-{id:03}.rs"),
            format!("owner-{id:03}-old").as_bytes(),
        )?;
        old_records.push(record);
        initial.push(OwnerMutation {
            owner: owner(id),
            expected: None,
            next: Some(record),
        });
    }
    let first = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            initial,
        )?,
        &mut budget(),
    ))?)?;
    assert_eq!(
        path_owner(&objects, source(&engine, first)?, "file-001.rs")?,
        owner(1)
    );

    let mut cycle = Vec::new();
    for id in 1..=OWNERS {
        let target = if id == OWNERS { 1 } else { id + 1 };
        let next = stage(
            &objects,
            &format!("file-{target:03}.rs"),
            format!("owner-{id:03}-new").as_bytes(),
        )?;
        let prior = old_records
            .get(usize::from(id - 1))
            .copied()
            .ok_or("prior source record absent")?;
        cycle.push(OwnerMutation {
            owner: owner(id),
            expected: Some(prior),
            next: Some(next),
        });
    }
    let second = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            first,
            source(&engine, first)?,
            3,
            2,
            cycle,
        )?,
        &mut budget(),
    ))?)?;
    assert_eq!(
        path_owner(&objects, source(&engine, second)?, "file-001.rs")?,
        owner(OWNERS)
    );
    assert_eq!(
        path_owner(&objects, source(&engine, second)?, "file-129.rs")?,
        owner(128)
    );
    assert_eq!(
        path_owner(&objects, source(&engine, second)?, "file-130.rs")?,
        owner(129)
    );
    assert_eq!(
        path_owner(&objects, source(&engine, first)?, "file-001.rs")?,
        owner(1)
    );
    let member = member(&engine, &objects, second)?;
    assert_eq!(
        hits(&engine, second, &objects, &member, b"owner-129-new")?,
        vec![owner(129)]
    );
    Ok(())
}

#[test]
fn staged_mutation_pages_reject_duplicate_count_and_stale_ingress() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, _objects, _catalog) = kernel(directory.path())?;
    let _genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let first = OwnerMutation {
        owner: owner(1),
        expected: Some(ObjectRef(Digest([71; 32]))),
        next: None,
    };
    let root = engine.stage_mutation_page(
        ingress,
        quanta_index_core::index::MapRoot(None),
        std::slice::from_ref(&first),
        &mut budget(),
    )?;
    assert!(matches!(
        engine.stage_mutation_page(ingress, root, std::slice::from_ref(&first), &mut budget()),
        Err(IndexError::Conflict)
    ));
    assert!(matches!(
        engine.seal_mutations(ingress, root, 2, &mut budget()),
        Err(IndexError::InvalidInput(_))
    ));
    let sealed = engine.seal_mutations(ingress, root, 1, &mut budget())?;
    assert_eq!(sealed.count, 1);
    assert_eq!(sealed.root, root);
    let replacement = engine.begin_input(operation(1))?;
    assert_ne!(ingress.fence, replacement.fence);
    assert_eq!(
        engine.stage_mutation_page(
            ingress,
            root,
            &[OwnerMutation {
                owner: owner(2),
                ..first
            }],
            &mut budget()
        ),
        Err(IndexError::FenceLost)
    );
    Ok(())
}

#[test]
fn mutation_pages_allow_cross_page_key_reordering_with_canonical_seal() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, objects, _catalog) = kernel(directory.path())?;
    let _genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let mutation = |id| OwnerMutation {
        owner: large_owner(id),
        expected: Some(ObjectRef(Digest([71; 32]))),
        next: None,
    };
    // A path-ordered producer may generate arbitrarily ordered owner hashes.
    // Only each bounded network page must be sorted; the stored map supplies
    // the complete logical order independently of upload page boundaries.
    let high_page: Vec<_> = (2..=129).map(mutation).collect();
    let high_root = engine.stage_mutation_page(
        ingress,
        quanta_index_core::index::MapRoot(None),
        &high_page,
        &mut budget(),
    )?;
    let low = mutation(1);
    let complete_root = engine.stage_mutation_page(
        ingress,
        high_root,
        std::slice::from_ref(&low),
        &mut budget(),
    )?;
    assert!(matches!(
        engine.stage_mutation_page(
            ingress,
            complete_root,
            std::slice::from_ref(&mutation(2)),
            &mut budget()
        ),
        Err(IndexError::Conflict)
    ));
    assert!(matches!(
        engine.seal_mutations(ingress, complete_root, 128, &mut budget()),
        Err(IndexError::InvalidInput(_))
    ));
    let sealed = engine.seal_mutations(ingress, complete_root, 129, &mut budget())?;
    let mut expected = quanta_index_core::index::OwnerMutationDigest::default();
    for id in 1..=129 {
        expected.push(&mutation(id))?;
    }
    assert_eq!(sealed.count, 129);
    assert_eq!(sealed.digest, expected.finish());
    let mut replay = quanta_index_core::index::OwnerMutationCursor::new(
        sealed,
        objects.as_ref(),
        objects.as_ref(),
    );
    for id in 1..=129 {
        assert_eq!(replay.next_mutation(&mut budget())?, Some(mutation(id)));
    }
    assert_eq!(replay.next_mutation(&mut budget())?, None);
    assert_eq!(replay.sealed_set(), Some(sealed));
    Ok(())
}

#[test]
fn source_only_manifest_exceeds_legacy_direct_child_limit() -> TestResult {
    const OWNERS: u32 = 4_097;
    let directory = tempfile::tempdir()?;
    let (engine, objects, _catalog) = kernel(directory.path())?;
    let genesis = engine.create_workspace(workspace(), 1, &mut large_fixture_budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let mut staged_budget = large_fixture_budget();
    let mut manifest_root = quanta_index_core::index::MapRoot(None);
    let mut page = Vec::with_capacity(128);
    let mut last_record = None;
    for start in (1..=OWNERS).step_by(32) {
        let end = start.saturating_add(31).min(OWNERS);
        let captures = (start..=end)
            .map(|id| SourceInput {
                path: format!("source-{id:05}.rs"),
                bytes: b"initial contents".to_vec(),
                text: quanta_index_core::index::SourceTextAdmission::Excluded,
            })
            .collect::<Vec<_>>();
        let records = engine.stage_sources(ingress, &captures, &mut staged_budget)?;
        if records.len() != captures.len() {
            return Err("source batch lost an ordinal result".into());
        }
        for (id, record) in (start..=end).zip(records) {
            if id == OWNERS {
                last_record = Some(record);
            }
            page.push(OwnerMutation {
                owner: large_owner(id),
                expected: None,
                next: Some(record),
            });
            if page.len() == 128 {
                manifest_root = engine.stage_mutation_page(
                    ingress,
                    manifest_root,
                    &page,
                    &mut staged_budget,
                )?;
                page.clear();
            }
        }
    }
    if !page.is_empty() {
        manifest_root =
            engine.stage_mutation_page(ingress, manifest_root, &page, &mut staged_budget)?;
    }
    let manifest = engine.seal_mutations(
        ingress,
        manifest_root,
        u64::from(OWNERS),
        &mut staged_budget,
    )?;
    assert_eq!(manifest.count, u64::from(OWNERS));
    let mut initial_transition = SourceTransition {
        expected_source: source(&engine, genesis)?,
        owners: manifest,
        events: Vec::new(),
    };
    initial_transition.events.push(SourceEvent {
        producer: operation(1).producer,
        stream: operation(1).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: initial_transition.owner_digest()?,
    });
    let mut initial_budget = large_fixture_budget();
    let first = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::SourceAdvance(initial_transition),
        },
        &mut initial_budget,
    ))?)?;
    let first_cut = source(&engine, first)?;
    assert_eq!(map_count(&objects, first_cut.owners)?, u64::from(OWNERS));
    assert_eq!(map_count(&objects, first_cut.paths)?, u64::from(OWNERS));
    let old_ticket = engine.pin_view(ViewSelection::Exact(first), &budget())?;
    let old_record = last_record.ok_or("last staged source record absent")?;

    let second_ingress = engine.begin_input(operation(2))?;
    let updated_record = engine.stage_source(
        second_ingress,
        &format!("source-{OWNERS:05}.rs"),
        b"updated contents",
        &mut staged_budget,
    )?;
    let changed = OwnerMutation {
        owner: large_owner(OWNERS),
        expected: Some(old_record),
        next: Some(updated_record),
    };
    let changed_root = engine.stage_mutation_page(
        second_ingress,
        quanta_index_core::index::MapRoot(None),
        &[changed],
        &mut staged_budget,
    )?;
    let changed_set = engine.seal_mutations(second_ingress, changed_root, 1, &mut staged_budget)?;
    let mut delta_transition = SourceTransition {
        expected_source: first_cut,
        owners: changed_set,
        events: Vec::new(),
    };
    delta_transition.events.push(SourceEvent {
        producer: operation(2).producer,
        stream: operation(2).stream,
        stream_epoch: 1,
        predecessor: 1,
        sequence: 2,
        payload: delta_transition.owner_digest()?,
    });
    let mut delta_budget = large_fixture_budget();
    let second = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: first,
            kind: UpdateKind::SourceAdvance(delta_transition),
        },
        &mut delta_budget,
    ))?)?;
    let second_cut = source(&engine, second)?;
    assert_eq!(map_count(&objects, second_cut.owners)?, u64::from(OWNERS));
    assert_eq!(second_cut.paths, first_cut.paths);
    assert_eq!(old_ticket.view().source, first_cut);
    let unchanged_key = owner_map_key(large_owner(1));
    assert_eq!(
        objects.lookup_object(
            first_cut.owners,
            &unchanged_key,
            &mut large_fixture_budget()
        )?,
        objects.lookup_object(
            second_cut.owners,
            &unchanged_key,
            &mut large_fixture_budget()
        )?
    );
    let changed_key = owner_map_key(large_owner(OWNERS));
    assert_eq!(
        objects.lookup_object(first_cut.owners, &changed_key, &mut large_fixture_budget())?,
        Some(old_record)
    );
    assert_eq!(
        objects.lookup_object(second_cut.owners, &changed_key, &mut large_fixture_budget())?,
        Some(updated_record)
    );
    assert!(initial_budget.usage().mutations >= u64::from(OWNERS));
    assert!(
        delta_budget.usage().mutations
            < initial_budget
                .usage()
                .mutations
                .checked_div(8)
                .ok_or("nonzero ratio denominator")?
    );
    old_ticket.release()?;
    Ok(())
}

#[test]
fn create_edit_delete_old_reader_replay_and_cold_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let a1 = stage(&objects, "a.rs", b"alpha needle old")?;
    let b1 = stage(&objects, "b.rs", b"beta needle shared")?;
    let initial = request(
        &engine,
        &catalog,
        &objects,
        configured,
        source(&engine, configured)?,
        2,
        1,
        vec![
            OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(a1),
            },
            OwnerMutation {
                owner: owner(2),
                expected: None,
                next: Some(b1),
            },
        ],
    )?;
    let v1 = published(block_on(engine.submit(initial.clone(), &mut budget()))?)?;
    let old_ticket = engine.pin_view(ViewSelection::Exact(v1), &budget())?;
    let old_member = member(&engine, &objects, v1)?;
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"needle")?,
        vec![owner(1), owner(2)]
    );

    let a2 = stage(&objects, "renamed.rs", b"alpha replacement new")?;
    let edit = request(
        &engine,
        &catalog,
        &objects,
        v1,
        source(&engine, v1)?,
        3,
        2,
        vec![OwnerMutation {
            owner: owner(1),
            expected: Some(a1),
            next: Some(a2),
        }],
    )?;
    let v2 = published(block_on(engine.submit(edit, &mut budget()))?)?;
    assert_eq!(
        hits(
            &engine,
            v2,
            &objects,
            &member(&engine, &objects, v2)?,
            b"needle"
        )?,
        vec![owner(2)]
    );
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"needle")?,
        vec![owner(1), owner(2)]
    );
    assert_eq!(
        published(block_on(engine.submit(initial, &mut budget()))?)?,
        v1
    );

    let delete = request(
        &engine,
        &catalog,
        &objects,
        v2,
        source(&engine, v2)?,
        4,
        3,
        vec![OwnerMutation {
            owner: owner(2),
            expected: Some(b1),
            next: None,
        }],
    )?;
    let v3 = published(block_on(engine.submit(delete.clone(), &mut budget()))?)?;
    assert!(
        hits(
            &engine,
            v3,
            &objects,
            &member(&engine, &objects, v3)?,
            b"needle"
        )?
        .is_empty()
    );
    assert_eq!(old_ticket.view().identity, v1);
    old_ticket.release()?;
    drop(engine);
    drop(objects);

    let (reopened, objects, _reopened_catalog) = kernel(root.path())?;
    assert_eq!(
        reopened
            .pin_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1
                },
                &budget()
            )?
            .view()
            .identity,
        v3
    );
    assert_eq!(
        published(block_on(reopened.submit(delete, &mut budget()))?)?,
        v3
    );
    assert_eq!(
        hits(
            &reopened,
            v3,
            &objects,
            &member(&reopened, &objects, v3)?,
            b"replacement"
        )?,
        vec![owner(1)]
    );
    Ok(())
}

#[test]
fn native_frontier_65_streams_preserve_all_markers_and_late_conflict_does_not_publish() -> TestResult
{
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(directory.path())?;
    let configured = configured(&engine, &objects)?;
    let base = source(&engine, configured)?;
    let mut transition = SourceTransition {
        expected_source: base,
        owners: stage_manifest(&engine, &catalog, &objects, operation(2), Vec::new())?,
        events: Vec::new(),
    };
    let payload = transition.owner_digest()?;
    transition.events = (1_u8..=65)
        .map(|id| SourceEvent {
            producer: operation(2).producer,
            stream: StreamId([id; 16]),
            stream_epoch: 1,
            predecessor: 0,
            sequence: 1,
            payload,
        })
        .collect();
    let published_view = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget(),
    ))?)?;
    let cut = source(&engine, published_view)?;
    assert_eq!(cut.owners, base.owners);
    assert_eq!(cut.paths, base.paths);
    assert_ne!(cut.frontiers, base.frontiers);
    let old_ticket = engine.pin_view(ViewSelection::Exact(configured), &budget())?;
    for id in 1_u8..=65 {
        // Independent preexisting key format and closed event wire decoder.
        let mut key = b"frontier\0".to_vec();
        key.extend_from_slice(&operation(2).producer.0);
        key.extend_from_slice(&[id; 16]);
        let reference = objects
            .lookup_object(cut.frontiers, &key, &mut budget())?
            .ok_or("frontier lost across the 64-object store-group boundary")?;
        let stored = objects.get(reference, &mut budget())?;
        let event: SourceEvent = serde_json::from_slice(&stored.payload)?;
        assert_eq!(event.producer, operation(2).producer);
        assert_eq!(event.stream, StreamId([id; 16]));
        assert_eq!(
            (event.stream_epoch, event.predecessor, event.sequence),
            (1, 0, 1)
        );
        assert_eq!(event.payload, payload);
        assert!(stored.registration.children.is_empty());
        assert!(
            objects
                .lookup_object(old_ticket.view().source.frontiers, &key, &mut budget())?
                .is_none()
        );
    }
    let mut invalid = SourceTransition {
        expected_source: cut,
        owners: stage_manifest(&engine, &catalog, &objects, operation(3), Vec::new())?,
        events: Vec::new(),
    };
    let next_payload = invalid.owner_digest()?;
    invalid.events = (1_u8..=65)
        .map(|id| SourceEvent {
            producer: operation(3).producer,
            stream: StreamId([id; 16]),
            stream_epoch: 1,
            predecessor: if id == 65 { 2 } else { 1 },
            sequence: if id == 65 { 3 } else { 2 },
            payload: next_payload,
        })
        .collect();
    assert_eq!(
        block_on(engine.submit(
            UpdateRequest {
                operation: operation(3),
                expected_view: published_view,
                kind: UpdateKind::SourceAdvance(invalid),
            },
            &mut budget(),
        ))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
    );
    let latest = engine.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert_eq!(latest.view().identity, published_view);
    assert_eq!(latest.view().source, cut);
    latest.release()?;
    old_ticket.release()?;
    Ok(())
}

#[test]
fn independent_producer_frontiers_preserve_other_contributions_and_old_view() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let primary = (ProducerId([5; 16]), StreamId([6; 16]));
    let secondary = (ProducerId([15; 16]), StreamId([16; 16]));
    let secondary_owner = OwnerKey {
        producer: secondary.0,
        stable_id: StableOwnerId([2; 16]),
    };
    let primary_record = stage(&objects, "primary.rs", b"primary anchor")?;
    let secondary_record = stage(&objects, "secondary.rs", b"secondary marker")?;
    let mut initial = SourceTransition {
        expected_source: source(&engine, configured)?,
        owners: stage_manifest(
            &engine,
            &catalog,
            &objects,
            operation(2),
            vec![
                OwnerMutation {
                    owner: owner(1),
                    expected: None,
                    next: Some(primary_record),
                },
                OwnerMutation {
                    owner: secondary_owner,
                    expected: None,
                    next: Some(secondary_record),
                },
            ],
        )?,
        events: Vec::new(),
    };
    let initial_payload = initial.owner_digest()?;
    initial.events = [primary, secondary]
        .into_iter()
        .map(|(producer, stream)| SourceEvent {
            producer,
            stream,
            stream_epoch: 1,
            predecessor: 0,
            sequence: 1,
            payload: initial_payload,
        })
        .collect();
    let v1 = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(initial),
        },
        &mut budget(),
    ))?)?;
    let old_member = member(&engine, &objects, v1)?;
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"anchor")?,
        vec![owner(1)]
    );
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"marker")?,
        vec![secondary_owner]
    );

    let changed_secondary = stage(&objects, "secondary.rs", b"secondary updated")?;
    let mut secondary_delta = SourceTransition {
        expected_source: source(&engine, v1)?,
        owners: stage_manifest(
            &engine,
            &catalog,
            &objects,
            OperationKey {
                producer: secondary.0,
                stream: secondary.1,
                sequence: 41,
                ..operation(2)
            },
            vec![OwnerMutation {
                owner: secondary_owner,
                expected: Some(secondary_record),
                next: Some(changed_secondary),
            }],
        )?,
        events: Vec::new(),
    };
    secondary_delta.events.push(SourceEvent {
        producer: secondary.0,
        stream: secondary.1,
        stream_epoch: 1,
        predecessor: 1,
        sequence: 2,
        payload: secondary_delta.owner_digest()?,
    });
    let v2 = published(block_on(engine.submit(
        UpdateRequest {
            operation: OperationKey {
                producer: secondary.0,
                stream: secondary.1,
                sequence: 41,
                ..operation(2)
            },
            expected_view: v1,
            kind: UpdateKind::SourceAdvance(secondary_delta),
        },
        &mut budget(),
    ))?)?;
    let current_member = member(&engine, &objects, v2)?;
    assert_eq!(
        hits(&engine, v2, &objects, &current_member, b"anchor")?,
        vec![owner(1)]
    );
    assert_eq!(
        hits(&engine, v2, &objects, &current_member, b"updated")?,
        vec![secondary_owner]
    );
    assert!(hits(&engine, v2, &objects, &current_member, b"marker")?.is_empty());
    assert_eq!(
        hits(&engine, v1, &objects, &old_member, b"marker")?,
        vec![secondary_owner]
    );

    let frontier =
        |cut: SourceCut, producer: ProducerId, stream: StreamId| -> TestResult<SourceEvent> {
            let mut key = b"frontier\0".to_vec();
            key.extend_from_slice(&producer.0);
            key.extend_from_slice(&stream.0);
            let reference = objects
                .lookup_object(cut.frontiers, &key, &mut budget())?
                .ok_or("source frontier absent")?;
            Ok(serde_json::from_slice(
                &objects.get(reference, &mut budget())?.payload,
            )?)
        };
    let v2_source = source(&engine, v2)?;
    assert_eq!(frontier(v2_source, primary.0, primary.1)?.sequence, 1);
    assert_eq!(frontier(v2_source, secondary.0, secondary.1)?.sequence, 2);

    let changed_primary = stage(&objects, "primary.rs", b"primary gap")?;
    let gapped_key = OperationKey {
        sequence: 99,
        ..operation(2)
    };
    let mut gapped = SourceTransition {
        expected_source: v2_source,
        owners: stage_manifest(
            &engine,
            &catalog,
            &objects,
            gapped_key,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(primary_record),
                next: Some(changed_primary),
            }],
        )?,
        events: Vec::new(),
    };
    gapped.events.push(SourceEvent {
        producer: primary.0,
        stream: primary.1,
        stream_epoch: 1,
        predecessor: 2,
        sequence: 3,
        payload: gapped.owner_digest()?,
    });
    assert!(matches!(
        block_on(engine.submit(
            UpdateRequest {
                operation: gapped_key,
                expected_view: v2,
                kind: UpdateKind::SourceAdvance(gapped),
            },
            &mut budget(),
        ))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict))
    ));
    assert_eq!(
        engine
            .pin_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget()
            )?
            .view()
            .identity,
        v2
    );
    assert_eq!(source(&engine, v2)?, v2_source);
    assert_eq!(frontier(v2_source, primary.0, primary.1)?.sequence, 1);
    assert_eq!(frontier(v2_source, secondary.0, secondary.1)?.sequence, 2);
    Ok(())
}

#[test]
fn path_collision_refuses_without_advancing_source_and_event_can_be_rebound() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let a = stage(&objects, "same.rs", b"first")?;
    let v1 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(a),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let collision = stage(&objects, "same.rs", b"second")?;
    let original = request(
        &engine,
        &catalog,
        &objects,
        v1,
        source(&engine, v1)?,
        3,
        2,
        vec![OwnerMutation {
            owner: owner(2),
            expected: None,
            next: Some(collision),
        }],
    )?;
    let mut refusal_work = budget();
    let refused = block_on(engine.submit(original.clone(), &mut refusal_work))?;
    assert!(refusal_work.request().diagnostic_context().is_none());
    assert!(format!("{refusal_work:?}").contains("failed_stage: Some(NativeSourceApply)"));
    assert!(matches!(
        refused,
        SubmitOutcome::Terminal(OperationStatus::Refused(_))
    ));
    assert_eq!(
        engine
            .pin_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1
                },
                &budget()
            )?
            .view()
            .identity,
        v1
    );
    let corrected = stage(&objects, "other.rs", b"second")?;
    // Rebinding retains the original captured payload; fixing invalid input
    // requires an explicit resync protocol, never replacement of e2's identity.
    let mut rebound = original;
    rebound.operation.sequence = 4;
    assert!(matches!(
        block_on(engine.submit(rebound, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(_))
    ));
    assert!(matches!(
        block_on(engine.submit(
            request(
                &engine,
                &catalog,
                &objects,
                v1,
                source(&engine, v1)?,
                5,
                2,
                vec![OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(corrected)
                }]
            )?,
            &mut budget()
        )),
        Err(IndexError::DigestConflict)
    ));
    Ok(())
}

#[test]
fn late_path_collision_does_not_publish_earlier_path_value() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let occupied = stage(&objects, "z.rs", b"original")?;
    let v1 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(occupied),
            }],
        )?,
        &mut budget(),
    ))?)?;

    let first = stage(&objects, "a.rs", b"valid-first")?;
    let collision = stage(&objects, "z.rs", b"colliding-last")?;
    let old_cut = source(&engine, v1)?;
    assert!(
        objects
            .lookup(old_cut.paths, b"path\0a.rs", &mut budget())?
            .is_none()
    );
    let refused = block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            v1,
            source(&engine, v1)?,
            3,
            2,
            vec![
                OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(first),
                },
                OwnerMutation {
                    owner: owner(3),
                    expected: None,
                    next: Some(collision),
                },
            ],
        )?,
        &mut budget(),
    ))?;
    assert!(matches!(
        refused,
        SubmitOutcome::Terminal(OperationStatus::Refused(_))
    ));
    assert_eq!(source(&engine, v1)?, old_cut);
    assert!(
        objects
            .lookup(old_cut.paths, b"path\0a.rs", &mut budget())?
            .is_none()
    );
    assert_eq!(path_owner(&objects, old_cut, "z.rs")?, owner(1));
    assert_eq!(
        engine
            .pin_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget()
            )?
            .view()
            .identity,
        v1
    );
    Ok(())
}

#[test]
fn batch_boundary_preserves_all_owners_and_fixed_delta_work() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let records = (1..=65)
        .map(|id| {
            stage(
                &objects,
                &format!("f{id}.rs"),
                format!("needle item-{id:03}").as_bytes(),
            )
            .map(|reference| OwnerMutation {
                owner: owner(id),
                expected: None,
                next: Some(reference),
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    let expected = records
        .first()
        .and_then(|value| value.next)
        .ok_or("missing first record")?;
    let initial = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            records,
        )?,
        &mut budget(),
    ))?)?;
    // 65 distinct contributions and suffix grams cross both 64-entry batch
    // boundaries. The expected owner inventory is independent of storage.
    let original_member = member(&engine, &objects, initial)?;
    assert_eq!(
        hits(&engine, initial, &objects, &original_member, b"needle")?,
        (1..=65).map(owner).collect::<Vec<_>>()
    );
    assert_eq!(
        hits(&engine, initial, &objects, &original_member, b"item-065")?,
        vec![owner(65)]
    );
    let next = stage(&objects, "f1.rs", b"edited marker")?;
    let mut work = budget();
    let view = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            initial,
            source(&engine, initial)?,
            3,
            2,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(expected),
                next: Some(next),
            }],
        )?,
        &mut work,
    ))?)?;
    assert_eq!(
        hits(
            &engine,
            view,
            &objects,
            &member(&engine, &objects, view)?,
            b"edited"
        )?,
        vec![owner(1)]
    );
    let current_member = member(&engine, &objects, view)?;
    assert_eq!(
        hits(&engine, view, &objects, &current_member, b"needle")?,
        (2..=65).map(owner).collect::<Vec<_>>()
    );
    assert_eq!(
        hits(&engine, view, &objects, &current_member, b"item-065")?,
        vec![owner(65)]
    );
    assert_eq!(
        hits(&engine, initial, &objects, &original_member, b"needle")?,
        (1..=65).map(owner).collect::<Vec<_>>()
    );
    assert!(
        work.usage().writes < 1000,
        "one owner delta unexpectedly rewrote corpus-sized state"
    );
    Ok(())
}

/// Test transport preserves the server operation after losing the response.
/// It exercises the public SDK contract, not a daemon socket implementation.
struct LocalTransport {
    engine: Arc<Kernel<SqliteCatalog>>,
    lose_ack: std::sync::atomic::AtomicBool,
}

impl quanta_index_sdk::native_index::NativeIndexTransport for LocalTransport {
    fn submit(
        &self,
        request: UpdateRequest,
    ) -> quanta_index_sdk::native_index::NativeFuture<
        '_,
        quanta_index_core::index::OperationReply<SubmitOutcome>,
    > {
        Box::pin(async move {
            let key = request.operation;
            let result = self
                .engine
                .submit(request, &mut budget())
                .await
                .map_err(|error| {
                    quanta_index_sdk::native_index::NativeSubmitError::NotAdmitted(
                        error.to_string(),
                    )
                })?;
            if self
                .lose_ack
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(quanta_index_sdk::native_index::NativeSubmitError::Unknown(
                    key,
                ));
            }
            Ok(quanta_index_core::index::OperationReply {
                operation: key,
                payload: result,
            })
        })
    }

    fn status(
        &self,
        key: OperationKey,
    ) -> quanta_index_sdk::native_index::NativeFuture<
        '_,
        quanta_index_core::index::OperationReply<OperationStatus>,
    > {
        Box::pin(async move {
            let result = self
                .engine
                .status(
                    key,
                    quanta_index_core::index::ports::OperationInspectAccess::Request(&budget()),
                )
                .map_err(|error| {
                    quanta_index_sdk::native_index::NativeSubmitError::StatusTransport(
                        error.to_string(),
                    )
                })?;
            Ok(quanta_index_core::index::OperationReply {
                operation: key,
                payload: result,
            })
        })
    }
}

#[test]
fn sdk_reconciles_lost_ack_with_original_key_and_receipt() -> TestResult {
    use quanta_index_sdk::native_index::{NativeIndexClient, NativeSubmitError};
    let root = tempfile::tempdir()?;
    let (engine, _objects, _catalog) = kernel(root.path())?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let profile = engine.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![spec()],
        },
        &mut budget(),
    )?;
    let engine = Arc::new(engine);
    let transport = Arc::new(LocalTransport {
        engine: engine.clone(),
        lose_ack: std::sync::atomic::AtomicBool::new(true),
    });
    let client = NativeIndexClient::new(transport);
    let request = UpdateRequest {
        operation: operation(1),
        expected_view: genesis,
        kind: UpdateKind::Reconfigure { profile },
    };
    assert_eq!(
        block_on(client.submit(request.clone())),
        Err(NativeSubmitError::Unknown(request.operation))
    );
    let OperationStatus::Published(committed) = block_on(client.status(request.operation))? else {
        return Err("lost ACK did not retain durable publication".into());
    };
    assert_eq!(published(block_on(client.submit(request))?)?, committed);
    let latest = engine.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert_eq!(latest.view().identity, committed);
    Ok(())
}

#[derive(Clone)]
struct CountingOwnerAdapter(Arc<std::sync::atomic::AtomicUsize>);

impl quanta_index_core::index::ports::IndexOwnerBuildPort for CountingOwnerAdapter {
    type Candidate = quanta_index_core::index::OwnerMemberBinding;

    fn resource_demand(&self) -> quanta_index_core::index::effort::ResourceNeed {
        quanta_index_core::index::effort::ResourceNeed::default()
    }

    fn validate_instance(
        &self,
        _instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        Ok(())
    }

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: quanta_index_core::index::OwnerInputBinding,
        _prior: Option<quanta_index_core::index::OwnerMemberBinding>,
        context: &'a mut quanta_index_core::index::AttemptContext<'_>,
    ) -> quanta_index_core::index::ports::IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            let _previous = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let payload = serde_json::to_vec(&(owner, input))
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
            let children = input.source.into_iter().collect::<Vec<_>>();
            let artifact = context.put(&payload, &children)?;
            Ok(quanta_index_core::index::OwnerMemberBinding {
                instance: instance.instance,
                owner,
                input,
                format: instance.format,
                artifact,
                logical_commitment: Digest(Sha256::digest(&payload).into()),
            })
        })
    }

    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        _context: &'a mut quanta_index_core::index::AttemptContext<'_>,
    ) -> quanta_index_core::index::ports::IndexFuture<'a, Self::Candidate> {
        Box::pin(async move { Ok(candidate) })
    }
}

// The real materialize caller reads an owner artifact after verify_owner.
// This adapter rereads it during searchable preparation, after the Kernel has
// already authenticated and retained that exact artifact under the pin.
struct RepeatedArtifactOwner(CountingOwnerAdapter);

impl quanta_index_core::index::ports::IndexOwnerBuildPort for RepeatedArtifactOwner {
    type Candidate = quanta_index_core::index::OwnerMemberBinding;

    fn resource_demand(&self) -> ResourceNeed {
        self.0.resource_demand()
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.0.validate_instance(instance)
    }

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: quanta_index_core::index::OwnerInputBinding,
        prior: Option<Self::Candidate>,
        context: &'a mut quanta_index_core::index::AttemptContext<'_>,
    ) -> quanta_index_core::index::ports::IndexFuture<'a, Self::Candidate> {
        self.0.prepare_owner(instance, owner, input, prior, context)
    }

    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut quanta_index_core::index::AttemptContext<'_>,
    ) -> quanta_index_core::index::ports::IndexFuture<'a, Self::Candidate> {
        self.0.verify_owner(candidate, context)
    }

    fn prepare_searchable<'a>(
        &'a self,
        _instance: &'a InstanceSpec,
        _prior: Option<&'a quanta_index_core::index::CoverageBinding>,
        owners: quanta_index_core::index::MapRoot,
        materialized: Option<(OwnerKey, ObjectRef)>,
        _object_maps: Option<&'a Arc<dyn quanta_index_core::index::ports::ObjectMapBatchPort>>,
        context: &'a mut quanta_index_core::index::AttemptContext<'_>,
    ) -> quanta_index_core::index::ports::IndexFuture<
        'a,
        quanta_index_core::index::SearchablePreparation,
    > {
        Box::pin(async move {
            context.budget.checkpoint()?;
            let Some((owner, binding_ref)) = materialized else {
                return Ok(quanta_index_core::index::SearchablePreparation::existing(
                    owners.0,
                ));
            };
            let stored = context.objects.get(binding_ref, context.budget)?;
            let binding: quanta_index_core::index::OwnerMemberBinding =
                serde_json::from_slice(&stored.payload)
                    .map_err(|error| IndexError::CorruptObject(error.to_string()))?;
            if binding.owner != owner {
                return Err(IndexError::CorruptObject("owner binding changed".into()));
            }
            let expected = serde_json::to_vec(&(owner, binding.input))
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
            for _ in 0..2 {
                let artifact = context.objects.get(binding.artifact, context.budget)?;
                if artifact.registration.reference != binding.artifact
                    || artifact.payload.as_slice() != expected.as_slice()
                {
                    return Err(IndexError::CorruptObject(
                        "repeated owner artifact changed".into(),
                    ));
                }
            }
            Ok(quanta_index_core::index::SearchablePreparation::existing(
                owners.0,
            ))
        })
    }
}

fn async_spec() -> InstanceSpec {
    InstanceSpec {
        activation: GroupActivation::AsyncCoverage,
        ..spec()
    }
}

fn owner_kernel(
    root: &std::path::Path,
    calls: Arc<std::sync::atomic::AtomicUsize>,
) -> TestResult<(
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
)> {
    let objects = Arc::new(FileObjectStore::open(root.join("native-objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        CountingOwnerAdapter(calls),
    )?;
    registry.register(
        AdapterDescriptor {
            kind: IndexKind([78; 16]),
            schema: spec().schema,
            format: spec().format,
        },
        NativeLiteralIndex,
        |delta| Ok(delta.clone()),
    )?;
    let services = KernelServices::new(
        objects.clone(),
        objects.clone(),
        catalog.clone(),
        Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.join("scratch"),
            16 * 1024 * 1024 * 1024,
        )?),
        Duration::from_secs(30),
    )?
    .with_object_map_backend(objects.clone());
    Ok((Kernel::new(services, registry), objects, catalog))
}

#[test]
fn mixed_activation_profile_refused_before_staging_object() -> TestResult {
    let directory = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, _catalog) = owner_kernel(directory.path(), Arc::clone(&calls))?;
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let core = InstanceSpec {
        instance: IndexInstanceId([79; 16]),
        kind: IndexKind([78; 16]),
        activation: GroupActivation::CoreRequired,
        ..spec()
    };
    let profile = IndexProfile {
        instances: vec![async_spec(), core],
    };
    let profile_ref = objects
        .identify(&serde_json::to_vec(&profile)?, &[], &mut budget())?
        .reference;
    assert!(matches!(
        objects.get(profile_ref, &mut budget()),
        Err(IndexError::CorruptObject(_))
    ));
    assert_eq!(
        engine.stage_profile(ingress, &profile, &mut budget()),
        Err(IndexError::InvalidInput(
            "publication group has mixed activation".into()
        ))
    );
    assert!(matches!(
        objects.get(profile_ref, &mut budget()),
        Err(IndexError::CorruptObject(_))
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        engine
            .pin_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget()
            )?
            .view()
            .identity,
        genesis
    );
    Ok(())
}

fn configured_owner(
    kernel: &Kernel<SqliteCatalog>,
    _objects: &FileObjectStore,
) -> TestResult<ViewRef> {
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = kernel.begin_input(operation(1))?;
    let profile = kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![async_spec()],
        },
        &mut budget(),
    )?;
    published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)
}

fn coverage(
    kernel: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<quanta_index_core::index::CoverageBinding> {
    let ticket = kernel.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&spec().instance.0);
    let reference = objects
        .lookup_object(ticket.view().coverage, &key, &mut budget())?
        .ok_or("coverage missing")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

fn owner_coverage(
    objects: &FileObjectStore,
    coverage: &quanta_index_core::index::CoverageBinding,
    owner: OwnerKey,
) -> TestResult<quanta_index_core::index::OwnerCoverage> {
    let reference = objects
        .lookup(
            coverage.owners,
            &quanta_index_core::index::owner_map_key(owner),
            &mut budget(),
        )?
        .ok_or("owner coverage missing")?;
    Ok(quanta_index_core::index::read_owner_coverage(
        reference,
        &quanta_index_core::index::owner_map_key(owner),
        objects,
        &mut budget(),
    )?)
}

fn intent_for(
    kernel: &Kernel<SqliteCatalog>,
    owner: OwnerKey,
) -> TestResult<quanta_index_core::index::WorkIntent> {
    kernel.ready_work(workspace(), 1, 128, &mut budget())?.into_iter()
        .find(|intent| matches!(intent.scope, quanta_index_core::index::WorkScope::Owner { owner: candidate, .. } if candidate == owner))
        .ok_or_else(|| "owner work missing".into())
}

fn complete_owner(
    kernel: &Kernel<SqliteCatalog>,
    view: ViewRef,
    sequence: u64,
    owner: OwnerKey,
) -> TestResult<ViewRef> {
    let claim = kernel.claim_work(
        &intent_for(kernel, owner)?,
        Duration::from_secs(120),
        &budget(),
    )?;
    published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(sequence),
            expected_view: view,
            kind: UpdateKind::Materialize { claim },
        },
        &mut budget(),
    ))?)
}

#[test]
fn materialize_trace_preserves_result_and_work_with_balanced_owner_phases() -> TestResult {
    use quanta_index_core::diagnostic_trace::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
        DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
        DiagnosticStageV1, DiagnosticTerminalV1,
    };
    use std::collections::BTreeMap;
    use std::sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    };

    #[derive(Debug, Default)]
    struct TraceClock(AtomicU64);
    impl DiagnosticClockV1 for TraceClock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }
    #[derive(Debug)]
    struct TraceSink(Mutex<Vec<DiagnosticEmitV1>>);
    impl DiagnosticEventSinkV1 for TraceSink {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            let Ok(mut rows) = self.0.try_lock() else {
                return false;
            };
            if rows.len() == 512 {
                return false;
            }
            rows.push(event);
            true
        }
    }

    let clock = Arc::new(TraceClock::default());
    let mut rows = Vec::new();
    rows.try_reserve_exact(512)?;
    let sink = Arc::new(TraceSink(Mutex::new(rows)));
    let context =
        DiagnosticContextV1::new(DiagnosticCaptureIdV1([97; 16]), clock.clone(), sink.clone());
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?;
    let root_id = root.span_id().get();
    let child = context.child_context(&root);
    let mut observed = Vec::new();
    for record in [false, true] {
        let prior_clock = clock.0.load(Ordering::Relaxed);
        let directory = tempfile::tempdir()?;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (engine, objects, catalog) = owner_kernel(directory.path(), calls.clone())?;
        let configured = configured_owner(&engine, &objects)?;
        let source_record = stage(&objects, "trace-owner.rs", b"trace owner content")?;
        let second_record = stage(&objects, "trace-second.rs", b"second owner content")?;
        let pending = published(block_on(engine.submit(
            request(
                &engine,
                &catalog,
                &objects,
                configured,
                source(&engine, configured)?,
                2,
                1,
                vec![
                    OwnerMutation {
                        owner: owner(1),
                        expected: None,
                        next: Some(source_record),
                    },
                    OwnerMutation {
                        owner: owner(2),
                        expected: None,
                        next: Some(second_record),
                    },
                ],
            )?,
            &mut budget(),
        ))?)?;
        let claim = engine.claim_work(
            &intent_for(&engine, owner(1))?,
            Duration::from_secs(120),
            &budget(),
        )?;
        let request_budget =
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(60));
        let request_budget = if record {
            request_budget.with_diagnostic_context(child.clone())
        } else {
            request_budget
        };
        let limits = WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        };
        let mut work = WorkBudget::with_request(limits, request_budget);
        let completed = published(block_on(engine.submit(
            UpdateRequest {
                operation: operation(3),
                expected_view: pending,
                kind: UpdateKind::Materialize { claim },
            },
            &mut work,
        ))?)?;
        let success_usage = work.usage();
        let second_claim = engine.claim_work(
            &intent_for(&engine, owner(2))?,
            Duration::from_secs(120),
            &budget(),
        )?;
        let stale_claim = quanta_index_core::index::WorkClaim {
            fence: second_claim
                .fence
                .checked_add(1)
                .ok_or("work fence overflow")?,
            ..second_claim
        };
        let request_budget =
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(60));
        let request_budget = if record {
            request_budget.with_diagnostic_context(child.clone())
        } else {
            request_budget
        };
        let mut refused_work = WorkBudget::with_request(limits, request_budget);
        let refused = block_on(engine.submit(
            UpdateRequest {
                operation: operation(4),
                expected_view: completed,
                kind: UpdateKind::Materialize { claim: stale_claim },
            },
            &mut refused_work,
        ));
        assert!(
            matches!(
                &refused,
                Ok(SubmitOutcome::Terminal(OperationStatus::Refused(
                    Refusal::FenceLost
                )))
            ),
            "invalid second owner work fence was accepted"
        );
        assert_eq!(
            engine
                .pin_view(
                    ViewSelection::Latest {
                        workspace: workspace(),
                        epoch: 1
                    },
                    &budget()
                )?
                .view()
                .identity,
            completed,
            "refused materialization changed the selected view"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "owner adapter ran more than once"
        );
        observed.push((completed, success_usage, refused?, refused_work.usage()));
        if !record {
            assert_eq!(
                clock.0.load(Ordering::Relaxed),
                prior_clock,
                "Off materialization touched diagnostic clock"
            );
            assert_eq!(
                sink.0.lock().map_err(|error| error.to_string())?.len(),
                1,
                "Off materialization emitted events"
            );
        }
    }
    let [off, record] = observed.as_slice() else {
        return Err("Off/Record materialization observations missing".into());
    };
    assert_eq!(
        off, record,
        "Off/Record changed materialization result or work"
    );
    root.finish(DiagnosticTerminalV1::Ok)?;
    let events = sink.0.lock().map_err(|error| error.to_string())?;
    let mut beginnings = BTreeMap::new();
    let mut terminals = BTreeMap::new();
    let mut stages = Vec::new();
    for event in events.iter() {
        match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage,
                ..
            } => {
                assert!(
                    beginnings
                        .insert(
                            span_id.get(),
                            (
                                parent_span_id.map(quanta_index_core::DiagnosticSpanIdV1::get),
                                stage
                            )
                        )
                        .is_none(),
                    "duplicate diagnostic span begin"
                );
                if span_id.get() != root_id {
                    stages.push(stage);
                }
            }
            DiagnosticEventKindV1::SpanEnd { span_id, outcome } => {
                assert!(
                    terminals.insert(span_id.get(), outcome).is_none(),
                    "duplicate diagnostic span terminal"
                );
            }
            DiagnosticEventKindV1::Health { .. } => {
                return Err("materialize trace health failure".into());
            }
            DiagnosticEventKindV1::RequestIdentity { .. }
            | DiagnosticEventKindV1::PhaseIdentity { .. }
            | DiagnosticEventKindV1::Counter { .. }
            | DiagnosticEventKindV1::AnnRegionObservation { .. }
            | DiagnosticEventKindV1::SqliteSyncObservation { .. }
            | DiagnosticEventKindV1::SyncAggregate { .. }
            | DiagnosticEventKindV1::ObjectKindAggregate { .. }
            | DiagnosticEventKindV1::AdoptionGroupAggregate { .. }
            | DiagnosticEventKindV1::Bm25CensusAggregate { .. }
            | DiagnosticEventKindV1::Bm25BoundaryAggregate { .. }
            | DiagnosticEventKindV1::Bm25LiveBuildAggregate { .. }
            | DiagnosticEventKindV1::Link { .. } => {}
        }
    }
    for (lane, api) in [(23, "prepared_work"), (24, "stage_work")] {
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event.kind,
                    DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::SqliteWork,
                        lane: DiagnosticLaneV1(observed),
                        ..
                    } if observed == lane
                ))
                .count(),
            1,
            "materialize {api} envelope missing or repeated"
        );
    }
    drop(events);
    assert_eq!(
        beginnings.len(),
        terminals.len(),
        "materialize trace left an open span"
    );
    assert_eq!(
        terminals
            .values()
            .filter(|outcome| **outcome == DiagnosticTerminalV1::Error { code: 1 })
            .count(),
        1,
        "materialize refusal must close exactly one failed Catalog span"
    );
    for (span_id, (parent, stage)) in &beginnings {
        let expected = if *span_id == root_id {
            DiagnosticTerminalV1::Ok
        } else if *stage == DiagnosticStageV1::SqliteWork
            && terminals.get(span_id) == Some(&DiagnosticTerminalV1::Error { code: 1 })
        {
            DiagnosticTerminalV1::Error { code: 1 }
        } else {
            DiagnosticTerminalV1::Ok
        };
        assert_eq!(
            terminals.get(span_id),
            Some(&expected),
            "materialize span outcome differs"
        );
        if *span_id != root_id {
            assert_eq!(*parent, Some(root_id), "materialize span has wrong parent");
        }
    }
    for stage in [
        DiagnosticStageV1::BuildPrepare,
        DiagnosticStageV1::Finalize,
        DiagnosticStageV1::SemanticPrepare,
        DiagnosticStageV1::NativeCoverageSave,
        DiagnosticStageV1::NativePublish,
    ] {
        assert_eq!(
            stages.iter().filter(|seen| **seen == stage).count(),
            1,
            "materialize owner phase missing or repeated"
        );
    }
    assert!(
        stages
            .iter()
            .filter(|stage| **stage == DiagnosticStageV1::SqliteWork)
            .count()
            >= 6,
        "materialize Catalog scopes were not observed"
    );
    Ok(())
}

#[test]
fn async_coverage_and_work_manifest_cross_leaf_batch_boundary() -> TestResult {
    use quanta_index_core::index::{CoverageStatus, WorkScope};

    const OWNERS: u8 = 65;
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls.clone())?;
    let configured = configured_owner(&engine, &objects)?;
    let mut mutations = Vec::new();
    for id in 1..=OWNERS {
        let record = stage(
            &objects,
            &format!("owner-{id:03}.rs"),
            format!("content-{id:03}").as_bytes(),
        )?;
        mutations.push(OwnerMutation {
            owner: owner(id),
            expected: None,
            next: Some(record),
        });
    }
    let view = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            mutations,
        )?,
        &mut budget(),
    ))?)?;
    let binding = coverage(&engine, &objects, view)?;
    let CoverageStatus::Partial { missing } = binding.status else {
        return Err("65 changed owners must remain explicitly pending".into());
    };
    assert_eq!(binding.owners, missing);
    assert_eq!(map_count(&objects, binding.owners)?, u64::from(OWNERS));
    assert_eq!(map_count(&objects, missing)?, u64::from(OWNERS));
    let ready = engine.ready_work(workspace(), 1, 128, &mut budget())?;
    assert_eq!(ready.len(), usize::from(OWNERS));
    let ready_owners = ready
        .iter()
        .map(|intent| match intent.scope {
            WorkScope::Owner { owner, .. } => Ok(owner),
            WorkScope::ProjectionDomain { .. } => Err("unexpected projection work"),
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    assert_eq!(ready_owners, (1..=OWNERS).map(owner).collect());
    for id in 1..=OWNERS {
        let current = owner_coverage(&objects, &binding, owner(id))?;
        assert!(current.covered.is_none());
        assert!(current.searchable.is_none());
        let key = quanta_index_core::index::owner_map_key(owner(id));
        let value = objects
            .lookup(binding.owners, &key, &mut budget())?
            .ok_or("pending value absent")?;
        let quanta_index_core::index::MapValue::Inline(data) = value else {
            return Err("pending source coverage has a wrapper object".into());
        };
        assert_eq!(data.data()?, current.desired.derivation_policy.0);
        assert_eq!(
            data.children()?,
            &[current.desired.source.ok_or("pending source absent")?]
        );
        assert_eq!(objects.lookup(missing, &key, &mut budget())?, Some(value));
        // The shape is strict; neither missing nor extra physical source edges
        // can be repaired into a logical pending record by the reader.
        for invalid in [
            quanta_index_core::index::MapValue::inline(data.data()?, None)?,
            quanta_index_core::index::MapValue::inline(
                data.data()?,
                data.children()?.iter().copied().cycle().take(2),
            )?,
            quanta_index_core::index::MapValue::inline(&[0; 31], data.children()?.iter().copied())?,
        ] {
            assert!(matches!(
                quanta_index_core::index::read_owner_coverage(
                    invalid,
                    &key,
                    objects.as_ref(),
                    &mut budget()
                ),
                Err(IndexError::CorruptObject(_))
            ));
        }
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    let completed = complete_owner(&engine, view, 3, owner(1))?;
    let current = coverage(&engine, &objects, completed)?;
    let CoverageStatus::Partial { missing } = current.status else {
        return Err("64 owners must remain pending after one completion".into());
    };
    assert_ne!(current.owners, missing);
    assert_eq!(map_count(&objects, missing)?, u64::from(OWNERS - 1));
    assert_eq!(
        engine.ready_work(workspace(), 1, 128, &mut budget())?.len(),
        usize::from(OWNERS - 1)
    );
    let completed_owner = owner_coverage(&objects, &current, owner(1))?;
    assert_eq!(completed_owner.covered, Some(completed_owner.desired));
    assert!(completed_owner.searchable.is_some());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    // The prior exact view remains a complete record of all 65 pending owners.
    assert_eq!(
        map_count(
            &objects,
            match coverage(&engine, &objects, view)?.status {
                CoverageStatus::Partial { missing } => missing,
                CoverageStatus::Pending { .. } | CoverageStatus::Complete => {
                    return Err("old view lost pending coverage".into());
                }
            }
        )?,
        u64::from(OWNERS)
    );
    Ok(())
}

#[test]
fn owner_jobs_rebase_over_unrelated_edits_and_aba_reuses_durable_result() -> TestResult {
    use quanta_index_core::index::{WorkScope, WorkStatus};
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls.clone())?;
    let configured = configured_owner(&engine, &objects)?;
    let a1 = stage(&objects, "a.rs", b"original")?;
    let b1 = stage(&objects, "b.rs", b"unrelated")?;
    let v1 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![
                OwnerMutation {
                    owner: owner(1),
                    expected: None,
                    next: Some(a1),
                },
                OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(b1),
                },
            ],
        )?,
        &mut budget(),
    ))?)?;
    let a_intent = intent_for(&engine, owner(1))?;
    let a_claim = engine.claim_work(&a_intent, Duration::from_secs(120), &budget())?;
    let b_intent = intent_for(&engine, owner(2))?;
    let b_claim = engine.claim_work(&b_intent, Duration::from_secs(120), &budget())?;
    let b2 = stage(&objects, "b.rs", b"edited unrelated")?;
    let v2 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            v1,
            source(&engine, v1)?,
            3,
            2,
            vec![OwnerMutation {
                owner: owner(2),
                expected: Some(b1),
                next: Some(b2),
            }],
        )?,
        &mut budget(),
    ))?)?;
    assert_eq!(
        engine.work_status(&b_intent, &budget())?,
        WorkStatus::Superseded
    );
    assert_eq!(
        engine.work_status(&a_intent, &budget())?,
        WorkStatus::Claimed(a_claim)
    );
    let v3 = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(4),
            expected_view: v1,
            kind: UpdateKind::Materialize { claim: a_claim },
        },
        &mut budget(),
    ))?)?;
    assert!(v3.sequence > v2.sequence);
    let current = owner_coverage(&objects, &coverage(&engine, &objects, v3)?, owner(1))?;
    assert_eq!(current.covered, Some(current.desired));
    let stale = block_on(engine.submit(
        UpdateRequest {
            operation: operation(5),
            expected_view: v1,
            kind: UpdateKind::Materialize { claim: b_claim },
        },
        &mut budget(),
    ))?;
    assert!(matches!(
        stale,
        SubmitOutcome::Terminal(OperationStatus::Refused(_))
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let a2 = stage(&objects, "a.rs", b"replacement")?;
    let v4 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            v3,
            source(&engine, v3)?,
            6,
            3,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(a1),
                next: Some(a2),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let v5 = complete_owner(&engine, v4, 7, owner(1))?;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let v6 = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            v5,
            source(&engine, v5)?,
            8,
            4,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(a2),
                next: Some(a1),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let restored_intent = intent_for(&engine, owner(1))?;
    assert_eq!(restored_intent, a_intent);
    assert!(matches!(restored_intent.scope, WorkScope::Owner { .. }));
    let v7 = complete_owner(&engine, v6, 9, owner(1))?;
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "ABA repeated expensive owner derivation"
    );
    let restored = owner_coverage(&objects, &coverage(&engine, &objects, v7)?, owner(1))?;
    assert_eq!(restored.desired, current.desired);
    assert_eq!(restored.covered, Some(restored.desired));
    let old = owner_coverage(&objects, &coverage(&engine, &objects, v1)?, owner(1))?;
    assert_eq!(
        old.covered, None,
        "materialization changed old immutable view"
    );
    Ok(())
}

#[test]
fn interrupted_resume_does_not_rotate_a_pending_operation_fence() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(directory.path())?;
    let configured = configured(&engine, &objects)?;
    let record = stage(&objects, "resume.rs", b"pending recovery")?;
    let request = request(
        &engine,
        &catalog,
        &objects,
        configured,
        source(&engine, configured)?,
        2,
        1,
        vec![OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(record),
        }],
    )?;
    let key = request.operation;
    // Use the public durable admission boundary to leave real Pending custody.
    // A low write budget may legitimately produce a terminal refusal instead.
    let UpdateKind::SourceAdvance(transition) = &request.kind else {
        return Err("expected source transition fixture".into());
    };
    let ingress = engine.begin_input(key)?;
    let stored_request = objects.put(
        &serde_json::to_vec(&request)?,
        &transition.owners.object_children(),
        &mut budget(),
    )?;
    catalog.reserve_inputs(ingress, std::slice::from_ref(&stored_request))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&stored_request))?;
    catalog.seal_inputs(ingress, stored_request.reference)?;
    assert!(matches!(
        catalog.admit(
            ImmutableOperation {
                key,
                request: stored_request.reference,
                source_mode: quanta_index_core::index::SourceOperationMode::Advance,
            },
            ingress,
            &transition.events,
            &mut budget(),
        )?,
        AdmissionOutcome::New(_)
    ));
    let limits = WorkLimits {
        reads: 1_000_000,
        writes: 1,
        bytes: 512 * 1024 * 1024,
        mutations: 1_000_000,
    };
    let original = engine.status(
        key,
        quanta_index_core::index::ports::OperationInspectAccess::Request(&budget()),
    )?;
    assert!(matches!(original, OperationStatus::Pending(_)));

    let mut expired = WorkBudget::new(limits, Instant::now());
    assert_eq!(
        block_on(engine.resume(key, &mut expired)),
        Err(IndexError::ResourceLimited)
    );
    assert_eq!(
        engine.status(
            key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        original
    );

    let request_budget = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
    request_budget.cancel_handle().cancel();
    let mut cancelled = WorkBudget::with_request(limits, request_budget);
    assert_eq!(
        block_on(engine.resume(key, &mut cancelled)),
        Err(IndexError::Cancelled)
    );
    assert_eq!(
        engine.status(
            key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        original
    );
    Ok(())
}

#[test]
fn expired_work_claim_terminates_old_operation_and_recovery_can_complete() -> TestResult {
    use quanta_index_core::index::Refusal;
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls.clone())?;
    let configured = configured_owner(&engine, &objects)?;
    let a = stage(&objects, "a.rs", b"lease test")?;
    let view = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(a),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let intent = intent_for(&engine, owner(1))?;
    let expired = engine.claim_work(&intent, Duration::from_millis(1), &budget())?;
    std::thread::sleep(Duration::from_millis(5));
    let old_operation = operation(3);
    let outcome = block_on(engine.submit(
        UpdateRequest {
            operation: old_operation,
            expected_view: view,
            kind: UpdateKind::Materialize { claim: expired },
        },
        &mut budget(),
    ))?;
    assert_eq!(
        outcome,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::FenceLost))
    );
    assert_eq!(
        block_on(engine.resume(old_operation, &mut budget()))?,
        outcome
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let recovered = engine.claim_work(&intent, Duration::from_secs(120), &budget())?;
    assert_ne!(expired.fence, recovered.fence);
    let result = block_on(engine.submit(
        UpdateRequest {
            operation: operation(4),
            expected_view: view,
            kind: UpdateKind::Materialize { claim: recovered },
        },
        &mut budget(),
    ))?;
    let _published = published(result)?;
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

fn file_member(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<MemberBinding> {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&file_spec().instance.0);
    let reference = objects
        .lookup_object(ticket.view().members, &key, &mut budget())?
        .ok_or("file member missing")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

#[test]
fn required_lexical_and_filemap_publish_one_view_and_prefix_pagination_is_exact() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let a = stage(&objects, "a", b"needle")?;
    let ab = stage(&objects, "ab", b"needle")?;
    let z = stage(&objects, "z", b"irrelevant")?;
    let view = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![
                OwnerMutation {
                    owner: owner(1),
                    expected: None,
                    next: Some(a),
                },
                OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(ab),
                },
                OwnerMutation {
                    owner: owner(3),
                    expected: None,
                    next: Some(z),
                },
            ],
        )?,
        &mut budget(),
    ))?)?;
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let file_binding = file_member(&engine, &objects, view)?;
    let mut work = budget();
    let mut context = FileMapReadContext {
        ticket: ticket.record(),
        objects: objects.as_ref(),
        maps: objects.as_ref(),
        budget: &mut work,
    };
    let first = query_paths(
        &FilePathQuery {
            member: &file_binding,
            prefix: "a",
            after: None,
            limit: 1,
        },
        &mut context,
    )?;
    assert_eq!(
        first
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a"]
    );
    assert!(first.has_more);
    let second = query_paths(
        &FilePathQuery {
            member: &file_binding,
            prefix: "a",
            after: Some("a"),
            limit: 1,
        },
        &mut context,
    )?;
    assert_eq!(
        second
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["ab"]
    );
    assert!(!second.has_more);
    let missing = query_paths(
        &FilePathQuery {
            member: &file_binding,
            prefix: "missing/",
            after: None,
            limit: 1,
        },
        &mut context,
    )?;
    assert!(missing.entries.is_empty());
    assert!(!missing.has_more);
    assert_eq!(
        hits(
            &engine,
            view,
            &objects,
            &member(&engine, &objects, view)?,
            b"needle"
        )?,
        vec![owner(1), owner(2)]
    );
    let need = ResourceNeed {
        cpu_units: 1,
        io_bytes: 4096,
        memory_bytes: 4096,
        provider_units: 0,
    };
    let literal_reader = NativeLiteralReader::new(need)?;
    let literal_spec = spec();
    let mut literal_budget = budget();
    let mut literal_context = QueryReadContext {
        ticket: ticket.record(),
        instance: &literal_spec,
        objects: objects.as_ref(),
        maps: objects.as_ref(),
        budget: &mut literal_budget,
    };
    let first_literal = literal_reader.query(
        &LiteralReadRequest {
            literal: b"needle".to_vec(),
            after: None,
            limit: 1,
        },
        &mut literal_context,
    )?;
    assert_eq!(
        first_literal
            .result()
            .hits
            .iter()
            .map(|hit| hit.owner)
            .collect::<Vec<_>>(),
        vec![owner(1)]
    );
    assert!(!first_literal.exhaustive());
    let second_literal = literal_reader.query(
        &LiteralReadRequest {
            literal: b"needle".to_vec(),
            after: first_literal.result().after.clone(),
            limit: 1,
        },
        &mut literal_context,
    )?;
    assert_eq!(
        second_literal
            .result()
            .hits
            .iter()
            .map(|hit| hit.owner)
            .collect::<Vec<_>>(),
        vec![owner(2)]
    );
    assert!(second_literal.exhaustive());

    let path_reader = NativePathReader::new(need)?;
    let path_spec = file_spec();
    let mut path_budget = budget();
    let mut path_context = QueryReadContext {
        ticket: ticket.record(),
        instance: &path_spec,
        objects: objects.as_ref(),
        maps: objects.as_ref(),
        budget: &mut path_budget,
    };
    let first_path = path_reader.query(
        &PathReadRequest {
            prefix: "a".into(),
            after: None,
            limit: 1,
        },
        &mut path_context,
    )?;
    assert_eq!(
        first_path
            .result()
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a"]
    );
    assert!(!first_path.exhaustive());
    let second_path = path_reader.query(
        &PathReadRequest {
            prefix: "a".into(),
            after: Some("a".into()),
            limit: 1,
        },
        &mut path_context,
    )?;
    assert_eq!(
        second_path
            .result()
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec!["ab"]
    );
    assert!(second_path.exhaustive());
    Ok(())
}

#[test]
fn empty_source_advance_rebinds_real_filemap_and_preserves_pinned_paths() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let a = stage(&objects, "a", b"needle")?;
    let ab = stage(&objects, "ab", b"needle")?;
    let first = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![
                OwnerMutation {
                    owner: owner(1),
                    expected: None,
                    next: Some(a),
                },
                OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(ab),
                },
            ],
        )?,
        &mut budget(),
    ))?)?;
    let old_ticket = engine.pin_view(ViewSelection::Exact(first), &budget())?;
    let old_cut = old_ticket.view().source;
    assert_eq!(map_count(&objects, old_cut.owners)?, 2);
    assert_eq!(
        objects.lookup_object(old_cut.owners, &owner_map_key(owner(1)), &mut budget())?,
        Some(a)
    );
    assert_eq!(
        objects.lookup_object(old_cut.owners, &owner_map_key(owner(2)), &mut budget())?,
        Some(ab)
    );
    let old_file = file_member(&engine, &objects, first)?;

    // A sealed empty owner manifest still advances the producer frontier.
    let second = published(block_on(engine.submit(
        request(&engine, &catalog, &objects, first, old_cut, 3, 2, vec![])?,
        &mut budget(),
    ))?)?;
    let new_ticket = engine.pin_view(ViewSelection::Exact(second), &budget())?;
    let new_cut = new_ticket.view().source;
    assert_eq!(new_cut.owners, old_cut.owners);
    assert_eq!(new_cut.paths, old_cut.paths);
    assert_ne!(new_cut.frontiers, old_cut.frontiers);
    assert_eq!(old_ticket.view().source, old_cut);
    let new_file = file_member(&engine, &objects, second)?;
    assert_eq!(new_file.input.root, old_file.input.root);
    assert_ne!(new_file.input.context, old_file.input.context);
    assert_ne!(new_file.artifact, old_file.artifact);

    let old_root: serde_json::Value =
        serde_json::from_slice(&objects.get(old_file.artifact, &mut budget())?.payload)?;
    let new_root: serde_json::Value =
        serde_json::from_slice(&objects.get(new_file.artifact, &mut budget())?.payload)?;
    assert_eq!(new_root.get("owners"), old_root.get("owners"));
    assert_eq!(new_root.get("paths"), old_root.get("paths"));

    let old_paths = query_paths(
        &FilePathQuery {
            member: &old_file,
            prefix: "a",
            after: None,
            limit: 10,
        },
        &mut FileMapReadContext {
            ticket: old_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    let new_paths = query_paths(
        &FilePathQuery {
            member: &new_file,
            prefix: "a",
            after: None,
            limit: 10,
        },
        &mut FileMapReadContext {
            ticket: new_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    assert_eq!(new_paths, old_paths);
    assert_eq!(
        new_paths
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry.owner, entry.source))
            .collect::<Vec<_>>(),
        vec![("a", owner(1), a), ("ab", owner(2), ab)]
    );
    assert!(!new_paths.has_more);
    assert_eq!(
        hits(
            &engine,
            first,
            &objects,
            &member(&engine, &objects, first)?,
            b"needle"
        )?,
        vec![owner(1), owner(2)]
    );
    assert_eq!(
        hits(
            &engine,
            second,
            &objects,
            &member(&engine, &objects, second)?,
            b"needle"
        )?,
        vec![owner(1), owner(2)]
    );
    old_ticket.release()?;
    new_ticket.release()?;
    drop(engine);
    drop(catalog);
    drop(objects);

    let (reopened, objects, _catalog) = kernel(root.path())?;
    for (view, binding) in [(first, old_file), (second, new_file)] {
        let ticket = reopened.pin_view(ViewSelection::Exact(view), &budget())?;
        let paths = query_paths(
            &FilePathQuery {
                member: &binding,
                prefix: "a",
                after: None,
                limit: 10,
            },
            &mut FileMapReadContext {
                ticket: ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        assert_eq!(paths, old_paths);
        ticket.release()?;
    }
    Ok(())
}

/// A terminal retry is a digest/status lookup, not a replay of retained bytes.
/// Fault injection removes only the request object, leaving published roots.
#[test]
fn terminal_replay_survives_pruned_physical_request() -> TestResult {
    use std::fmt::Write as _;
    let temp = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(temp.path())?;
    let initial = configured(&engine, &objects)?;
    let record = stage(&objects, "a.rs", b"immutable needle")?;
    let update = request(
        &engine,
        &catalog,
        &objects,
        initial,
        source(&engine, initial)?,
        2,
        1,
        vec![OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(record),
        }],
    )?;
    let view = published(block_on(engine.submit(update.clone(), &mut budget()))?)?;
    let request_ref = objects
        .identify(
            &serde_json::to_vec(&update)?,
            &match &update.kind {
                UpdateKind::SourceAdvance(transition) => transition.owners.object_children(),
                UpdateKind::SourceResync { .. }
                | UpdateKind::ProjectionAdvance(_)
                | UpdateKind::Reconfigure { .. }
                | UpdateKind::Materialize { .. } => {
                    return Err("expected source request fixture".into());
                }
            },
            &mut budget(),
        )?
        .reference;
    let mut name = String::with_capacity(64);
    for byte in (request_ref.0).0 {
        write!(&mut name, "{byte:02x}")?;
    }
    let shard = format!("{:02x}", (request_ref.0).0.first().ok_or("digest empty")?);
    std::fs::remove_file(
        temp.path()
            .join("native-objects/objects")
            .join(shard)
            .join(name),
    )?;
    assert_eq!(
        published(block_on(engine.submit(update.clone(), &mut budget()))?)?,
        view
    );
    let mut conflict = update;
    if let UpdateKind::SourceAdvance(transition) = &mut conflict.kind {
        transition.owners = OwnerMutationSet {
            root: quanta_index_core::index::MapRoot(None),
            count: 0,
            digest: quanta_index_core::index::OwnerMutationDigest::default().finish(),
        };
        let payload = transition.owner_digest()?;
        for event in &mut transition.events {
            event.payload = payload;
        }
    }
    assert_eq!(
        block_on(engine.submit(conflict, &mut budget())),
        Err(IndexError::DigestConflict)
    );
    assert_eq!(
        hits(
            &engine,
            view,
            &objects,
            &member(&engine, &objects, view)?,
            b"needle"
        )?,
        vec![owner(1)]
    );
    drop(engine);
    drop(objects);
    let (reopened, objects, _reopened_catalog) = kernel(temp.path())?;
    assert_eq!(
        reopened.status(
            operation(2),
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Published(view)
    );
    assert_eq!(
        hits(
            &reopened,
            view,
            &objects,
            &member(&reopened, &objects, view)?,
            b"needle"
        )?,
        vec![owner(1)]
    );
    Ok(())
}

/// Fixed independent token gold through actual Kernel/Catalog publication.
/// Covers Content and flat boolean BM25; regex and product query parity are excluded.
#[test]
fn native_text_statistics_follow_delta_delete_noop_and_pinned_history() -> TestResult {
    use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan};
    use quanta_index_core::index::SourceTextAdmission;
    use quanta_index_lexical::native::{
        Bm25Policy, CaseMode, NativeTextIndex, TextReadContext, TextRecipe,
    };
    use quanta_index_lexical::planner::LexicalPlanner;

    let root = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(root.path().join("native-objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(
        root.path(),
        Duration::from_millis(100),
    )?);
    let admission = Digest([31; 32]);
    let recipe = TextRecipe::new(admission);
    let text_spec = InstanceSpec {
        kind: IndexKind([28; 16]),
        policy: recipe.output_policy(),
        ..spec()
    };
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: text_spec.kind,
            schema: text_spec.schema,
            format: text_spec.format,
        },
        NativeTextIndex::new(recipe),
        |delta| Ok(delta.clone()),
    )?;
    let engine = Kernel::new(
        KernelServices::new(
            objects.clone(),
            objects.clone(),
            catalog.clone(),
            Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
                root.path().join("scratch"),
                16 * 1024 * 1024 * 1024,
            )?),
            Duration::from_secs(30),
        )?,
        registry,
    );
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let profile = engine.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![text_spec.clone()],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let stage_text = |key, path: &str, bytes: &[u8], text| -> TestResult<ObjectRef> {
        Ok(engine.stage_source_text(engine.begin_input(key)?, path, bytes, text, &mut budget())?)
    };
    let admitted = SourceTextAdmission::Utf8 { policy: admission };
    let a = stage_text(
        operation(2),
        "a.rs",
        "Café CAFÉ one one".as_bytes(),
        admitted,
    )?;
    let b = stage_text(operation(2), "b.rs", "cafe\u{301} one".as_bytes(), admitted)?;
    let empty = stage_text(operation(2), "empty.rs", b"", admitted)?;
    let excluded = stage_text(
        operation(2),
        "binary.rs",
        &[0xff],
        SourceTextAdmission::Excluded,
    )?;
    let first = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![
                OwnerMutation {
                    owner: owner(1),
                    expected: None,
                    next: Some(a),
                },
                OwnerMutation {
                    owner: owner(2),
                    expected: None,
                    next: Some(b),
                },
                OwnerMutation {
                    owner: owner(3),
                    expected: None,
                    next: Some(empty),
                },
                OwnerMutation {
                    owner: owner(4),
                    expected: None,
                    next: Some(excluded),
                },
            ],
        )?,
        &mut budget(),
    ))?)?;
    let old_ticket = engine.pin_view(ViewSelection::Exact(first), &budget())?;
    let old_member = member(&engine, &objects, first)?;
    let reader = NativeTextIndex::new(recipe);
    assert!(matches!(
        reader.inspect_term_df(
            &old_member,
            CaseMode::Sensitive,
            &"x".repeat(quanta_index_lexical::native::MAX_TEXT_INSPECT_TERM_BYTES + 1),
            TextReadContext {
                ticket: old_ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        ),
        Err(IndexError::ResourceLimited)
    ));
    let read_stats = |ticket: &quanta_index_core::index::TicketRecord,
                      binding: &MemberBinding|
     -> TestResult<(u64, u64)> {
        let stats = reader.inspect_stats(
            binding,
            TextReadContext {
                ticket,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        Ok((stats.docs, stats.indexed_tokens))
    };
    let read_df = |ticket: &quanta_index_core::index::TicketRecord,
                   binding: &MemberBinding,
                   case,
                   term: &str|
     -> TestResult<u64> {
        Ok(reader.inspect_term_df(
            binding,
            case,
            term,
            TextReadContext {
                ticket,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?)
    };
    assert_eq!(read_stats(old_ticket.record(), &old_member)?, (3, 6));
    assert_eq!(
        read_df(old_ticket.record(), &old_member, CaseMode::Sensitive, "one")?,
        2
    );
    assert_eq!(
        read_df(
            old_ticket.record(),
            &old_member,
            CaseMode::Sensitive,
            "café"
        )?,
        1
    );
    assert_eq!(
        read_df(old_ticket.record(), &old_member, CaseMode::Folded, "café")?,
        2
    );
    let content_plan = |term: &str| -> TestResult<_> {
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword(term.into()));
        Ok(LexicalPlanner::plan(&query)?)
    };
    let one_plan = content_plan("one")?;
    for pattern_type in [
        quanta_index_contract::LqPatternType::Regexp,
        quanta_index_contract::LqPatternType::Literal,
    ] {
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword("one".into()));
        query.options.pattern_type = pattern_type;
        let unsupported = LexicalPlanner::plan(&query)?;
        assert!(matches!(
            reader.search_content(
                &old_member,
                &unsupported,
                &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                1,
                Bm25Policy { k1: 1.2, b: 0.75 },
                TextReadContext {
                    ticket: old_ticket.record(),
                    objects: objects.as_ref(),
                    maps: objects.as_ref(),
                    budget: &mut budget(),
                }
            ),
            Err(IndexError::InvalidInput(_))
        ));
    }
    let ranking_policy = Bm25Policy { k1: 1.2, b: 0.75 };
    let mut ranking_budget = budget();
    let ranked = reader.search_content(
        &old_member,
        &one_plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        2,
        ranking_policy,
        TextReadContext {
            ticket: old_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut ranking_budget,
        },
    )?;
    assert_eq!(ranked.view, first);
    assert_eq!(ranked.artifact, old_member.artifact);
    assert_eq!(ranked.matched, 2);
    // Fixed independent gold: N=3 includes the empty admitted file;
    // df=2, avgdl=2, (tf,dl)=(2,4),(1,2). Binary input is excluded.
    for (hit, (expected_owner, expected_source, expected_score)) in ranked.hits.iter().zip([
        (owner(1), a, 0.504_394_138_702_740_6),
        (owner(2), b, 0.470_003_629_245_735_5),
    ]) {
        assert_eq!(
            (hit.owner, durable_text_source(hit.source)?),
            (expected_owner, expected_source)
        );
        assert!(score_within_16_ulps(hit.score, expected_score));
    }
    assert_eq!(ranked.hits.len(), 2);
    // The typed read port must preserve the published source paths and fixed score gold.
    let read_port = quanta_index_lexical::native::NativeTextReader::new(
        recipe,
        ranking_policy,
        quanta_index_core::index::effort::ResourceNeed {
            cpu_units: 1,
            io_bytes: 64 * 1024,
            memory_bytes: 4 * 1024 * 1024,
            provider_units: 0,
        },
    )?;
    let mut read_budget = budget();
    let answer = quanta_index_core::index::IndexQueryPort::query(
        &read_port,
        &quanta_index_lexical::native::PlannedTextRead::new(
            one_plan.clone(),
            2,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            |bytes, lifetime| read_budget.reserve_memory(bytes, lifetime),
        )?,
        &mut quanta_index_core::index::QueryReadContext {
            ticket: old_ticket.record(),
            instance: &text_spec,
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut read_budget,
        },
    )?;
    assert_eq!(answer.view(), first);
    assert_eq!(
        answer.coverage(),
        &quanta_index_core::index::QueryCoverage::Core
    );
    assert!(answer.exhaustive());
    assert_eq!(answer.result().artifact, old_member.artifact);
    assert_eq!(answer.result().matched, 2);
    assert_eq!(answer.result().hits.len(), 2);
    for (hit, (expected_owner, expected_source, expected_path, expected_score)) in
        answer.result().hits.iter().zip([
            (owner(1), a, "a.rs", 0.504_394_138_702_740_6),
            (owner(2), b, "b.rs", 0.470_003_629_245_735_5),
        ])
    {
        assert_eq!(
            (
                hit.owner,
                durable_text_source(hit.source)?,
                hit.path.as_str()
            ),
            (expected_owner, expected_source, expected_path)
        );
        assert!(score_within_16_ulps(hit.score.get(), expected_score));
    }

    // The service bridge must select the reader from the pinned profile, not a
    // caller descriptor, and preserve the independent score/path gold above.
    let read_need = quanta_index_core::index::effort::ResourceNeed {
        cpu_units: 1,
        io_bytes: 64 * 1024,
        memory_bytes: 4 * 1024 * 1024,
        provider_units: 0,
    };
    let planned = |work: &WorkBudget| {
        quanta_index_lexical::native::PlannedTextRead::new(
            one_plan.clone(),
            2,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            |bytes, lifetime| work.reserve_memory(bytes, lifetime),
        )
    };
    let mut queries = quanta_index_kernel::QueryAdapterRegistry::new();
    queries.register(
        AdapterDescriptor {
            kind: text_spec.kind,
            schema: text_spec.schema,
            format: text_spec.format,
        },
        read_need,
        read_port,
    )?;
    queries.validate_reservation(read_need)?;
    let guard = query_guard(read_need)?;
    let mut bridged_budget = budget();
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(first),
        text_spec.instance,
        planned(&bridged_budget)?,
        &mut bridged_budget,
    )?;
    assert_eq!(prepared.required_resources(), read_need);
    let bridged = prepared.execute(&guard)?;
    assert_eq!(bridged, answer);
    let mut empty_budget = budget();
    assert!(matches!(
        engine
            .prepare_query(
                &queries,
                ViewSelection::Exact(configured),
                text_spec.instance,
                planned(&empty_budget)?,
                &mut empty_budget,
            )?
            .execute(&guard),
        Ok(empty) if empty.result().matched == 0 && empty.result().hits.is_empty()
    ));
    let mut unavailable_budget = budget();
    assert!(matches!(
        engine.prepare_query(
            &queries,
            ViewSelection::Exact(genesis),
            text_spec.instance,
            planned(&unavailable_budget)?,
            &mut unavailable_budget,
        ),
        Err(IndexError::SnapshotUnavailable)
    ));
    let mut wrong_need = read_need;
    wrong_need.cpu_units = 2;
    let mut wrong_budget = budget();
    assert!(matches!(
        engine
            .prepare_query(
                &queries,
                ViewSelection::Exact(first),
                text_spec.instance,
                planned(&wrong_budget)?,
                &mut wrong_budget,
            )?
            .execute(&query_guard(wrong_need)?),
        Err(IndexError::BackendViolation(_))
    ));

    // These values come from the fixed three-document corpus, not another
    // search_content call. Folded `one` and `café` have the same df and per-owner
    // tf; each contributes one of these exact BM25 terms. A failed AND branch
    // contributes no term through an enclosing OR.
    let one_term_gold = [
        (owner(1), a, 0.504_394_138_702_740_6),
        (owner(2), b, 0.470_003_629_245_735_5),
    ];
    let two_term_gold = [
        (owner(1), a, 1.008_788_277_405_481_2),
        (owner(2), b, 0.940_007_258_491_471),
    ];
    for (expr, expected_matched, expected_hits) in [
        (
            LqExpr::Any(vec![
                LqExpr::All(vec![
                    LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                    LqExpr::Leaf(LqLeaf::Keyword("absent".into())),
                ]),
                LqExpr::Leaf(LqLeaf::Keyword("café".into())),
            ]),
            2,
            one_term_gold.as_slice(),
        ),
        (
            LqExpr::All(vec![
                LqExpr::Any(vec![
                    LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                    LqExpr::Leaf(LqLeaf::Keyword("absent".into())),
                ]),
                LqExpr::Leaf(LqLeaf::Keyword("CAFÉ".into())),
            ]),
            2,
            two_term_gold.as_slice(),
        ),
        (
            LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                LqExpr::Leaf(LqLeaf::Keyword("CAFÉ".into())),
            ]),
            2,
            two_term_gold.as_slice(),
        ),
        (
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                LqExpr::Leaf(LqLeaf::Keyword("café".into())),
            ]),
            2,
            two_term_gold.as_slice(),
        ),
        (
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                LqExpr::Leaf(LqLeaf::Keyword("ONE".into())),
            ]),
            2,
            one_term_gold.as_slice(),
        ),
        (
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("CAFÉ".into())),
                LqExpr::Leaf(LqLeaf::Keyword("cafe\u{301}".into())),
            ]),
            2,
            one_term_gold.as_slice(),
        ),
        (
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                LqExpr::Leaf(LqLeaf::Keyword("absent".into())),
            ]),
            2,
            one_term_gold.as_slice(),
        ),
        (
            LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                LqExpr::Leaf(LqLeaf::Keyword("absent".into())),
            ]),
            0,
            &[],
        ),
    ] {
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = expr;
        let boolean = reader.search_content(
            &old_member,
            &LexicalPlanner::plan(&query)?,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            10,
            ranking_policy,
            TextReadContext {
                ticket: old_ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        assert_eq!(boolean.matched, expected_matched);
        assert_eq!(boolean.hits.len(), expected_hits.len());
        for (hit, (expected_owner, expected_source, expected_score)) in
            boolean.hits.iter().zip(expected_hits)
        {
            assert_eq!(
                (hit.owner, durable_text_source(hit.source)?),
                (*expected_owner, *expected_source)
            );
            assert!(score_within_16_ulps(hit.score, *expected_score));
        }
    }
    let mut sensitive = LqQuery::empty(LqSpan::synthetic(0));
    sensitive.expr = LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::Keyword("one".into())),
        LqExpr::Leaf(LqLeaf::Keyword("café".into())),
    ]);
    sensitive.options.case = Some(quanta_index_contract::LqCase::Sensitive);
    let sensitive = reader.search_content(
        &old_member,
        &LexicalPlanner::plan(&sensitive)?,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        10,
        ranking_policy,
        TextReadContext {
            ticket: old_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    let [hit] = sensitive.hits.as_slice() else {
        return Err("sensitive conjunction must select only NFC lowercase owner".into());
    };
    assert_eq!((hit.owner, durable_text_source(hit.source)?), (owner(2), b));
    assert!(score_within_16_ulps(hit.score, 1.450_832_882_257_461_8));
    let top_one = reader.search_content(
        &old_member,
        &one_plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        1,
        ranking_policy,
        TextReadContext {
            ticket: old_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    let [first_ranked, _second_ranked] = ranked.hits.as_slice() else {
        return Err("expected two ranked hits".into());
    };
    assert_eq!(top_one.hits, vec![*first_ranked]);
    // Observe the real FileObjectStore/map read cost through one fully
    // validated posting. visit_term and single-Content BM25 share the same
    // posting_cursor/next_hit path; only ranking's heap work follows the hit.
    // The callback stops before the second owner and has fixed corpus gold.
    let mut first_owner_probe = budget();
    let mut probed = Vec::new();
    let probe = reader.visit_term(
        &old_member,
        CaseMode::Folded,
        "one",
        TextReadContext {
            ticket: old_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut first_owner_probe,
        },
        |hit| {
            probed.push((hit.owner, hit.source));
            Err(IndexError::Cancelled)
        },
    );
    assert!(matches!(probe, Err(IndexError::Cancelled)));
    assert_eq!(probed, vec![(owner(1), a)]);
    let reads_through_first_owner = first_owner_probe.usage().reads;
    assert!(reads_through_first_owner > 0);
    let mut incomplete = WorkBudget::new(
        WorkLimits {
            // Permit exactly the observed first owner. The next posting's
            // entry charge or map/object read must refuse the whole ranking.
            reads: reads_through_first_owner,
            writes: 0,
            bytes: 128 * 1024 * 1024,
            // The nested evaluator charges one program step per candidate;
            // only the measured read boundary should end this query.
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("native text test deadline overflow")?,
    );
    assert!(matches!(
        reader.search_content(
            &old_member,
            &one_plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            1,
            ranking_policy,
            TextReadContext {
                ticket: old_ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut incomplete,
            }
        ),
        Err(IndexError::ResourceLimited)
    ));
    assert_eq!(incomplete.usage().reads, reads_through_first_owner);
    assert!(matches!(
        reader.search_content(
            &old_member,
            &content_plan("one two")?,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            1,
            ranking_policy,
            TextReadContext {
                ticket: old_ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            }
        ),
        Err(IndexError::InvalidInput(_))
    ));
    let a2 = stage_text(operation(3), "a.rs", b"two", admitted)?;
    let second = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            first,
            source(&engine, first)?,
            3,
            2,
            vec![
                OwnerMutation {
                    owner: owner(1),
                    expected: Some(a),
                    next: Some(a2),
                },
                OwnerMutation {
                    owner: owner(2),
                    expected: Some(b),
                    next: None,
                },
            ],
        )?,
        &mut budget(),
    ))?)?;
    let second_member = member(&engine, &objects, second)?;
    let second_ticket = engine.pin_view(ViewSelection::Exact(second), &budget())?;
    assert_eq!(read_stats(second_ticket.record(), &second_member)?, (2, 1));
    let deleted = reader.search_content(
        &second_member,
        &one_plan,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        2,
        ranking_policy,
        TextReadContext {
            ticket: second_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    assert_eq!(deleted.matched, 0);
    assert!(deleted.hits.is_empty());
    let two_ranked = reader.search_content(
        &second_member,
        &content_plan("two")?,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        1,
        ranking_policy,
        TextReadContext {
            ticket: second_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    let [two_hit] = two_ranked.hits.as_slice() else {
        return Err("expected one ranked owner".into());
    };
    assert_eq!(
        (two_hit.owner, durable_text_source(two_hit.source)?),
        (owner(1), a2)
    );
    assert!(score_within_16_ulps(two_hit.score, 0.491_910_902_332_864_4));
    assert_eq!(
        read_df(
            second_ticket.record(),
            &second_member,
            CaseMode::Folded,
            "café"
        )?,
        0
    );
    assert_eq!(
        read_df(
            second_ticket.record(),
            &second_member,
            CaseMode::Sensitive,
            "two"
        )?,
        1
    );
    old_ticket.validate()?;
    assert_eq!(read_stats(old_ticket.record(), &old_member)?, (3, 6));
    assert_eq!(
        reader.search_content(
            &old_member,
            &one_plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            2,
            ranking_policy,
            TextReadContext {
                ticket: old_ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            }
        )?,
        ranked
    );
    let third = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            second,
            source(&engine, second)?,
            4,
            3,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(a2),
                next: Some(a2),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let third_member = member(&engine, &objects, third)?;
    assert_eq!(third_member.artifact, second_member.artifact);
    let third_ticket = engine.pin_view(ViewSelection::Exact(third), &budget())?;
    assert_eq!(read_stats(third_ticket.record(), &third_member)?, (2, 1));
    for (sequence, bytes, admission, expected) in [
        (
            5,
            b"two".to_vec(),
            SourceTextAdmission::Utf8 {
                policy: Digest([32; 32]),
            },
            Refusal::InvalidInput,
        ),
        (6, b"x ".repeat(8193), admitted, Refusal::ResourceLimited),
    ] {
        // Independent refusal cases use distinct streams. A refused event
        // keeps its original payload; rebinding it cannot replace the content.
        let key = OperationKey {
            stream: StreamId([u8::try_from(sequence + 40)?; 16]),
            ..operation(sequence)
        };
        let rejected = stage_text(key, "a.rs", &bytes, admission)?;
        let owners = engine.stage_mutations(
            engine.begin_input(key)?,
            std::iter::once(Ok(OwnerMutation {
                owner: owner(1),
                expected: Some(a2),
                next: Some(rejected),
            })),
            &mut budget(),
        )?;
        let mut transition = SourceTransition {
            expected_source: source(&engine, third)?,
            owners,
            events: Vec::new(),
        };
        transition.events.push(SourceEvent {
            producer: key.producer,
            stream: key.stream,
            stream_epoch: key.stream_epoch,
            predecessor: 0,
            sequence: 1,
            payload: transition.owner_digest()?,
        });
        assert_eq!(
            block_on(engine.submit(
                UpdateRequest {
                    operation: key,
                    expected_view: third,
                    kind: UpdateKind::SourceAdvance(transition),
                },
                &mut budget(),
            ))
            .map_err(|error| format!("native text refusal case {sequence}: {error}"))?,
            SubmitOutcome::Terminal(OperationStatus::Refused(expected)),
        );
        let latest = engine.pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?;
        assert_eq!(latest.view().identity, third);
        assert_eq!(read_stats(latest.record(), &third_member)?, (2, 1));
        latest.release()?;
    }
    let renamed = stage_text(operation(5), "renamed.rs", b"two", admitted)?;
    assert_ne!(renamed, a2);
    let fourth = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            third,
            source(&engine, third)?,
            5,
            4,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(a2),
                next: Some(renamed),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let renamed_member = member(&engine, &objects, fourth)?;
    let renamed_ticket = engine.pin_view(ViewSelection::Exact(fourth), &budget())?;
    assert_eq!(
        read_stats(renamed_ticket.record(), &renamed_member)?,
        (2, 1)
    );
    let renamed_ranking = reader.search_content(
        &renamed_member,
        &content_plan("two")?,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        1,
        ranking_policy,
        TextReadContext {
            ticket: renamed_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    let [renamed_hit] = renamed_ranking.hits.as_slice() else {
        return Err("expected one renamed ranked owner".into());
    };
    assert_eq!(renamed_ranking.view, fourth);
    assert_eq!(renamed_ranking.artifact, renamed_member.artifact);
    assert_eq!(
        (renamed_hit.owner, durable_text_source(renamed_hit.source)?),
        (owner(1), renamed)
    );
    assert_eq!(renamed_hit.score.to_bits(), two_hit.score.to_bits());
    assert_eq!(renamed_ranking.matched, 1);
    third_ticket.validate()?;
    let historical = reader.search_content(
        &third_member,
        &content_plan("two")?,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        1,
        ranking_policy,
        TextReadContext {
            ticket: third_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    let [historical_hit] = historical.hits.as_slice() else {
        return Err("expected one historical ranked owner".into());
    };
    assert_eq!(
        (
            historical_hit.owner,
            durable_text_source(historical_hit.source)?
        ),
        (owner(1), a2)
    );
    assert_eq!(historical_hit.score.to_bits(), two_hit.score.to_bits());
    // Both terms exist, but never in the same owner. This distinguishes
    // conjunction from union without treating an absent term as disjointness.
    let disjoint_source = stage_text(operation(6), "b.rs", b"one", admitted)?;
    let disjoint_view = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            fourth,
            source(&engine, fourth)?,
            6,
            5,
            vec![OwnerMutation {
                owner: owner(2),
                expected: None,
                next: Some(disjoint_source),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let disjoint_member = member(&engine, &objects, disjoint_view)?;
    let disjoint_ticket = engine.pin_view(ViewSelection::Exact(disjoint_view), &budget())?;
    // Fixed OR order: admitted owners 1 and 2 each have one indexed token,
    // one occurrence of their sole term, and df=1 in a 3-document corpus
    // (owner 3 is empty). Both BM25 inputs are N=3, avgdl=2/3, df=tf=dl=1,
    // so both scores equal (44/53)*ln(8/3); the owner-key tie break places
    // stable ID [1;16] before [2;16].
    for (expr, expected_matched, expected_hits) in [
        (
            LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Keyword("two".into())),
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
            ]),
            0,
            vec![],
        ),
        (
            LqExpr::Any(vec![
                LqExpr::Leaf(LqLeaf::Keyword("two".into())),
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
            ]),
            2,
            vec![(owner(1), renamed), (owner(2), disjoint_source)],
        ),
        (
            LqExpr::Any(vec![
                LqExpr::All(vec![
                    LqExpr::Leaf(LqLeaf::Keyword("two".into())),
                    LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                ]),
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
            ]),
            1,
            vec![(owner(2), disjoint_source)],
        ),
        (
            LqExpr::All(vec![
                LqExpr::Any(vec![
                    LqExpr::Leaf(LqLeaf::Keyword("two".into())),
                    LqExpr::Leaf(LqLeaf::Keyword("one".into())),
                ]),
                LqExpr::Leaf(LqLeaf::Keyword("one".into())),
            ]),
            1,
            vec![(owner(2), disjoint_source)],
        ),
    ] {
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = expr;
        let result = reader.search_content(
            &disjoint_member,
            &LexicalPlanner::plan(&query)?,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            10,
            ranking_policy,
            TextReadContext {
                ticket: disjoint_ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        assert_eq!(result.matched, expected_matched);
        assert_eq!(result.hits.len(), expected_hits.len());
        for (hit, expected) in result.hits.iter().zip(expected_hits) {
            assert_eq!((hit.owner, durable_text_source(hit.source)?), expected);
            assert!((hit.score - 0.814_273_342_122_942_6).abs() < 1e-12);
        }
    }
    disjoint_ticket.release()?;
    renamed_ticket.release()?;
    old_ticket.release()?;
    second_ticket.release()?;
    third_ticket.release()?;
    Ok(())
}

/// Repeated source revisions keep one live token set. `WorkUsage` counts logical
/// charged operations, not elapsed time, physical bytes, map nodes, or RSS.
#[test]
fn native_text_fixed_live_set_churn_preserves_current_and_pinned_gold() -> TestResult {
    use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan};
    use quanta_index_core::index::{SourceTextAdmission, TicketRecord, WorkUsage};
    use quanta_index_lexical::{
        native::{Bm25Policy, NativeTextIndex, TextReadContext, TextRecipe},
        planner::LexicalPlanner,
    };

    const REVISIONS: u64 = 32;
    const GAP_BYTES: usize = 33;
    let root = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(root.path().join("native-objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(
        root.path(),
        Duration::from_millis(100),
    )?);
    let admission = Digest([31; 32]);
    let recipe = TextRecipe::new(admission);
    let text_spec = InstanceSpec {
        kind: IndexKind([28; 16]),
        policy: recipe.output_policy(),
        ..spec()
    };
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: text_spec.kind,
            schema: text_spec.schema,
            format: text_spec.format,
        },
        NativeTextIndex::new(recipe),
        |delta| Ok(delta.clone()),
    )?;
    let engine = Kernel::new(
        KernelServices::new(
            objects.clone(),
            objects.clone(),
            catalog.clone(),
            Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
                root.path().join("scratch"),
                16 * 1024 * 1024 * 1024,
            )?),
            Duration::from_secs(30),
        )?,
        registry,
    );
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let profile = engine.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![text_spec],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let admitted = SourceTextAdmission::Utf8 { policy: admission };
    let hot_body = |revision: u64| -> TestResult<Vec<u8>> {
        let mut separator = [b' '; GAP_BYTES];
        if revision > 0 {
            *separator
                .get_mut(usize::try_from(revision - 1)?)
                .ok_or("revision exceeds gap")? = b'\t';
        }
        let mut bytes = b"needle".to_vec();
        bytes.extend_from_slice(&separator);
        bytes.extend_from_slice(b"stable");
        Ok(bytes)
    };
    let stage_text = |key, path: &str, bytes: &[u8]| -> TestResult<ObjectRef> {
        Ok(engine.stage_source_text(
            engine.begin_input(key)?,
            path,
            bytes,
            admitted,
            &mut budget(),
        )?)
    };
    let initial_hot = stage_text(operation(2), "hot.rs", &hot_body(0)?)?;
    let mut initial_mutations = vec![OwnerMutation {
        owner: owner(1),
        expected: None,
        next: Some(initial_hot),
    }];
    for (id, path, bytes) in [
        (2, "a.rs", b"alpha stable".as_slice()),
        (3, "b.rs", b"beta stable".as_slice()),
        (4, "c.rs", b"gamma stable".as_slice()),
    ] {
        initial_mutations.push(OwnerMutation {
            owner: owner(id),
            expected: None,
            next: Some(stage_text(operation(2), path, bytes)?),
        });
    }
    let initial = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            initial_mutations,
        )?,
        &mut budget(),
    ))?)?;
    let mut old_ticket = engine.pin_view(ViewSelection::Exact(initial), &budget())?;
    let old_member = member(&engine, &objects, initial)?;
    let reader = NativeTextIndex::new(recipe);
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Leaf(LqLeaf::Keyword("needle".into()));
    let plan = LexicalPlanner::plan(&query)?;
    let expected_score = (10.0_f64 / 3.0).ln();
    let rank = |ticket: &TicketRecord,
                member: &MemberBinding,
                expected_view: ViewRef,
                expected_source: ObjectRef|
     -> TestResult<WorkUsage> {
        let mut work = budget();
        let ranked = reader.search_content(
            member,
            &plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            4,
            Bm25Policy { k1: 1.2, b: 0.75 },
            TextReadContext {
                ticket,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut work,
            },
        )?;
        let [hit] = ranked.hits.as_slice() else {
            return Err("fixed live set must have one needle hit".into());
        };
        assert_eq!(ranked.view, expected_view);
        assert_eq!(ranked.artifact, member.artifact);
        assert_eq!(ranked.matched, 1);
        assert_eq!(
            (hit.owner, durable_text_source(hit.source)?),
            (owner(1), expected_source)
        );
        let source_record: SourceRecord = serde_json::from_slice(
            &objects
                .get(durable_text_source(hit.source)?, &mut budget())?
                .payload,
        )?;
        assert_eq!(source_record.path, "hot.rs");
        // Independent BM25 gold: N=4, df=1, tf=1, dl=avgdl=2.
        assert!(score_within_16_ulps(hit.score, expected_score));
        Ok(work.usage())
    };

    let mut current = initial;
    let mut hot_source = initial_hot;
    let mut update_usage = Vec::<(u64, WorkUsage)>::new();
    let mut current_query_usage = Vec::<(u64, WorkUsage)>::new();
    current_query_usage.push((
        0,
        rank(old_ticket.record(), &old_member, initial, initial_hot)?,
    ));
    for revision in 1..=REVISIONS {
        old_ticket.renew()?;
        let key = operation(revision + 2);
        let next_source = stage_text(key, "hot.rs", &hot_body(revision)?)?;
        assert_ne!(next_source, hot_source);
        let transition = request(
            &engine,
            &catalog,
            &objects,
            current,
            source(&engine, current)?,
            revision + 2,
            revision + 1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(hot_source),
                next: Some(next_source),
            }],
        )?;
        // Source upload and manifest staging use separate budgets in every
        // revision; only the submit budget is recorded here.
        let mut work = budget();
        current = published(block_on(engine.submit(transition, &mut work))?)?;
        update_usage.push((revision, work.usage()));
        hot_source = next_source;
        if revision == 8 || revision == REVISIONS {
            old_ticket.renew()?;
            let current_ticket = engine.pin_view(ViewSelection::Exact(current), &budget())?;
            let current_member = member(&engine, &objects, current)?;
            current_query_usage.push((
                revision,
                rank(
                    current_ticket.record(),
                    &current_member,
                    current,
                    hot_source,
                )?,
            ));
            current_ticket.release()?;
            let _old_usage = rank(old_ticket.record(), &old_member, initial, initial_hot)?;
        }
    }
    // Equal live keys and token statistics give the same logical query-read
    // work even as immutable historical revisions accumulate.
    let [
        (0, initial_usage),
        (8, intermediate_usage),
        (REVISIONS, final_usage),
    ] = current_query_usage.as_slice()
    else {
        return Err("query measurements must cover H=0, H=8, and H=32".into());
    };
    assert!(initial_usage.reads > 0);
    assert_eq!(initial_usage.reads, intermediate_usage.reads);
    assert_eq!(initial_usage.reads, final_usage.reads);
    assert_eq!(update_usage.len(), usize::try_from(REVISIONS)?);
    assert!(
        update_usage
            .iter()
            .all(|(_, usage)| usage.reads > 0 && usage.writes > 0)
    );
    old_ticket.validate()?;
    old_ticket.release()?;
    Ok(())
}

#[test]
fn native_literal_plan_rejects_mismatched_source_context_before_writes() -> TestResult {
    use quanta_index_core::index::{
        BaseMember, DeltaInput, InputBinding, MapRoot, OwnerMutationDigest, PlanContext,
        PlanDecision, SourceDelta, ports::IndexBuildPort,
    };
    let empty = SourceCut {
        owners: MapRoot(None),
        paths: MapRoot(None),
        frontiers: MapRoot(None),
    };
    let output = MapRoot(Some(ObjectRef(Digest([21; 32]))));
    let other = MapRoot(Some(ObjectRef(Digest([22; 32]))));
    let input = InputBinding {
        root: MapRoot(None),
        schema: spec().schema,
        policy: spec().policy,
        context: Digest([23; 32]),
    };
    let base = MemberBinding {
        instance: spec().instance,
        format: spec().format,
        artifact: ObjectRef(Digest([24; 32])),
        input,
        logical_commitment: Digest([25; 32]),
    };
    let delta = SourceDelta {
        prior: empty,
        next: SourceCut {
            owners: output,
            ..empty
        },
        changed: OwnerMutationSet {
            root: MapRoot(None),
            count: 0,
            digest: OwnerMutationDigest::default().finish(),
        },
    };
    let valid = PlanContext {
        instance: spec(),
        source: delta.next,
        input: InputBinding {
            root: output,
            ..input
        },
    };
    assert!(matches!(
        NativeLiteralIndex.plan(
            BaseMember::Present(Box::new(base.clone())),
            DeltaInput::Patch(delta.clone()),
            &valid,
            &mut budget()
        )?,
        PlanDecision::Incremental(_)
    ));
    for invalid in [
        PlanContext {
            source: SourceCut {
                owners: other,
                ..empty
            },
            ..valid.clone()
        },
        PlanContext {
            input: InputBinding {
                root: other,
                ..input
            },
            ..valid
        },
    ] {
        assert!(matches!(
            NativeLiteralIndex.plan(
                BaseMember::Present(Box::new(base.clone())),
                DeltaInput::Patch(delta.clone()),
                &invalid,
                &mut budget()
            ),
            Err(IndexError::Conflict)
        ));
    }
    let invalid_bootstrap = PlanContext {
        instance: spec(),
        source: empty,
        input: InputBinding {
            root: output,
            ..input
        },
    };
    assert!(matches!(
        NativeLiteralIndex.plan(
            BaseMember::Absent,
            DeltaInput::Bootstrap,
            &invalid_bootstrap,
            &mut budget()
        )?,
        PlanDecision::RebuildRequired
    ));
    Ok(())
}

#[test]
fn native_reconfigure_bootstraps_async_and_retires_work_atomically() -> TestResult {
    use quanta_index_core::index::WorkStatus;

    // Removal spans a cursor page. Activation and policy replacements cover
    // retirement followed by a new output identity for the same instance.
    for (count, replace_with_required, replace_policy) in
        [(129u32, false, false), (3, true, false), (3, false, true)]
    {
        let root = tempfile::tempdir()?;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (engine, objects, catalog) = owner_kernel(root.path(), calls.clone())?;
        let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
        let mut mutations = Vec::new();
        for id in 1..=count {
            let next = engine.stage_source(
                engine.begin_input(operation(1))?,
                &format!("src/{id:03}.rs"),
                b"needle",
                &mut budget(),
            )?;
            mutations.push(OwnerMutation {
                owner: large_owner(id),
                expected: None,
                next: Some(next),
            });
        }
        let source_only = published(block_on(engine.submit(
            request(
                &engine,
                &catalog,
                &objects,
                genesis,
                source(&engine, genesis)?,
                1,
                1,
                mutations,
            )?,
            &mut budget(),
        ))?)?;
        let submit_profile =
            |view, sequence, instances, work: &mut WorkBudget| -> TestResult<SubmitOutcome> {
                let key = operation(sequence);
                let profile = engine.stage_profile(
                    engine.begin_input(key)?,
                    &IndexProfile { instances },
                    &mut budget(),
                )?;
                Ok(block_on(engine.submit(
                    UpdateRequest {
                        operation: key,
                        expected_view: view,
                        kind: UpdateKind::Reconfigure { profile },
                    },
                    work,
                ))?)
            };
        let async_view = published(submit_profile(
            source_only,
            2,
            vec![async_spec()],
            &mut budget(),
        )?)?;
        let ready = engine.ready_work(workspace(), 1, 256, &mut budget())?;
        assert_eq!(ready.len(), usize::try_from(count)?);
        let completed_intent = *ready.first().ok_or("completed owner intent missing")?;
        let completed_claim =
            engine.claim_work(&completed_intent, Duration::from_secs(120), &budget())?;
        let completed = published(block_on(engine.submit(
            UpdateRequest {
                operation: operation(3),
                expected_view: async_view,
                kind: UpdateKind::Materialize {
                    claim: completed_claim,
                },
            },
            &mut budget(),
        ))?)?;
        let completed_status = engine.work_status(&completed_intent, &budget())?;
        assert!(matches!(completed_status, WorkStatus::Completed(_)));
        let claimed_intent = *ready.get(1).ok_or("claimed owner intent missing")?;
        let claim = engine.claim_work(&claimed_intent, Duration::from_secs(120), &budget())?;
        let pending_intent = *ready.get(2).ok_or("pending owner intent missing")?;
        assert_eq!(
            engine.work_status(&pending_intent, &budget())?,
            WorkStatus::Pending
        );
        let ready_before = engine.ready_work(workspace(), 1, 256, &mut budget())?;
        let prior_coverage = coverage(&engine, &objects, completed)?;
        let required = InstanceSpec {
            instance: IndexInstanceId([77; 16]),
            kind: IndexKind([78; 16]),
            group: PublicationGroupId([79; 16]),
            ..spec()
        };
        let expanded = published(submit_profile(
            completed,
            4,
            vec![async_spec(), required.clone()],
            &mut budget(),
        )?)?;
        assert_eq!(coverage(&engine, &objects, expanded)?, prior_coverage);
        assert_eq!(
            engine.ready_work(workspace(), 1, 256, &mut budget())?,
            ready_before
        );
        assert_eq!(
            engine.work_status(&claimed_intent, &budget())?,
            WorkStatus::Claimed(claim)
        );
        let old_ticket = engine.pin_view(ViewSelection::Exact(expanded), &budget())?;
        let mut target = vec![required];
        if replace_with_required {
            target.push(InstanceSpec {
                kind: IndexKind([78; 16]),
                ..spec()
            });
        } else if replace_policy {
            target.push(InstanceSpec {
                policy: Digest([88; 32]),
                ..async_spec()
            });
        }
        let mut limited = WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 512 * 1024 * 1024,
                mutations: 10,
            },
            Instant::now()
                .checked_add(Duration::from_secs(120))
                .ok_or("native reconfigure test deadline overflow")?,
        );
        if !replace_with_required && !replace_policy {
            assert_eq!(
                submit_profile(expanded, 5, target.clone(), &mut limited)?,
                SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::ResourceLimited))
            );
            let latest = engine.pin_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )?;
            assert_eq!(latest.view().identity, expanded);
            latest.release()?;
            assert_eq!(
                engine.work_status(&claimed_intent, &budget())?,
                WorkStatus::Claimed(claim)
            );
            assert_eq!(
                engine.work_status(&pending_intent, &budget())?,
                WorkStatus::Pending
            );
            assert_eq!(
                engine.ready_work(workspace(), 1, 256, &mut budget())?,
                ready_before
            );
        } else if replace_policy {
            let unsafe_spec = InstanceSpec {
                group: PublicationGroupId([89; 16]),
                policy: Digest([88; 32]),
                ..async_spec()
            };
            assert_eq!(
                submit_profile(expanded, 5, vec![unsafe_spec], &mut budget())?,
                SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::RebuildRequired))
            );
            let latest = engine.pin_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )?;
            assert_eq!(latest.view().identity, expanded);
            latest.release()?;
            assert_eq!(
                engine.work_status(&claimed_intent, &budget())?,
                WorkStatus::Claimed(claim)
            );
            assert_eq!(
                engine.ready_work(workspace(), 1, 256, &mut budget())?,
                ready_before
            );
        }
        let removed = published(submit_profile(expanded, 6, target, &mut budget())?)?;
        assert!(removed.sequence > expanded.sequence);
        let replacement_work = engine.ready_work(workspace(), 1, 256, &mut budget())?;
        if replace_policy {
            assert_eq!(replacement_work.len(), usize::try_from(count)?);
            for intent in &replacement_work {
                let quanta_index_core::index::WorkScope::Owner { expected, .. } = intent.scope
                else {
                    return Err("source bootstrap created non-owner work".into());
                };
                assert_eq!(expected.derivation_policy, Digest([88; 32]));
                assert!(!ready.contains(intent));
                assert_eq!(engine.work_status(intent, &budget())?, WorkStatus::Pending);
            }
        } else {
            assert!(replacement_work.is_empty());
        }
        assert_eq!(
            engine.work_status(&claimed_intent, &budget())?,
            WorkStatus::Superseded
        );
        assert_eq!(
            engine.work_status(&pending_intent, &budget())?,
            WorkStatus::Superseded
        );
        assert_eq!(
            engine.work_status(&completed_intent, &budget())?,
            completed_status
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        old_ticket.validate()?;
        assert_eq!(coverage(&engine, &objects, expanded)?, prior_coverage);
        old_ticket.release()?;
        assert!(matches!(
            block_on(engine.submit(
                UpdateRequest {
                    operation: operation(7),
                    expected_view: removed,
                    kind: UpdateKind::Materialize { claim },
                },
                &mut budget()
            ))?,
            SubmitOutcome::Terminal(OperationStatus::Refused(_))
        ));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    Ok(())
}

fn owned_policy() -> quanta_index_core::index::OwnedWorkPolicy {
    quanta_index_core::index::OwnedWorkPolicy {
        dispatch_limits: WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        max_dispatches: 3,
        total_timeout_ms: 60_000,
        lease_ms: 30_000,
        retry_backoff_ms: 10,
    }
}

fn staged_owned_job(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    catalog: &SqliteCatalog,
) -> TestResult<quanta_index_core::index::WorkIntent> {
    let configured = configured_owner(engine, objects)?;
    let record = stage(objects, "owned.rs", b"owned work")?;
    let _view = published(block_on(engine.submit(
        request(
            engine,
            catalog,
            objects,
            configured,
            source(engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(record),
            }],
        )?,
        &mut budget(),
    ))?)?;
    intent_for(engine, owner(1))
}

#[test]
fn native_work_status_forwards_caller_cancellation() -> TestResult {
    use quanta_index_core::index::WorkStatus;

    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls)?;
    let intent = staged_owned_job(&engine, &objects, &catalog)?;
    let cancelled = budget();
    cancelled.request().cancel_handle().cancel();

    assert_eq!(
        engine.work_status(&intent, &cancelled),
        Err(IndexError::Cancelled)
    );
    assert_eq!(engine.work_status(&intent, &budget())?, WorkStatus::Pending);
    Ok(())
}

#[test]
fn native_manual_claim_and_renew_forward_caller_cancellation() -> TestResult {
    use quanta_index_core::index::WorkStatus;

    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls)?;
    let intent = staged_owned_job(&engine, &objects, &catalog)?;

    let cancelled_claim = budget();
    cancelled_claim.request().cancel_handle().cancel();
    assert_eq!(
        engine.claim_work(&intent, Duration::from_secs(30), &cancelled_claim),
        Err(IndexError::Cancelled)
    );
    assert_eq!(engine.work_status(&intent, &budget())?, WorkStatus::Pending);

    let claim = engine.claim_work(&intent, Duration::from_secs(30), &budget())?;
    let cancelled_renew = budget();
    cancelled_renew.request().cancel_handle().cancel();
    assert_eq!(
        engine.renew_work(&claim, Duration::from_secs(30), &cancelled_renew),
        Err(IndexError::Cancelled)
    );
    assert_eq!(
        engine.work_status(&intent, &budget())?,
        WorkStatus::Claimed(claim)
    );
    let renewed = engine.renew_work(&claim, Duration::from_secs(30), &budget())?;
    assert_eq!(renewed.fence, claim.fence);
    Ok(())
}

#[test]
fn native_owned_assignment_survives_reopen_and_completes_original_receipt() -> TestResult {
    use quanta_index_core::index::{OwnedWorkReport, WorkStatus};
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls.clone())?;
    let intent = staged_owned_job(&engine, &objects, &catalog)?;
    let assigned = engine
        .acquire_owned_work(&intent, owned_policy(), &budget())?
        .ok_or("missing assignment")?;
    let key = assigned.request.operation;
    assert!(matches!(
        engine.begin_input(key),
        Err(IndexError::InvalidInput(_))
    ));
    assert!(matches!(
        engine.resume_dispatch(key, &mut budget()),
        Err(IndexError::InvalidInput(_))
    ));
    assert!(matches!(
        block_on(engine.resume(key, &mut budget())),
        Err(IndexError::InvalidInput(_))
    ));
    assert!(matches!(
        block_on(engine.submit(assigned.request.clone(), &mut budget())),
        Err(IndexError::InvalidInput(_))
    ));
    assert!(matches!(
        engine.claim_work(&intent, Duration::from_secs(1), &budget()),
        Err(IndexError::InvalidInput(_))
    ));
    drop(engine);
    drop(objects);
    drop(catalog);
    let (engine, objects, catalog) = owner_kernel(root.path(), calls.clone())?;
    let changed_policy = quanta_index_core::index::OwnedWorkPolicy {
        max_dispatches: 10,
        ..owned_policy()
    };
    let recovered = engine
        .acquire_owned_work(&intent, changed_policy, &budget())?
        .ok_or("missing recovery")?;
    assert_eq!(recovered.request, assigned.request);
    assert_eq!(recovered.policy, assigned.policy);
    assert_eq!(recovered.deadline_ms, assigned.deadline_ms);
    let grant = require_owned_grant(engine.begin_owned_work(&recovered)?, "missing grant")?;
    assert_eq!(grant.ordinal, 1);
    let UpdateKind::Materialize { claim } = &grant.assignment.request.kind else {
        return Err("expected owned materialization grant".into());
    };
    assert!(matches!(
        engine.renew_work(claim, Duration::from_secs(1), &budget()),
        Err(IndexError::InvalidInput(_))
    ));
    let mut stale_grant = grant.clone();
    stale_grant.ordinal = 2;
    assert!(matches!(
        engine.renew_owned_work(&stale_grant),
        Err(IndexError::WorkFenceLost)
    ));
    let renewed = engine.renew_owned_work(&grant)?;
    assert_eq!(renewed.fence, claim.fence);
    assert!(renewed.lease_until_ms <= grant.assignment.deadline_ms);
    let mut altered = grant.assignment.request.clone();
    altered.expected_view.sequence += 1;
    assert!(matches!(
        block_on(engine.submit_owned_work(altered, &mut budget())),
        Err(IndexError::DigestConflict)
    ));
    // Owned identity failure retains priority over generic workspace checks.
    let mut altered = grant.assignment.request.clone();
    altered.expected_view.epoch += 1;
    assert_eq!(
        block_on(engine.submit_owned_work(altered, &mut budget())),
        Err(IndexError::DigestConflict)
    );
    let mut altered = grant.assignment.request.clone();
    let UpdateKind::Materialize { claim } = &mut altered.kind else {
        return Err("expected altered owned claim".into());
    };
    claim.intent.epoch += 1;
    assert_eq!(
        block_on(engine.submit_owned_work(altered, &mut budget())),
        Err(IndexError::DigestConflict)
    );
    assert_eq!(
        engine.status(
            key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Unknown
    );
    let mut external = grant.assignment.request.clone();
    external.operation = operation(99);
    assert!(matches!(
        block_on(engine.submit(external, &mut budget())),
        Err(IndexError::WorkFenceLost)
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let _sql_profile = native_catalog_profile::Guard::start()?;
    native_catalog_profile::attach(&catalog)?;
    let sql_before = native_catalog_profile::snapshot()?;
    let complete = published(block_on(
        engine.submit_owned_work(grant.assignment.request.clone(), &mut budget()),
    )?)?;
    let sql = native_catalog_profile::since(sql_before)?.ok_or("missing owned SQL profile")?;
    // Preserve durable validation and atomically seal/admit the owned request.
    assert!(sql.commit.calls <= 16, "owned SQL commits: {sql:?}");
    // Publication is durable before the manager receives or reconciles an ACK.
    drop(engine);
    drop(objects);
    drop(catalog);
    let (engine, _objects, _catalog) = owner_kernel(root.path(), calls.clone())?;
    assert_eq!(
        engine.status(
            key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Published(complete)
    );
    assert_eq!(
        engine.work_status(&intent, &budget())?,
        WorkStatus::Completed(complete)
    );
    engine.finish_owned_work(&grant, OwnedWorkReport::Reconcile)?;
    assert_eq!(
        engine.ready_owned_work(workspace(), 1, 256, &mut budget())?,
        OwnedReadyOutcome::Ready(Vec::new())
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn native_owned_quota_blocks_without_reset() -> TestResult {
    use quanta_index_core::index::{OwnedWorkReport, WorkStatus};
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls)?;
    let intent = staged_owned_job(&engine, &objects, &catalog)?;
    let policy = quanta_index_core::index::OwnedWorkPolicy {
        max_dispatches: 1,
        ..owned_policy()
    };
    let assignment = engine
        .acquire_owned_work(&intent, policy, &budget())?
        .ok_or("missing assignment")?;
    let grant = require_owned_grant(engine.begin_owned_work(&assignment)?, "missing grant")?;
    assert_eq!(
        engine.begin_owned_work(&assignment)?,
        OwnedWorkBegin::Deferred(OwnedWorkDefer::Active)
    );
    engine.finish_owned_work(&grant, OwnedWorkReport::Interrupted)?;
    std::thread::sleep(Duration::from_millis(20));
    let retry = engine.acquire_owned_work(&intent, owned_policy(), &budget())?;
    if let Some(retry) = retry {
        assert_eq!(retry.policy, policy);
        assert_eq!(
            engine.begin_owned_work(&retry)?,
            OwnedWorkBegin::Terminal(OwnedWorkTerminal::Blocked(Refusal::ResourceLimited))
        );
    }
    assert_eq!(
        engine.work_status(&intent, &budget())?,
        WorkStatus::Blocked(Refusal::ResourceLimited)
    );
    assert_eq!(
        engine.ready_owned_work(workspace(), 1, 256, &mut budget())?,
        OwnedReadyOutcome::Ready(Vec::new())
    );
    assert!(
        engine
            .ready_work(workspace(), 1, 256, &mut budget())?
            .is_empty()
    );
    Ok(())
}

#[test]
fn native_owned_stale_report_cannot_finish_a_new_dispatch() -> TestResult {
    use quanta_index_core::index::{OwnedWorkReport, WorkStatus};
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls)?;
    let intent = staged_owned_job(&engine, &objects, &catalog)?;
    let assigned = engine
        .acquire_owned_work(&intent, owned_policy(), &budget())?
        .ok_or("missing assignment")?;
    let first = require_owned_grant(engine.begin_owned_work(&assigned)?, "missing first grant")?;
    engine.finish_owned_work(&first, OwnedWorkReport::Interrupted)?;
    std::thread::sleep(Duration::from_millis(20));
    let assigned = engine
        .acquire_owned_work(&intent, owned_policy(), &budget())?
        .ok_or("missing retry")?;
    let second = require_owned_grant(engine.begin_owned_work(&assigned)?, "missing second grant")?;
    assert_eq!(second.ordinal, 2);
    assert!(matches!(
        engine.finish_owned_work(&first, OwnedWorkReport::Failed(Refusal::InvalidInput)),
        Err(IndexError::WorkFenceLost | IndexError::FenceLost | IndexError::Conflict)
    ));
    assert!(matches!(
        engine.work_status(&intent, &budget())?,
        WorkStatus::Claimed(_)
    ));
    assert_eq!(
        engine.begin_owned_work(&assigned)?,
        OwnedWorkBegin::Deferred(OwnedWorkDefer::Active)
    );
    engine.finish_owned_work(&second, OwnedWorkReport::Failed(Refusal::InvalidInput))?;
    assert_eq!(
        engine.work_status(&intent, &budget())?,
        WorkStatus::Blocked(Refusal::InvalidInput)
    );
    assert_eq!(
        engine.ready_owned_work(workspace(), 1, 256, &mut budget())?,
        OwnedReadyOutcome::Ready(Vec::new())
    );
    Ok(())
}

#[test]
fn native_owned_pump_does_not_take_expired_manual_claims() -> TestResult {
    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), calls)?;
    let intent = staged_owned_job(&engine, &objects, &catalog)?;
    let manual = engine.claim_work(&intent, Duration::from_millis(1), &budget())?;
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(
        engine.ready_owned_work(workspace(), 1, 256, &mut budget())?,
        OwnedReadyOutcome::Ready(Vec::new())
    );
    assert!(matches!(
        engine.acquire_owned_work(&intent, owned_policy(), &budget()),
        Err(IndexError::InvalidInput(_))
    ));
    let renewed_manual = engine.claim_work(&intent, Duration::from_secs(1), &budget())?;
    assert_ne!(manual.fence, renewed_manual.fence);
    Ok(())
}

struct QueryTestScheduler;

impl quanta_index_core::index::effort::ResourceScheduler for QueryTestScheduler {
    type Token = ();

    fn try_reserve(
        &self,
        _need: quanta_index_core::index::effort::ResourceNeed,
    ) -> Option<Self::Token> {
        Some(())
    }

    fn release(&self, _token: Self::Token) {}
}

fn query_guard(
    need: quanta_index_core::index::effort::ResourceNeed,
) -> TestResult<quanta_index_core::index::effort::ReservationGuard<QueryTestScheduler>> {
    quanta_index_core::index::effort::ReservationGuard::try_acquire(
        Arc::new(QueryTestScheduler),
        need,
    )
    .ok_or_else(|| "test scheduler refused reservation".into())
}

struct CountingPinnedQueryPort {
    need: quanta_index_core::index::effort::ResourceNeed,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl quanta_index_core::index::IndexQueryPort<(), Option<ObjectRef>> for CountingPinnedQueryPort {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        if *instance != spec() {
            return Err(IndexError::InvalidInput("unexpected query instance".into()));
        }
        Ok(())
    }

    fn reservation(
        &self,
        _request: &(),
    ) -> Result<quanta_index_core::index::effort::ResourceNeed, IndexError> {
        Ok(self.need)
    }

    fn query(
        &self,
        _request: &(),
        context: &mut quanta_index_core::index::QueryReadContext<'_>,
    ) -> Result<quanta_index_core::index::QueryOutcome<Option<ObjectRef>>, IndexError> {
        let _prior = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let source = context.maps.lookup_object(
            context.ticket.view.source.owners,
            &owner_map_key(owner(1)),
            context.budget,
        )?;
        quanta_index_core::index::QueryOutcome::core_scanned(
            context.ticket,
            context.instance,
            true,
            source,
        )
    }
}

struct DynamicNeedQueryPort {
    query_calls: Arc<std::sync::atomic::AtomicUsize>,
    drift: Arc<std::sync::atomic::AtomicBool>,
}

impl quanta_index_core::index::IndexQueryPort<quanta_index_core::index::effort::ResourceNeed, ()>
    for DynamicNeedQueryPort
{
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        if *instance != spec() {
            return Err(IndexError::InvalidInput("unexpected query instance".into()));
        }
        Ok(())
    }

    fn reservation(
        &self,
        request: &quanta_index_core::index::effort::ResourceNeed,
    ) -> Result<quanta_index_core::index::effort::ResourceNeed, IndexError> {
        let mut need = *request;
        if self.drift.load(std::sync::atomic::Ordering::SeqCst) {
            need.cpu_units = need
                .cpu_units
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        Ok(need)
    }

    fn query(
        &self,
        _request: &quanta_index_core::index::effort::ResourceNeed,
        context: &mut quanta_index_core::index::QueryReadContext<'_>,
    ) -> Result<quanta_index_core::index::QueryOutcome<()>, IndexError> {
        let _prior = self
            .query_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        quanta_index_core::index::QueryOutcome::core_scanned(
            context.ticket,
            context.instance,
            true,
            (),
        )
    }
}

struct FaultQueryPort {
    need: quanta_index_core::index::effort::ResourceNeed,
    fault: QueryFault,
}

enum QueryFault {
    WrongView,
    Cancel(quanta_index_core::CancelHandleV1),
}

impl quanta_index_core::index::IndexQueryPort<(), ()> for FaultQueryPort {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        if *instance != spec() {
            return Err(IndexError::InvalidInput("unexpected query instance".into()));
        }
        Ok(())
    }

    fn reservation(
        &self,
        _request: &(),
    ) -> Result<quanta_index_core::index::effort::ResourceNeed, IndexError> {
        Ok(self.need)
    }

    fn query(
        &self,
        _request: &(),
        context: &mut quanta_index_core::index::QueryReadContext<'_>,
    ) -> Result<quanta_index_core::index::QueryOutcome<()>, IndexError> {
        let mut ticket = context.ticket.clone();
        match &self.fault {
            QueryFault::WrongView => {
                ticket.view.identity.sequence = ticket
                    .view
                    .identity
                    .sequence
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            QueryFault::Cancel(handle) => handle.cancel(),
        }
        quanta_index_core::index::QueryOutcome::core_scanned(&ticket, context.instance, true, ())
    }
}

fn native_pin_count(catalog: &SqliteCatalog) -> TestResult<i64> {
    Ok(rusqlite::Connection::open(catalog.path())?.query_row(
        "SELECT COUNT(*) FROM native_pin_v1",
        [],
        |row| row.get(0),
    )?)
}

#[test]
fn prepared_query_uses_request_sized_need_within_reader_ceiling() -> TestResult {
    use quanta_index_core::index::effort::ResourceNeed;

    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let selected = configured(&engine, &objects)?;
    let query_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let drift = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ceiling = ResourceNeed {
        cpu_units: 2,
        io_bytes: 2048,
        memory_bytes: 2048,
        provider_units: 1,
    };
    let actual = ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 1024,
        provider_units: 0,
    };
    let mut queries = quanta_index_kernel::QueryAdapterRegistry::new();
    queries.register(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        ceiling,
        DynamicNeedQueryPort {
            query_calls: query_calls.clone(),
            drift: drift.clone(),
        },
    )?;
    let invalid = [
        ResourceNeed {
            cpu_units: 0,
            ..actual
        },
        ResourceNeed {
            io_bytes: 0,
            ..actual
        },
        ResourceNeed {
            memory_bytes: 0,
            ..actual
        },
        ResourceNeed::default(),
        ResourceNeed {
            cpu_units: 3,
            ..actual
        },
        ResourceNeed {
            io_bytes: 2049,
            ..actual
        },
        ResourceNeed {
            memory_bytes: 2049,
            ..actual
        },
        ResourceNeed {
            provider_units: 2,
            ..actual
        },
    ];
    for requested in invalid {
        let mut rejected_budget = budget();
        assert!(matches!(
            engine.prepare_query(
                &queries,
                ViewSelection::Exact(selected),
                spec().instance,
                requested,
                &mut rejected_budget,
            ),
            Err(IndexError::BackendViolation(_))
        ));
        assert_eq!(query_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(native_pin_count(&catalog)?, 0);
    }
    let mut query_budget = budget();
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(selected),
        spec().instance,
        actual,
        &mut query_budget,
    )?;
    assert_eq!(prepared.required_resources(), actual);
    assert_eq!(native_pin_count(&catalog)?, 1);
    let outcome = prepared.execute(&query_guard(actual)?)?;
    assert_eq!(outcome.view(), selected);
    assert_eq!(query_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(native_pin_count(&catalog)?, 0);

    let mut drift_budget = budget();
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(selected),
        spec().instance,
        actual,
        &mut drift_budget,
    )?;
    assert_eq!(prepared.required_resources(), actual);
    assert_eq!(native_pin_count(&catalog)?, 1);
    drift.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        prepared.execute(&query_guard(actual)?),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(query_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn prepared_query_keeps_exact_old_view_through_source_advance() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let configured = configured(&engine, &objects)?;
    let old_record = stage(&objects, "a.rs", b"old")?;
    let selected = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(old_record),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let need = quanta_index_core::index::effort::ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 1024,
        provider_units: 0,
    };
    let mut queries = quanta_index_kernel::QueryAdapterRegistry::new();
    queries.register(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        need,
        CountingPinnedQueryPort {
            need,
            calls: calls.clone(),
        },
    )?;
    let mut query_budget = budget();
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(selected),
        spec().instance,
        (),
        &mut query_budget,
    )?;
    assert_eq!(prepared.required_resources(), need);
    assert_eq!(native_pin_count(&catalog)?, 1);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    drop(queries);

    let new_record = stage(&objects, "a.rs", b"new")?;
    let advanced = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            selected,
            source(&engine, selected)?,
            3,
            2,
            vec![OwnerMutation {
                owner: owner(1),
                expected: Some(old_record),
                next: Some(new_record),
            }],
        )?,
        &mut budget(),
    ))?)?;
    assert_ne!(selected, advanced);
    let guard = query_guard(prepared.required_resources())?;
    let outcome = prepared.execute(&guard)?;
    assert_eq!(outcome.view(), selected);
    assert_eq!(*outcome.result(), Some(old_record));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(native_pin_count(&catalog)?, 0);
    // The selected profile has two instances; only preparation decodes it.
    assert_eq!(query_budget.usage().mutations, 2);
    Ok(())
}

#[test]
fn prepared_query_wrong_reservation_never_calls_reader_and_releases_pin() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let selected = configured(&engine, &objects)?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let need = quanta_index_core::index::effort::ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 1024,
        provider_units: 0,
    };
    let mut queries = quanta_index_kernel::QueryAdapterRegistry::new();
    queries.register(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        need,
        CountingPinnedQueryPort {
            need,
            calls: calls.clone(),
        },
    )?;
    let mut query_budget = budget();
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(selected),
        spec().instance,
        (),
        &mut query_budget,
    )?;
    assert_eq!(native_pin_count(&catalog)?, 1);
    let wrong = query_guard(quanta_index_core::index::effort::ResourceNeed {
        cpu_units: 2,
        ..need
    })?;
    assert!(matches!(
        prepared.execute(&wrong),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn prepared_query_cancel_and_abort_release_pin_without_reader_call() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let selected = configured(&engine, &objects)?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let need = quanta_index_core::index::effort::ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 1024,
        provider_units: 0,
    };
    let mut queries = quanta_index_kernel::QueryAdapterRegistry::new();
    queries.register(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        need,
        CountingPinnedQueryPort {
            need,
            calls: calls.clone(),
        },
    )?;
    let mut abort_budget = budget();
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(selected),
        spec().instance,
        (),
        &mut abort_budget,
    )?;
    assert_eq!(native_pin_count(&catalog)?, 1);
    prepared.abort()?;
    assert_eq!(native_pin_count(&catalog)?, 0);

    let mut drop_budget = budget();
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(selected),
        spec().instance,
        (),
        &mut drop_budget,
    )?;
    assert_eq!(native_pin_count(&catalog)?, 1);
    drop(prepared);
    assert_eq!(native_pin_count(&catalog)?, 0);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    let request = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
    let cancel = request.cancel_handle();
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 0,
            bytes: 512 * 1024 * 1024,
            mutations: 1024,
        },
        request,
    );
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(selected),
        spec().instance,
        (),
        &mut work,
    )?;
    assert_eq!(native_pin_count(&catalog)?, 1);
    cancel.cancel();
    let guard = query_guard(need)?;
    assert!(matches!(
        prepared.execute(&guard),
        Err(IndexError::Cancelled)
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

#[test]
fn registered_query_rejects_wrong_view_and_releases_selected_pin() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let selected = configured(&engine, &objects)?;
    let baseline = native_pin_count(&catalog)?;
    assert_eq!(baseline, 0);

    let need = quanta_index_core::index::effort::ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 1024,
        provider_units: 0,
    };
    let mut queries = quanta_index_kernel::QueryAdapterRegistry::new();
    queries.register(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        need,
        FaultQueryPort {
            need,
            fault: QueryFault::WrongView,
        },
    )?;
    let mut query_budget = budget();
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(selected),
        spec().instance,
        (),
        &mut query_budget,
    )?;
    assert!(matches!(
        prepared.execute(&query_guard(need)?),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(native_pin_count(&catalog)?, baseline);

    assert!(matches!(
        engine.prepare_query(
            &queries,
            ViewSelection::Exact(selected),
            IndexInstanceId([0xfa; 16]),
            (),
            &mut budget(),
        ),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(native_pin_count(&catalog)?, baseline);
    let absent = quanta_index_kernel::QueryAdapterRegistry::<(), ()>::new();
    assert!(matches!(
        engine.prepare_query(
            &absent,
            ViewSelection::Exact(selected),
            spec().instance,
            (),
            &mut budget(),
        ),
        Err(IndexError::SnapshotUnavailable)
    ));
    assert_eq!(native_pin_count(&catalog)?, baseline);
    Ok(())
}

#[test]
fn registered_query_cancellation_releases_selected_pin() -> TestResult {
    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = kernel(root.path())?;
    let selected = configured(&engine, &objects)?;
    let baseline = native_pin_count(&catalog)?;
    assert_eq!(baseline, 0);

    let need = quanta_index_core::index::effort::ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 1024,
        provider_units: 0,
    };
    let request = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
    let cancel = request.cancel_handle();
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 0,
            bytes: 512 * 1024 * 1024,
            // Leave budget for validating the selected two-instance profile.
            mutations: 1024,
        },
        request,
    );
    let mut queries = quanta_index_kernel::QueryAdapterRegistry::new();
    queries.register(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        need,
        FaultQueryPort {
            need,
            fault: QueryFault::Cancel(cancel),
        },
    )?;
    let prepared = engine.prepare_query(
        &queries,
        ViewSelection::Exact(selected),
        spec().instance,
        (),
        &mut work,
    )?;
    let outcome = prepared.execute(&query_guard(need)?);
    assert!(
        work.request().is_cancelled(),
        "query adapter did not cancel the request"
    );
    assert!(matches!(outcome, Err(IndexError::Cancelled)), "{outcome:?}");
    assert_eq!(work.usage().writes, 0);
    assert_eq!(native_pin_count(&catalog)?, baseline);
    Ok(())
}

#[derive(Clone, Copy)]
enum AsyncQueryFault {
    None,
    Desired,
    Covered,
    Status,
    Instance,
}

struct AsyncCoverageQueryPort {
    need: quanta_index_core::index::effort::ResourceNeed,
    fault: AsyncQueryFault,
}

impl quanta_index_core::index::IndexQueryPort<(), ()> for AsyncCoverageQueryPort {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        if *instance != async_spec() {
            return Err(IndexError::InvalidInput(
                "unexpected async query instance".into(),
            ));
        }
        Ok(())
    }

    fn reservation(
        &self,
        _request: &(),
    ) -> Result<quanta_index_core::index::effort::ResourceNeed, IndexError> {
        Ok(self.need)
    }

    fn query(
        &self,
        _request: &(),
        context: &mut quanta_index_core::index::QueryReadContext<'_>,
    ) -> Result<quanta_index_core::index::QueryOutcome<()>, IndexError> {
        use quanta_index_core::index::{CoverageBinding, CoverageStatus, MapRoot};

        let mut key = b"member\0".to_vec();
        key.extend_from_slice(&context.instance.instance.0);
        let reference = context
            .maps
            .lookup_object(context.ticket.view.coverage, &key, context.budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let stored = context.objects.get(reference, context.budget)?;
        let mut binding: CoverageBinding = serde_json::from_slice(&stored.payload)
            .map_err(|error| IndexError::CorruptObject(error.to_string()))?;
        match self.fault {
            AsyncQueryFault::None => {}
            AsyncQueryFault::Desired => binding.desired.context = Digest([0xfd; 32]),
            AsyncQueryFault::Covered => {
                binding.covered = if binding.covered.is_some() {
                    None
                } else {
                    Some(binding.desired)
                };
            }
            AsyncQueryFault::Status => {
                binding.status = match binding.status {
                    CoverageStatus::Pending { .. } => CoverageStatus::Partial {
                        missing: MapRoot(None),
                    },
                    CoverageStatus::Partial { .. } | CoverageStatus::Complete => {
                        CoverageStatus::Pending {
                            previous_missing: MapRoot(None),
                        }
                    }
                };
            }
            AsyncQueryFault::Instance => binding.instance = IndexInstanceId([0xfa; 16]),
        }
        quanta_index_core::index::QueryOutcome::async_ranked(
            context.ticket,
            context.instance,
            &binding,
            false,
            (),
        )
    }
}

fn async_query_registry(
    fault: AsyncQueryFault,
    need: quanta_index_core::index::effort::ResourceNeed,
) -> TestResult<quanta_index_kernel::QueryAdapterRegistry<(), ()>> {
    let mut queries = quanta_index_kernel::QueryAdapterRegistry::new();
    queries.register(
        AdapterDescriptor {
            kind: async_spec().kind,
            schema: async_spec().schema,
            format: async_spec().format,
        },
        need,
        AsyncCoverageQueryPort { need, fault },
    )?;
    Ok(queries)
}

#[test]
fn registered_async_query_binds_stored_coverage_for_old_and_current_views() -> TestResult {
    use quanta_index_core::index::QueryCoverage;

    let root = tempfile::tempdir()?;
    let (engine, objects, catalog) = owner_kernel(
        root.path(),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    )?;
    let configured = configured_owner(&engine, &objects)?;
    let source_record = stage(&objects, "owner.rs", b"content")?;
    let pending = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(source_record),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let old_ticket = engine.pin_view(ViewSelection::Exact(pending), &budget())?;
    let completed = complete_owner(&engine, pending, 3, owner(1))?;
    old_ticket.validate()?;
    let old_binding = coverage(&engine, &objects, pending)?;
    let current_binding = coverage(&engine, &objects, completed)?;
    assert_ne!(old_binding, current_binding);
    let need = quanta_index_core::index::effort::ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        memory_bytes: 1024,
        provider_units: 0,
    };
    let queries = async_query_registry(AsyncQueryFault::None, need)?;
    let baseline = native_pin_count(&catalog)?;
    assert_eq!(baseline, 1);
    for (view, expected) in [(pending, old_binding), (completed, current_binding)] {
        let mut query_budget = budget();
        let prepared = engine.prepare_query(
            &queries,
            ViewSelection::Exact(view),
            async_spec().instance,
            (),
            &mut query_budget,
        )?;
        let outcome = prepared.execute(&query_guard(need)?)?;
        assert_eq!(outcome.view(), view);
        assert_eq!(
            outcome.coverage(),
            &QueryCoverage::Async(Box::new(expected))
        );
        assert!(!outcome.exhaustive());
        assert_eq!(native_pin_count(&catalog)?, baseline);
    }
    for fault in [
        AsyncQueryFault::Desired,
        AsyncQueryFault::Covered,
        AsyncQueryFault::Status,
        AsyncQueryFault::Instance,
    ] {
        let queries = async_query_registry(fault, need)?;
        let mut query_budget = budget();
        let prepared = engine.prepare_query(
            &queries,
            ViewSelection::Exact(pending),
            async_spec().instance,
            (),
            &mut query_budget,
        )?;
        assert!(matches!(
            prepared.execute(&query_guard(need)?),
            Err(IndexError::BackendViolation(_))
        ));
        assert_eq!(native_pin_count(&catalog)?, baseline);
    }
    old_ticket.release()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

// This spy observes only the measured submit. Fixture construction and its
// independent read oracles continue to use the concrete store directly.
struct UpdateCostSpy {
    inner: Arc<FileObjectStore>,
    retain_reads: bool,
    observe_reads: bool,
    active: std::sync::atomic::AtomicBool,
    refuse_contribution_batch_once: std::sync::atomic::AtomicBool,
    refuse_contribution_scalar_once: std::sync::atomic::AtomicBool,
    confirmed_contributions_before_tail: std::sync::atomic::AtomicU64,
    refuse_inventory_after_confirm_once: std::sync::atomic::AtomicBool,
    confirmed_inventory_roots: std::sync::atomic::AtomicU64,
    state: std::sync::Mutex<UpdateCostSpyState>,
    io_observer: Option<Arc<quanta_index_storage::ObjectIoObservation>>,
}

#[derive(Clone, Default)]
struct MapStreamCost {
    calls: u64,
    roots: u64,
    edits: u64,
    reserve_calls: u64,
    barrier_calls: u64,
}

impl MapStreamCost {
    fn shape(&self) -> [u64; 3] {
        [self.calls, self.roots, self.edits]
    }

    fn record(
        &mut self,
        roots: usize,
        edits: u64,
        io: Option<&quanta_index_storage::ObjectIoSnapshot>,
    ) -> Result<(), IndexError> {
        self.calls = self
            .calls
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.roots = self
            .roots
            .checked_add(u64::try_from(roots).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        self.edits = self
            .edits
            .checked_add(edits)
            .ok_or(IndexError::ResourceLimited)?;
        if let Some(io) = io {
            self.reserve_calls = self
                .reserve_calls
                .checked_add(io.reserve_calls)
                .ok_or(IndexError::ResourceLimited)?;
            self.barrier_calls = self
                .barrier_calls
                .checked_add(io.barrier_calls)
                .ok_or(IndexError::ResourceLimited)?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct UpdateCostSpyState {
    physical_reads: std::collections::BTreeMap<ObjectRef, u64>,
    retained_reads: std::collections::BTreeMap<ObjectRef, u64>,
    prior_source_owners: Option<quanta_index_core::index::MapRoot>,
    unrelated_records: std::collections::BTreeSet<ObjectRef>,
    prior_source_scans: u64,
    unrelated_record_reads: u64,
    scanned_entries: u64,
    occurrence_inputs: u64,
    occurrence_install_calls: u64,
    materialized_metadata_batches: u64,
    empty_base_edits: u64,
    single_map_stream: MapStreamCost,
    multi_map_stream: MapStreamCost,
    io_baseline: Option<quanta_index_storage::ObjectIoSnapshot>,
}

#[derive(Clone)]
struct UpdateCostObservation {
    cold_sql: Option<native_catalog_profile::Snapshot>,
    delta_sql: Option<native_catalog_profile::Snapshot>,
    delete_sql: Option<native_catalog_profile::Snapshot>,
    prior_source_scans: u64,
    unrelated_record_reads: u64,
    scanned_entries: u64,
    occurrence_inputs: u64,
    occurrence_install_calls: u64,
    materialized_metadata_batches: u64,
    empty_base_edits: u64,
    single_map_stream: MapStreamCost,
    multi_map_stream: MapStreamCost,
    object_io: Option<quanta_index_storage::ObjectIoSnapshot>,
    cold_object_io: Option<quanta_index_storage::ObjectIoSnapshot>,
    cold_elapsed: Option<Duration>,
    cold_occurrence_inputs: Option<u64>,
    cold_occurrence_install_calls: Option<u64>,
    cold_single_map_stream: Option<MapStreamCost>,
    cold_multi_map_stream: Option<MapStreamCost>,
    inventory_batch_limit: Option<usize>,
    delta_elapsed: Option<Duration>,
    cold_artifact: Option<ObjectRef>,
    delta_artifact: Option<ObjectRef>,
    source_stage_io: Option<quanta_index_storage::ObjectIoSnapshot>,
    source_stage_elapsed: Option<Duration>,
    capture_io: Option<quanta_index_storage::ObjectIoSnapshot>,
    capture_elapsed: Option<Duration>,
    initialization_elapsed: Option<Duration>,
    delta_capture_io: Option<quanta_index_storage::ObjectIoSnapshot>,
    delta_capture_elapsed: Option<Duration>,
}

impl UpdateCostSpy {
    fn record_map_stream(
        &self,
        roots: usize,
        edits: u64,
        before: Option<&quanta_index_storage::ObjectIoSnapshot>,
    ) -> Result<(), IndexError> {
        let io = match (&self.io_observer, before) {
            (Some(observer), Some(before)) => Some(observer.snapshot()?.checked_since(before)?),
            (None, None) => None,
            _ => {
                return Err(IndexError::BackendViolation(
                    "map stream IO baseline missing".into(),
                ));
            }
        };
        let mut state = self
            .state
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let cost = if roots == 1 {
            &mut state.single_map_stream
        } else {
            &mut state.multi_map_stream
        };
        cost.record(roots, edits, io.as_ref())
    }

    fn record_occurrence_inputs<'a>(
        &self,
        payloads: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<(), IndexError> {
        if !self.active.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        // Fixed persisted occurrence codec gold: {"positions":[...]}. Count
        // submitted inputs across every immutable-write route, not files after
        // content-addressed reuse.
        let count = payloads
            .into_iter()
            .filter(|payload| payload.starts_with(br#"{"positions":"#))
            .count();
        if count == 0 {
            return Ok(());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        state.occurrence_inputs = state
            .occurrence_inputs
            .checked_add(u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        state.occurrence_install_calls = state
            .occurrence_install_calls
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        drop(state);
        Ok(())
    }

    fn record_empty_base_edits(&self, count: usize) -> Result<(), IndexError> {
        if count == 0 || !self.active.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        state.empty_base_edits = state
            .empty_base_edits
            .checked_add(u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        drop(state);
        Ok(())
    }

    fn new(inner: Arc<FileObjectStore>) -> Self {
        Self {
            inner,
            retain_reads: false,
            observe_reads: false,
            active: std::sync::atomic::AtomicBool::new(false),
            refuse_contribution_batch_once: std::sync::atomic::AtomicBool::new(false),
            refuse_contribution_scalar_once: std::sync::atomic::AtomicBool::new(false),
            confirmed_contributions_before_tail: std::sync::atomic::AtomicU64::new(0),
            refuse_inventory_after_confirm_once: std::sync::atomic::AtomicBool::new(false),
            confirmed_inventory_roots: std::sync::atomic::AtomicU64::new(0),
            state: std::sync::Mutex::new(UpdateCostSpyState::default()),
            io_observer: None,
        }
    }

    fn new_observed(
        inner: Arc<FileObjectStore>,
        observer: Arc<quanta_index_storage::ObjectIoObservation>,
    ) -> Self {
        Self {
            io_observer: Some(observer),
            ..Self::new(inner)
        }
    }

    fn new_observing_reads(inner: Arc<FileObjectStore>, retain_reads: bool) -> Self {
        Self {
            retain_reads,
            observe_reads: true,
            ..Self::new(inner)
        }
    }

    fn arm(
        &self,
        prior_source_owners: quanta_index_core::index::MapRoot,
        unrelated_records: std::collections::BTreeSet<ObjectRef>,
    ) -> TestResult {
        let io_baseline = self
            .io_observer
            .as_ref()
            .map(|observer| observer.snapshot())
            .transpose()?;
        {
            let mut state = self
                .state
                .lock()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            *state = UpdateCostSpyState {
                prior_source_owners: Some(prior_source_owners),
                unrelated_records,
                io_baseline,
                ..UpdateCostSpyState::default()
            };
        }
        self.active
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    fn disarm(&self) -> TestResult<UpdateCostObservation> {
        self.active
            .store(false, std::sync::atomic::Ordering::Release);
        let state = self
            .state
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(UpdateCostObservation {
            cold_sql: None,
            delta_sql: None,
            delete_sql: None,
            prior_source_scans: state.prior_source_scans,
            unrelated_record_reads: state.unrelated_record_reads,
            scanned_entries: state.scanned_entries,
            occurrence_inputs: state.occurrence_inputs,
            occurrence_install_calls: state.occurrence_install_calls,
            materialized_metadata_batches: state.materialized_metadata_batches,
            empty_base_edits: state.empty_base_edits,
            single_map_stream: state.single_map_stream.clone(),
            multi_map_stream: state.multi_map_stream.clone(),
            object_io: match (&self.io_observer, &state.io_baseline) {
                (Some(observer), Some(prior)) => Some(observer.snapshot()?.checked_since(prior)?),
                (None, None) => None,
                _ => {
                    return Err(
                        std::io::Error::other("update cost observer lost its baseline").into(),
                    );
                }
            },
            cold_object_io: None,
            cold_elapsed: None,
            cold_occurrence_inputs: None,
            cold_occurrence_install_calls: None,
            cold_single_map_stream: None,
            cold_multi_map_stream: None,
            inventory_batch_limit: None,
            delta_elapsed: None,
            cold_artifact: None,
            delta_artifact: None,
            source_stage_io: None,
            source_stage_elapsed: None,
            capture_io: None,
            capture_elapsed: None,
            initialization_elapsed: None,
            delta_capture_io: None,
            delta_capture_elapsed: None,
        })
    }
}

impl ImmutableObjectPort for UpdateCostSpy {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
        self.record_occurrence_inputs([payload])?;
        self.inner.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
        self.record_occurrence_inputs([payload])?;
        if payload.starts_with(br#"{"record":"#)
            && self
                .refuse_contribution_scalar_once
                .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(IndexError::ResourceLimited);
        }
        self.inner.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[quanta_index_core::index::ObjectBatchInput],
        sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        self.record_occurrence_inputs(inputs.iter().map(|input| input.payload.as_slice()))?;
        // Fixed persisted contribution codec gold; fail only the used multi-owner
        // batch, after all earlier source/inventory installations have completed.
        if inputs.len() > 1
            && inputs
                .iter()
                .all(|input| input.payload.starts_with(br#"{"record":"#))
            && self
                .refuse_contribution_batch_once
                .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(IndexError::ResourceLimited);
        }
        let confirmed = self.inner.install_batch(inputs, sink, budget)?;
        if self.active.load(std::sync::atomic::Ordering::Acquire)
            && let [binding_input, coverage_input] = inputs
        {
            let binding = serde_json::from_slice::<quanta_index_core::index::OwnerMemberBinding>(
                &binding_input.payload,
            );
            let coverage = serde_json::from_slice::<quanta_index_core::index::OwnerCoverage>(
                &coverage_input.payload,
            );
            if let (Ok(binding), Ok(coverage), Some(confirmed_binding)) =
                (binding, coverage, confirmed.first())
                && binding.owner == coverage.owner
                && coverage.covered == Some(binding.input)
                && confirmed.len() == 2
                && coverage.searchable == Some(confirmed_binding.reference)
                && coverage_input
                    .children
                    .contains(&confirmed_binding.reference)
            {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|error| IndexError::Storage(error.to_string()))?;
                state.materialized_metadata_batches = state
                    .materialized_metadata_batches
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        if self
            .refuse_contribution_scalar_once
            .load(std::sync::atomic::Ordering::Acquire)
            && inputs
                .iter()
                .all(|input| input.payload.starts_with(br#"{"record":"#))
        {
            let count =
                u64::try_from(confirmed.len()).map_err(|_error| IndexError::ResourceLimited)?;
            let _prior = self
                .confirmed_contributions_before_tail
                .fetch_update(
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                    |prior| prior.checked_add(count),
                )
                .map_err(|_error| {
                    IndexError::BackendViolation("tail refusal observation overflowed".into())
                })?;
        }
        Ok(confirmed)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StoredObject, IndexError> {
        if self.active.load(std::sync::atomic::Ordering::Acquire) {
            let mut state = self
                .state
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            if self.observe_reads {
                let reads = state.physical_reads.entry(reference).or_default();
                *reads = reads.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            if state.unrelated_records.contains(&reference) {
                state.unrelated_record_reads = state
                    .unrelated_record_reads
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        self.inner.get(reference, budget)
    }

    fn read_retained(
        &self,
        object: &quanta_index_core::index::StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StoredObject, IndexError> {
        if !self.retain_reads {
            return self.get(object.registration.reference, budget);
        }
        if self.observe_reads && self.active.load(std::sync::atomic::Ordering::Acquire) {
            let mut state = self
                .state
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            let reads = state
                .retained_reads
                .entry(object.registration.reference)
                .or_default();
            *reads = reads.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            drop(state);
        }
        self.inner.read_retained(object, budget)
    }
}

impl PersistentMapPort for UpdateCostSpy {
    fn visit_diff(
        &self,
        before: quanta_index_core::index::MapRoot,
        after: quanta_index_core::index::MapRoot,
        range: quanta_index_core::index::ports::MapDiffRange<'_>,
        visitor: &mut dyn quanta_index_core::index::ports::MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        self.inner.visit_diff(before, after, range, visitor, budget)
    }

    fn lookup(
        &self,
        root: quanta_index_core::index::MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
        self.inner.lookup(root, key, budget)
    }

    fn scan(
        &self,
        root: quanta_index_core::index::MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::MapPage, IndexError> {
        let page = self.inner.scan(root, after, limit, budget)?;
        if self.active.load(std::sync::atomic::Ordering::Acquire) {
            let mut state = self
                .state
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            if state.prior_source_owners == Some(root) {
                state.prior_source_scans = state
                    .prior_source_scans
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            state.scanned_entries = state
                .scanned_entries
                .checked_add(
                    u64::try_from(page.entries.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
        }
        Ok(page)
    }

    fn apply(
        &self,
        base: quanta_index_core::index::MapRoot,
        edits: &[quanta_index_core::index::MapEdit],
        sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::MapWrite, IndexError> {
        if base == quanta_index_core::index::MapRoot(None) {
            self.record_empty_base_edits(edits.len())?;
        }
        self.inner.apply(base, edits, sink, budget)
    }

    fn apply_batch(
        &self,
        requests: &[quanta_index_core::index::MapBatchRequest],
        sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::MapBatchWrite, IndexError> {
        if self.active.load(std::sync::atomic::Ordering::Acquire) {
            let empty_edits = requests
                .iter()
                .filter(|request| request.base == quanta_index_core::index::MapRoot(None))
                .try_fold(0_usize, |sum, request| {
                    sum.checked_add(request.edits.len())
                        .ok_or(IndexError::ResourceLimited)
                })?;
            self.record_empty_base_edits(empty_edits)?;
        }
        self.inner.apply_batch(requests, sink, budget)
    }

    fn apply_stream(
        &self,
        bases: &[quanta_index_core::index::MapRoot],
        edits: &mut dyn quanta_index_core::index::ports::MapEditCursor,
        sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
        struct CountingCursor<'a> {
            inner: &'a mut dyn quanta_index_core::index::ports::MapEditCursor,
            bases: &'a [quanta_index_core::index::MapRoot],
            empty_edits: usize,
            edits: u64,
        }
        impl quanta_index_core::index::ports::MapEditCursor for CountingCursor<'_> {
            fn next_edit(
                &mut self,
                budget: &mut WorkBudget,
            ) -> Result<Option<quanta_index_core::index::OrdinalMapEdit>, IndexError> {
                let edit = self.inner.next_edit(budget)?;
                if edit.is_some() {
                    self.edits = self
                        .edits
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                }
                if edit.as_ref().is_some_and(|item| {
                    matches!(
                        usize::try_from(item.root),
                        Ok(index)
                            if self.bases.get(index)
                                == Some(&quanta_index_core::index::MapRoot(None))
                    )
                }) {
                    self.empty_edits = self
                        .empty_edits
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                }
                Ok(edit)
            }
        }
        // Failure at the real inventory-completion boundary, after every
        // node in this first 64-root group is durably registered by the store.
        if bases.len() == 64
            && self
                .refuse_inventory_after_confirm_once
                .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            let confirmed = self.inner.apply_stream(bases, edits, sink, budget)?;
            self.confirmed_inventory_roots.store(
                u64::try_from(confirmed.len()).map_err(|_error| IndexError::ResourceLimited)?,
                std::sync::atomic::Ordering::Release,
            );
            return Err(IndexError::ResourceLimited);
        }
        if !self.active.load(std::sync::atomic::Ordering::Acquire) {
            return self.inner.apply_stream(bases, edits, sink, budget);
        }
        let mut counted = CountingCursor {
            inner: edits,
            bases,
            empty_edits: 0,
            edits: 0,
        };
        let before = self
            .io_observer
            .as_ref()
            .map(|observer| observer.snapshot())
            .transpose()?;
        let roots = self.inner.apply_stream(bases, &mut counted, sink, budget)?;
        self.record_map_stream(bases.len(), counted.edits, before.as_ref())?;
        self.record_empty_base_edits(counted.empty_edits)?;
        Ok(roots)
    }
}

type CostKernelFixture = (
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
    Arc<UpdateCostSpy>,
);

fn cost_kernel(root: &std::path::Path) -> TestResult<CostKernelFixture> {
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.join("native-objects"),
        scratch.clone(),
    )?);
    let spy = Arc::new(UpdateCostSpy::new(Arc::clone(&objects)));
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        NativeLiteralIndex,
        |delta| Ok(delta.clone()),
    )?;
    registry.register(
        AdapterDescriptor {
            kind: file_spec().kind,
            schema: file_spec().schema,
            format: file_spec().format,
        },
        NativeFileMap,
        |delta| Ok(delta.clone()),
    )?;
    let services = KernelServices::new(
        spy.clone(),
        spy.clone(),
        catalog.clone(),
        scratch,
        Duration::from_secs(30),
    )?;
    Ok((Kernel::new(services, registry), objects, catalog, spy))
}

#[test]
fn materialize_batches_independent_coverage_roots_and_preserves_old_view() -> TestResult {
    use quanta_index_core::index::CoverageStatus;

    let directory = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(
        directory.path().join("native-objects"),
    )?);
    let spy = Arc::new(UpdateCostSpy::new(Arc::clone(&objects)));
    let catalog = Arc::new(SqliteCatalog::open(
        directory.path(),
        Duration::from_millis(100),
    )?);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        CountingOwnerAdapter(Arc::clone(&calls)),
    )?;
    let services = KernelServices::new(
        spy.clone(),
        spy.clone(),
        catalog.clone(),
        Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            directory.path().join("scratch"),
            16 * 1024 * 1024 * 1024,
        )?),
        Duration::from_secs(30),
    )?;
    let engine = Kernel::new(services, registry);
    let configured = configured_owner(&engine, &objects)?;
    let record = stage(&objects, "selected.rs", b"selected owner")?;
    let source_request = request(
        &engine,
        &catalog,
        &objects,
        configured,
        source(&engine, configured)?,
        2,
        1,
        vec![OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(record),
        }],
    )?;
    spy.arm(
        source(&engine, configured)?.owners,
        std::collections::BTreeSet::new(),
    )?;
    let pending = published(block_on(engine.submit(source_request, &mut budget()))?)?;
    let cold = spy.disarm()?;
    // Fresh uncovered owners have the same independent owners/missing gold;
    // their shared root must require only one produced coverage edit stream.
    assert_eq!(cold.single_map_stream.shape(), [1, 1, 1]);
    let before = coverage(&engine, &objects, pending)?;
    let CoverageStatus::Partial {
        missing: prior_missing,
    } = before.status
    else {
        return Err("owner must be pending before materialization".into());
    };
    let prior_pin = engine.pin_view(ViewSelection::Exact(pending), &budget())?;
    let prior_source_owners = prior_pin.view().source.owners;
    spy.arm(prior_source_owners, std::collections::BTreeSet::new())?;
    let completed = complete_owner(&engine, pending, 3, owner(1))?;
    let measured = spy.disarm()?;
    assert_eq!(measured.multi_map_stream.shape(), [1, 2, 2]);
    assert_eq!(measured.materialized_metadata_batches, 1);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let after = coverage(&engine, &objects, completed)?;
    assert_eq!(after.status, CoverageStatus::Complete);
    assert_eq!(map_count(&objects, after.owners)?, 1);
    let selected = owner_coverage(&objects, &after, owner(1))?;
    assert_eq!(selected.covered, Some(selected.desired));
    assert!(selected.searchable.is_some());
    assert_eq!(coverage(&engine, &objects, pending)?.status, before.status);
    assert_eq!(map_count(&objects, prior_missing)?, 1);
    drop(prior_pin);
    Ok(())
}

#[test]
fn pinned_materialize_reuses_artifact_data_without_changing_logical_work() -> TestResult {
    let mut baseline = None;
    for retain_reads in [false, true] {
        let directory = tempfile::tempdir()?;
        let objects = Arc::new(FileObjectStore::open(
            directory.path().join("native-objects"),
        )?);
        let spy = Arc::new(UpdateCostSpy::new_observing_reads(
            Arc::clone(&objects),
            retain_reads,
        ));
        let catalog = Arc::new(SqliteCatalog::open(
            directory.path(),
            Duration::from_millis(100),
        )?);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut registry = AdapterRegistry::new();
        registry.register_owner(
            AdapterDescriptor {
                kind: spec().kind,
                schema: spec().schema,
                format: spec().format,
            },
            RepeatedArtifactOwner(CountingOwnerAdapter(Arc::clone(&calls))),
        )?;
        let services = KernelServices::new(
            spy.clone(),
            spy.clone(),
            catalog.clone(),
            Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
                directory.path().join("scratch"),
                16 * 1024 * 1024 * 1024,
            )?),
            Duration::from_secs(30),
        )?;
        let engine = Kernel::new(services, registry);
        let configured = configured_owner(&engine, &objects)?;
        let record = stage(&objects, "selected.rs", b"selected owner")?;
        let pending = published(block_on(engine.submit(
            request(
                &engine,
                &catalog,
                &objects,
                configured,
                source(&engine, configured)?,
                2,
                1,
                vec![OwnerMutation {
                    owner: owner(1),
                    expected: None,
                    next: Some(record),
                }],
            )?,
            &mut budget(),
        ))?)?;
        spy.arm(
            source(&engine, configured)?.owners,
            std::collections::BTreeSet::new(),
        )?;
        let mut work = budget();
        let claim = engine.claim_work(
            &intent_for(&engine, owner(1))?,
            Duration::from_secs(120),
            &budget(),
        )?;
        let completed = published(block_on(engine.submit(
            UpdateRequest {
                operation: operation(3),
                expected_view: pending,
                kind: UpdateKind::Materialize { claim },
            },
            &mut work,
        ))?)?;
        let _observed = spy.disarm()?;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let selected =
            owner_coverage(&objects, &coverage(&engine, &objects, completed)?, owner(1))?;
        let binding_ref = selected.searchable.ok_or("owner binding missing")?;
        let binding: quanta_index_core::index::OwnerMemberBinding =
            serde_json::from_slice(&objects.get(binding_ref, &mut budget())?.payload)?;
        let state = spy
            .state
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let physical = state
            .physical_reads
            .get(&binding.artifact)
            .copied()
            .unwrap_or(0);
        let retained = state
            .retained_reads
            .get(&binding.artifact)
            .copied()
            .unwrap_or(0);
        drop(state);
        assert!(
            physical >= 1,
            "original store did not authenticate the artifact"
        );
        if retain_reads {
            let (old_work, old_physical) = baseline.ok_or("missing independent baseline")?;
            assert_eq!(work.usage(), old_work, "DATA reuse reduced logical fees");
            assert!(
                physical < old_physical,
                "pinned caller did not avoid physical reads"
            );
            assert!(retained >= 1, "pinned caller did not use read_retained");
        } else {
            assert!(
                physical >= 2,
                "independent baseline did not repeat the read"
            );
            assert_eq!(retained, 0);
            baseline = Some((work.usage(), physical));
        }
    }
    Ok(())
}

fn one_owner_update_cost_case(
    owners: u32,
    history: u64,
) -> TestResult<quanta_index_core::index::WorkUsage> {
    let directory = tempfile::tempdir()?;
    let (engine, objects, catalog, spy) = cost_kernel(directory.path())?;
    let configured = configured(&engine, &objects)?;
    let hot_owner = large_owner(owners);
    let hot_path = format!("f{owners}.rs");
    let mut unrelated = std::collections::BTreeSet::new();
    let mut mutations = Vec::new();
    let mut hot_source = None;
    for id in 1..=owners {
        let path = format!("f{id}.rs");
        let bytes = if id == owners {
            b"needle old".as_slice()
        } else {
            b"plain".as_slice()
        };
        let reference = stage(&objects, &path, bytes)?;
        if id == owners {
            hot_source = Some(reference);
        } else {
            let _new_record = unrelated.insert(reference);
        }
        mutations.push(OwnerMutation {
            owner: large_owner(id),
            expected: None,
            next: Some(reference),
        });
    }
    let mut hot_source = hot_source.ok_or("hot source missing")?;
    let mut current = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            mutations,
        )?,
        &mut large_fixture_budget(),
    ))?)?;
    for revision in 0..history {
        let next_body = if revision.is_multiple_of(2) {
            b"needle new".as_slice()
        } else {
            b"needle old".as_slice()
        };
        let next_source = stage(&objects, &hot_path, next_body)?;
        let sequence = revision.checked_add(3).ok_or("history sequence overflow")?;
        let event_sequence = revision.checked_add(2).ok_or("history event overflow")?;
        current = published(block_on(engine.submit(
            request(
                &engine,
                &catalog,
                &objects,
                current,
                source(&engine, current)?,
                sequence,
                event_sequence,
                vec![OwnerMutation {
                    owner: hot_owner,
                    expected: Some(hot_source),
                    next: Some(next_source),
                }],
            )?,
            &mut budget(),
        ))?)?;
        hot_source = next_source;
    }
    if history.is_multiple_of(2) {
        assert_eq!(
            hits(
                &engine,
                current,
                &objects,
                &member(&engine, &objects, current)?,
                b"old"
            )?,
            vec![hot_owner]
        );
    }
    let old_ticket = engine.pin_view(ViewSelection::Exact(current), &budget())?;
    let old_cut = old_ticket.view().source;
    let old_literal = member(&engine, &objects, current)?;
    let old_file = file_member(&engine, &objects, current)?;
    let next_source = stage(&objects, &hot_path, b"needle new")?;
    let request = request(
        &engine,
        &catalog,
        &objects,
        current,
        old_cut,
        history.checked_add(3).ok_or("measured sequence overflow")?,
        history.checked_add(2).ok_or("measured event overflow")?,
        vec![OwnerMutation {
            owner: hot_owner,
            expected: Some(hot_source),
            next: Some(next_source),
        }],
    )?;
    let UpdateKind::SourceAdvance(transition) = &request.kind else {
        return Err("measured request is not a source transition".into());
    };
    assert_eq!(transition.owners.count, 1);
    let changed_owner_count = transition.owners.count;
    spy.arm(old_cut.owners, unrelated)?;
    let mut work = budget();
    let updated = published(block_on(engine.submit(request, &mut work))?)?;
    let observed = spy.disarm()?;
    assert_eq!(
        observed.prior_source_scans, 0,
        "patch scanned the entire prior source map"
    );
    assert_eq!(
        observed.unrelated_record_reads, 0,
        "patch read an unrelated source record"
    );
    // This profile has exactly one changed owner and two required adapters.
    // Five cursors can scan the sealed one-entry mutation map: admission seal
    // (update.rs), source validation and source COW edits (source.rs), then
    // SourceInputCursor in NativeLiteralIndex and NativeFileMap. Work actions
    // are empty. Each cursor can yield that entry once; none needs a source
    // snapshot page. This bound is independent of unrelated owners and history.
    let scan_ceiling = changed_owner_count
        .checked_mul(5)
        .ok_or("scan ceiling overflow")?;
    assert!(
        observed.scanned_entries <= scan_ceiling,
        "patch scanned more map entries than the five changed-owner cursor passes"
    );

    let new_cut = source(&engine, updated)?;
    assert_eq!(old_ticket.view().source, old_cut);
    assert_eq!(
        objects.lookup_object(old_cut.owners, &owner_map_key(hot_owner), &mut budget())?,
        Some(hot_source)
    );
    assert_eq!(
        objects.lookup_object(new_cut.owners, &owner_map_key(hot_owner), &mut budget())?,
        Some(next_source)
    );
    assert_eq!(
        hits(&engine, current, &objects, &old_literal, b"old")?,
        vec![hot_owner]
    );
    let new_literal = member(&engine, &objects, updated)?;
    assert!(hits(&engine, updated, &objects, &new_literal, b"old")?.is_empty());
    assert_eq!(
        hits(&engine, updated, &objects, &new_literal, b"new")?,
        vec![hot_owner]
    );
    let new_file = file_member(&engine, &objects, updated)?;
    let new_ticket = engine.pin_view(ViewSelection::Exact(updated), &budget())?;
    assert_eq!(old_literal.input.root, old_cut.owners);
    assert_eq!(new_literal.input.root, new_cut.owners);
    assert_eq!(old_file.input.root, old_cut.owners);
    assert_eq!(new_file.input.root, new_cut.owners);
    for (ticket, file, expected_source) in [
        (old_ticket.record(), &old_file, hot_source),
        (new_ticket.record(), &new_file, next_source),
    ] {
        let paths = query_paths(
            &FilePathQuery {
                member: file,
                prefix: &hot_path,
                after: None,
                limit: 2,
            },
            &mut FileMapReadContext {
                ticket,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        let [entry] = paths.entries.as_slice() else {
            return Err("hot path query must return one owner".into());
        };
        assert_eq!(
            (entry.path.as_str(), entry.owner, entry.source),
            (hot_path.as_str(), hot_owner, expected_source)
        );
        assert!(!paths.has_more);
    }
    old_ticket.release()?;
    new_ticket.release()?;
    Ok(work.usage())
}

#[test]
fn one_owner_patch_avoids_prior_corpus_walk_and_preserves_old_view() -> TestResult {
    let mut usage = Vec::new();
    for (owners, history) in [(64, 0), (512, 0), (512, 8), (512, 32)] {
        usage.push((
            (owners, history),
            one_owner_update_cost_case(owners, history)?,
        ));
    }
    assert_eq!(usage.len(), 4);
    assert!(
        usage
            .iter()
            .all(|(_, work)| work.reads > 0 && work.writes > 0)
    );
    Ok(())
}

#[derive(Clone, Copy)]
enum TextCostCorpus {
    Shared,
    Distinct,
}

#[derive(Clone, Copy)]
enum TextSourceStage {
    Scalar,
    Batch32,
}

fn native_text_cost_request(
    kernel: &Kernel<SqliteCatalog>,
    expected_view: ViewRef,
    expected_source: SourceCut,
    key: OperationKey,
    event_sequence: u64,
    mutations: Vec<OwnerMutation>,
) -> TestResult<UpdateRequest> {
    let ingress = kernel.begin_input(key)?;
    let mut transition = SourceTransition {
        expected_source,
        owners: kernel.stage_mutations(ingress, mutations.into_iter().map(Ok), &mut budget())?,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("event sequence must start at one")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    Ok(UpdateRequest {
        operation: key,
        expected_view,
        kind: UpdateKind::SourceAdvance(transition),
    })
}

// Fixed two-token corpus; ASCII base26 stays unique across the full u32 domain.
// Generation belongs to fixture setup, outside measured ingress/publication.
fn text_cost_distinct_body(id: u32) -> TestResult<String> {
    let mut letters = [b'a'; 7];
    let mut value = id;
    for letter in letters.iter_mut().rev() {
        *letter = b'a'
            .checked_add(u8::try_from(value % 26)?)
            .ok_or("distinct corpus letter overflow")?;
        value = value
            .checked_div(26)
            .ok_or("distinct corpus divisor is zero")?;
    }
    let mut width = 2_usize;
    let mut remaining = id
        .checked_div(676)
        .ok_or("distinct corpus width divisor is zero")?;
    while remaining != 0 {
        width = width
            .checked_add(1)
            .ok_or("distinct corpus width overflow")?;
        remaining = remaining
            .checked_div(26)
            .ok_or("distinct corpus divisor is zero")?;
    }
    let start = letters
        .len()
        .checked_sub(width)
        .ok_or("distinct corpus width exceeds u32")?;
    let mut body = String::from("plain stable");
    body.extend(letters.iter().skip(start).copied().map(char::from));
    Ok(body)
}

#[test]
fn native_text_cost_corpus_preserves_ascii_terms_beyond_u8() -> TestResult {
    for (id, expected) in [
        (0, "plain stableaa"),
        (25, "plain stableaz"),
        (26, "plain stableba"),
        (255, "plain stablejv"),
        (675, "plain stablezz"),
        (676, "plain stablebaa"),
        (32_768, "plain stablebwmi"),
    ] {
        assert_eq!(text_cost_distinct_body(id)?, expected);
    }
    let maximum = text_cost_distinct_body(u32::MAX)?;
    assert!(maximum.is_ascii());
    assert_eq!(maximum.split_ascii_whitespace().count(), 2);
    assert_eq!(maximum.len(), 19);
    Ok(())
}

fn native_text_services(
    root: &std::path::Path,
    inventory_reuse_limit: usize,
    inventory_batch_limit: usize,
    io_observer: &Arc<quanta_index_storage::ObjectIoObservation>,
) -> TestResult<(CostKernelFixture, InstanceSpec)> {
    use quanta_index_lexical::native::{NativeTextIndex, TextRecipe};
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    native_catalog_profile::attach(&catalog)?;
    let objects = Arc::new(
        FileObjectStore::open_managed_with_scratch(
            root.join("native-objects"),
            scratch.clone(),
            catalog.clone(),
        )?
        .with_io_observation(Arc::clone(io_observer)),
    );
    let spy = Arc::new(UpdateCostSpy::new_observed(
        Arc::clone(&objects),
        Arc::clone(io_observer),
    ));
    let admission = Digest([31; 32]);
    let recipe = TextRecipe::new(admission);
    let text_spec = InstanceSpec {
        kind: IndexKind([28; 16]),
        policy: recipe.output_policy(),
        ..spec()
    };
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: text_spec.kind,
            schema: text_spec.schema,
            format: text_spec.format,
        },
        NativeTextIndex::new(recipe)
            .with_inventory_reuse_limit(inventory_reuse_limit)?
            .with_inventory_batch_limit(inventory_batch_limit)?,
        |delta| Ok(delta.clone()),
    )?;
    let engine = Kernel::new(
        KernelServices::new(
            spy.clone(),
            spy.clone(),
            catalog.clone(),
            scratch,
            Duration::from_secs(30),
        )?,
        registry,
    );
    Ok(((engine, objects, catalog, spy), text_spec))
}

fn native_text_test_df(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
    term: &str,
) -> TestResult<u64> {
    use quanta_index_lexical::native::{CaseMode, NativeTextIndex, TextReadContext, TextRecipe};
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let binding = member(engine, objects, view)?;
    let result = NativeTextIndex::new(TextRecipe::new(Digest([31; 32]))).inspect_term_df(
        &binding,
        CaseMode::Sensitive,
        term,
        TextReadContext {
            ticket: ticket.record(),
            objects,
            maps: objects,
            budget: &mut budget(),
        },
    )?;
    ticket.release()?;
    Ok(result)
}

#[test]
fn native_text_byte_bounded_batch_confirmed_prefix_mixed_update_and_reopen() -> TestResult {
    use quanta_index_core::index::SourceTextAdmission;
    let directory = tempfile::tempdir()?;
    let observer = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    let ((engine, objects, catalog, spy), text_spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    let key = |id: u8| OperationKey {
        stream: StreamId([id; 16]),
        sequence: 1,
        ..operation(2)
    };
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let configure = key(20);
    let profile = engine.stage_profile(
        engine.begin_input(configure)?,
        &IndexProfile {
            instances: vec![text_spec],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(engine.submit(
        UpdateRequest {
            operation: configure,
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let admitted = SourceTextAdmission::Utf8 {
        policy: Digest([31; 32]),
    };
    // Two distinct bodies individually fit the public 8 MiB file contract,
    // but together exceed one draft window. Few tokens keep this a narrow law.
    let mut left = vec![b' '; 4 * 1024 * 1024];
    // Five terms keep this confirmed-prefix fault on real mapped
    // inventories after small inventories gained inline representation.
    left.extend_from_slice(b"left shared alpha beta gamma");
    let mut right = vec![b' '; 4 * 1024 * 1024];
    right.extend_from_slice(b"right shared alpha beta gamma");
    let initial_inputs = [
        SourceInput {
            path: "left.rs".into(),
            bytes: left,
            text: admitted,
        },
        SourceInput {
            path: "right.rs".into(),
            bytes: right.clone(),
            text: admitted,
        },
    ];
    let initial_key = key(21);
    let initial_ingress = engine.begin_input(initial_key)?;
    let mut references = Vec::new();
    // Source ingress and Text draft windows have independent bounds. Each
    // large body fits ingress; the Text window must split their combined draft.
    for input in &initial_inputs {
        let batch = engine.stage_sources(
            initial_ingress,
            std::slice::from_ref(input),
            &mut large_fixture_budget(),
        )?;
        assert_eq!(batch.len(), 1);
        references.extend(batch);
    }
    assert_eq!(references.len(), 2);
    let initial_request = native_text_cost_request(
        &engine,
        configured,
        source(&engine, configured)?,
        initial_key,
        1,
        vec![
            OwnerMutation {
                owner: large_owner(1),
                expected: None,
                next: references.first().copied(),
            },
            OwnerMutation {
                owner: large_owner(2),
                expected: None,
                next: references.get(1).copied(),
            },
        ],
    )?;
    spy.arm(
        source(&engine, configured)?.owners,
        std::collections::BTreeSet::new(),
    )?;
    let initial = published(block_on(
        engine.submit(initial_request, &mut large_fixture_budget()),
    )?)?;
    let measured = spy.disarm()?;
    assert_eq!(measured.single_map_stream.calls, 2);
    assert_eq!(measured.single_map_stream.roots, 2);
    assert_eq!(native_text_test_df(&engine, &objects, initial, "left")?, 1);
    assert_eq!(native_text_test_df(&engine, &objects, initial, "right")?, 1);
    assert_eq!(
        native_text_test_df(&engine, &objects, initial, "shared")?,
        2
    );
    let old_ticket = engine.pin_view(ViewSelection::Exact(initial), &budget())?;
    let initial_member = member(&engine, &objects, initial)?;
    let cut = old_ticket.view().source;

    let additions: Vec<SourceInput> = (3..=67u32)
        .map(|id| SourceInput {
            path: format!("f{id}.rs"),
            bytes: format!("fresh item{id} alpha beta gamma").into_bytes(),
            text: admitted,
        })
        .collect();
    let failed_key = key(22);
    let failed_ingress = engine.begin_input(failed_key)?;
    let mut staged = Vec::new();
    for window in additions.chunks(32) {
        let batch = engine.stage_sources(failed_ingress, window, &mut large_fixture_budget())?;
        assert_eq!(batch.len(), window.len());
        staged.extend(batch);
    }
    assert_eq!(staged.len(), 65);
    let mutations: Vec<_> = (3..=67u32)
        .zip(staged)
        .map(|(id, next)| OwnerMutation {
            owner: large_owner(id),
            expected: None,
            next: Some(next),
        })
        .collect();
    let failed = native_text_cost_request(&engine, initial, cut, failed_key, 1, mutations)?;
    spy.refuse_inventory_after_confirm_once
        .store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        block_on(engine.submit(failed.clone(), &mut large_fixture_budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::ResourceLimited))
    );
    assert_eq!(
        spy.confirmed_inventory_roots
            .load(std::sync::atomic::Ordering::Acquire),
        64
    );
    assert!(
        !spy.refuse_inventory_after_confirm_once
            .load(std::sync::atomic::Ordering::Acquire)
    );
    let latest = engine.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert_eq!(latest.view().identity, initial);
    assert_eq!(latest.view().source, cut);
    assert_eq!(
        member(&engine, &objects, initial)?.artifact,
        initial_member.artifact
    );
    assert_eq!(native_text_test_df(&engine, &objects, initial, "fresh")?, 0);
    latest.release()?;
    let before = observer.snapshot()?;
    assert_eq!(
        block_on(engine.submit(failed.clone(), &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::ResourceLimited))
    );
    let replay = observer.snapshot()?.checked_since(&before)?;
    assert_eq!(
        (
            replay.reserve_calls,
            replay.confirm_calls,
            replay.written_bytes
        ),
        (0, 0, 0)
    );

    // Use the new operation's own ingress; replayed terminal inputs are not
    // treated as implicit source-staging authority for a different operation.
    let mut mixed = vec![
        SourceInput {
            path: "left.rs".into(),
            bytes: b"changed mixed".to_vec(),
            text: admitted,
        },
        SourceInput {
            path: "renamed/right.rs".into(),
            bytes: right,
            text: admitted,
        },
    ];
    mixed.extend(additions);
    mixed.extend([
        SourceInput {
            path: "excluded.rs".into(),
            bytes: b"excluded only".to_vec(),
            text: SourceTextAdmission::Excluded,
        },
        SourceInput {
            path: "empty.rs".into(),
            bytes: b" \n\t".to_vec(),
            text: admitted,
        },
        SourceInput {
            path: "alias.rs".into(),
            bytes: b"fresh item3 alpha beta gamma".to_vec(),
            text: admitted,
        },
    ]);
    let success_key = key(23);
    let success_ingress = engine.begin_input(success_key)?;
    let mut staged = Vec::new();
    for window in mixed.chunks(32) {
        let batch = engine.stage_sources(success_ingress, window, &mut large_fixture_budget())?;
        assert_eq!(batch.len(), window.len());
        staged.extend(batch);
    }
    assert_eq!(staged.len(), 70);
    let mut mutations = Vec::new();
    for (id, next) in (1..=70u32).zip(staged) {
        mutations.push(OwnerMutation {
            owner: large_owner(id),
            expected: objects.lookup_object(
                cut.owners,
                &owner_map_key(large_owner(id)),
                &mut budget(),
            )?,
            next: Some(next),
        });
    }
    let success = native_text_cost_request(&engine, initial, cut, success_key, 1, mutations)?;
    let updated = published(block_on(
        engine.submit(success, &mut large_fixture_budget()),
    )?)?;
    let updated_member = member(&engine, &objects, updated)?;
    let latest = engine.pin_view(ViewSelection::Exact(updated), &budget())?;
    assert_eq!(native_text_test_df(&engine, &objects, updated, "left")?, 0);
    assert_eq!(native_text_test_df(&engine, &objects, updated, "right")?, 1);
    assert_eq!(
        native_text_test_df(&engine, &objects, updated, "shared")?,
        1
    );
    assert_eq!(
        native_text_test_df(&engine, &objects, updated, "changed")?,
        1
    );
    assert_eq!(
        native_text_test_df(&engine, &objects, updated, "fresh")?,
        66
    );
    assert_eq!(
        native_text_test_df(&engine, &objects, updated, "excluded")?,
        0
    );
    for (id, expected_path) in [
        (1, "left.rs"),
        (2, "renamed/right.rs"),
        (69, "empty.rs"),
        (70, "alias.rs"),
    ] {
        let source = objects
            .lookup_object(
                latest.view().source.owners,
                &owner_map_key(large_owner(id)),
                &mut budget(),
            )?
            .ok_or("mixed source missing")?;
        let record: SourceRecord =
            serde_json::from_slice(&objects.get(source, &mut budget())?.payload)?;
        assert_eq!(record.path, expected_path);
    }
    assert_eq!(native_text_test_df(&engine, &objects, initial, "left")?, 1);
    assert_eq!(native_text_test_df(&engine, &objects, initial, "fresh")?, 0);
    latest.release()?;
    old_ticket.release()?;
    assert_eq!(native_pin_count(&catalog)?, 0);
    drop(spy);
    drop(engine);
    drop(objects);
    drop(catalog);
    let ((reopened, objects, catalog, _spy), _spec) =
        native_text_services(directory.path(), 64, 64, &observer)?;
    assert_eq!(
        reopened.status(
            failed_key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Refused(Refusal::ResourceLimited)
    );
    assert_eq!(
        reopened.status(
            success_key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Published(updated)
    );
    assert_eq!(
        member(&reopened, &objects, updated)?.artifact,
        updated_member.artifact
    );
    assert_eq!(
        native_text_test_df(&reopened, &objects, updated, "fresh")?,
        66
    );
    assert_eq!(
        native_text_test_df(&reopened, &objects, initial, "shared")?,
        2
    );
    let before = observer.snapshot()?;
    assert_eq!(
        block_on(reopened.submit(failed, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::ResourceLimited))
    );
    let replay = observer.snapshot()?.checked_since(&before)?;
    assert_eq!(
        (
            replay.reserve_calls,
            replay.confirm_calls,
            replay.written_bytes
        ),
        (0, 0, 0)
    );
    assert_eq!(native_pin_count(&catalog)?, 0);
    Ok(())
}

fn one_owner_native_text_cost_case(
    owners: u32,
    corpus: TextCostCorpus,
    inventory_reuse_limit: usize,
    source_stage: TextSourceStage,
    inventory_batch_limit: usize,
) -> TestResult<(UpdateCostObservation, UpdateCostObservation)> {
    use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan};
    use quanta_index_core::index::{SourceInput, SourceTextAdmission, TicketRecord};
    use quanta_index_lexical::{
        native::{Bm25Policy, NativeTextIndex, TextReadContext, TextRecipe},
        planner::LexicalPlanner,
    };

    let owner_count = usize::try_from(owners)?;
    if owner_count == 0 {
        return Err("cost corpus must have an owner".into());
    }
    let directory = tempfile::tempdir()?;
    let initialization_start = std::time::Instant::now();
    let io_observer = Arc::new(quanta_index_storage::ObjectIoObservation::default());
    let ((engine, objects, _catalog, spy), text_spec) = native_text_services(
        directory.path(),
        inventory_reuse_limit,
        inventory_batch_limit,
        &io_observer,
    )?;
    let admission = Digest([31; 32]);
    let recipe = TextRecipe::new(admission);
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let profile = engine.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![text_spec],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let initialization_elapsed = initialization_start.elapsed();
    let admitted = SourceTextAdmission::Utf8 { policy: admission };
    let mut initial_mutations = Vec::with_capacity(owner_count);
    let mut unrelated = std::collections::BTreeSet::new();
    let mut old_source = None;
    let mut initial_inputs = Vec::with_capacity(owner_count);
    for id in 1..=owners {
        let path = format!("f{id}.rs");
        let distinct_text;
        let body = match (id, corpus) {
            (1, _) => b"needle old".as_slice(),
            (_, TextCostCorpus::Shared) => b"plain stable".as_slice(),
            (_, TextCostCorpus::Distinct) => {
                // Keep two ASCII tokens per owner while varying term identity.
                // Corpus preparation remains outside the publication clock.
                distinct_text = text_cost_distinct_body(id)?;
                distinct_text.as_bytes()
            }
        };
        initial_inputs.push(SourceInput {
            path,
            bytes: body.to_vec(),
            text: admitted,
        });
    }
    // Corpus generation is outside the capture clock; actual ingress/staging
    // and mutation sealing are included. Both routes use the original key.
    let capture_baseline = io_observer.snapshot()?;
    let capture_start = std::time::Instant::now();
    let ingress = engine.begin_input(operation(2))?;
    let source_stage_baseline = io_observer.snapshot()?;
    let source_stage_start = std::time::Instant::now();
    let mut staged = Vec::with_capacity(owner_count);
    match source_stage {
        TextSourceStage::Scalar => {
            for input in &initial_inputs {
                staged.push(engine.stage_source_text(
                    ingress,
                    &input.path,
                    &input.bytes,
                    input.text,
                    &mut budget(),
                )?);
            }
        }
        TextSourceStage::Batch32 => {
            for window in initial_inputs.chunks(32) {
                let refs = engine.stage_sources(ingress, window, &mut budget())?;
                assert_eq!(refs.len(), window.len());
                staged.extend(refs);
            }
        }
    }
    let source_stage_elapsed = source_stage_start.elapsed();
    let source_stage_io = io_observer
        .snapshot()?
        .checked_since(&source_stage_baseline)?;
    assert_eq!(staged.len(), owner_count);
    for (id, reference) in (1..=owners).zip(staged.iter().copied()) {
        if id == 1 {
            old_source = Some(reference);
        } else {
            let _inserted = unrelated.insert(reference);
        }
        initial_mutations.push(OwnerMutation {
            owner: large_owner(id),
            expected: None,
            next: Some(reference),
        });
    }
    let old_source = old_source.ok_or("missing hot source")?;
    let initial_request = native_text_cost_request(
        &engine,
        configured,
        source(&engine, configured)?,
        operation(2),
        1,
        initial_mutations,
    )?;
    let capture_elapsed = capture_start.elapsed();
    let capture_io = io_observer.snapshot()?.checked_since(&capture_baseline)?;
    // Independent decode/child/body gold binds every returned ordinal outside
    // both measured capture and publication clocks.
    for (input, reference) in initial_inputs.iter().zip(staged) {
        let record_object = objects.get(reference, &mut budget())?;
        let record: SourceRecord = serde_json::from_slice(&record_object.payload)?;
        assert_eq!(record.path, input.path);
        assert_eq!(record.text, input.text);
        assert_eq!(record.byte_len, u64::try_from(input.bytes.len())?);
        assert_eq!(record_object.registration.children, vec![record.bytes]);
        let body = objects.get(record.bytes, &mut budget())?;
        assert!(body.registration.children.is_empty());
        assert_eq!(body.payload.as_slice(), input.bytes.as_slice());
    }
    // Staging/request construction are outside these index-publication phases.
    spy.arm(
        source(&engine, configured)?.owners,
        std::collections::BTreeSet::new(),
    )?;
    let cold_baseline = io_observer.snapshot()?;
    let cold_sql_baseline = native_catalog_profile::snapshot()?;
    let cold_start = std::time::Instant::now();
    let initial = published(block_on(
        engine.submit(initial_request, &mut large_fixture_budget()),
    )?)?;
    let cold_elapsed = cold_start.elapsed();
    let cold_io = io_observer.snapshot()?.checked_since(&cold_baseline)?;
    let cold_observation = spy.disarm()?;
    let cold_sql = native_catalog_profile::since(cold_sql_baseline)?;
    let old_ticket = engine.pin_view(ViewSelection::Exact(initial), &budget())?;
    let old_member = member(&engine, &objects, initial)?;
    let old_cut = old_ticket.view().source;
    let delta_capture_baseline = io_observer.snapshot()?;
    let delta_capture_start = std::time::Instant::now();
    let new_source = engine.stage_source_text(
        engine.begin_input(operation(3))?,
        "f1.rs",
        b"needle new",
        admitted,
        &mut budget(),
    )?;
    let update = native_text_cost_request(
        &engine,
        initial,
        old_cut,
        operation(3),
        2,
        vec![OwnerMutation {
            owner: large_owner(1),
            expected: Some(old_source),
            next: Some(new_source),
        }],
    )?;
    let delta_capture_elapsed = delta_capture_start.elapsed();
    let delta_capture_io = io_observer
        .snapshot()?
        .checked_since(&delta_capture_baseline)?;
    spy.arm(old_cut.owners, unrelated)?;
    let delta_sql_baseline = native_catalog_profile::snapshot()?;
    let delta_start = std::time::Instant::now();
    let updated = published(block_on(engine.submit(update, &mut budget()))?)?;
    let delta_elapsed = delta_start.elapsed();
    let mut observed = spy.disarm()?;
    observed.cold_sql = cold_sql;
    observed.delta_sql = native_catalog_profile::since(delta_sql_baseline)?;
    observed.initialization_elapsed = Some(initialization_elapsed);
    observed.delta_capture_io = Some(delta_capture_io);
    observed.delta_capture_elapsed = Some(delta_capture_elapsed);
    observed.cold_object_io = Some(cold_io);
    observed.cold_elapsed = Some(cold_elapsed);
    observed.cold_occurrence_inputs = Some(cold_observation.occurrence_inputs);
    observed.cold_occurrence_install_calls = Some(cold_observation.occurrence_install_calls);
    observed.cold_single_map_stream = Some(cold_observation.single_map_stream);
    observed.cold_multi_map_stream = Some(cold_observation.multi_map_stream);
    observed.inventory_batch_limit = Some(inventory_batch_limit);
    observed.delta_elapsed = Some(delta_elapsed);
    observed.source_stage_io = Some(source_stage_io);
    observed.source_stage_elapsed = Some(source_stage_elapsed);
    observed.capture_io = Some(capture_io);
    observed.capture_elapsed = Some(capture_elapsed);
    assert_eq!(observed.prior_source_scans, 0);
    assert_eq!(observed.unrelated_record_reads, 0);

    let new_ticket = engine.pin_view(ViewSelection::Exact(updated), &budget())?;
    let new_member = member(&engine, &objects, updated)?;
    observed.cold_artifact = Some(old_member.artifact);
    observed.delta_artifact = Some(new_member.artifact);
    let reader = NativeTextIndex::new(recipe);
    let verify = |ticket: &TicketRecord,
                  binding: &MemberBinding,
                  view: ViewRef,
                  source: ObjectRef,
                  current_term: &str,
                  absent_term: &str|
     -> TestResult {
        let stats = reader.inspect_stats(
            binding,
            TextReadContext {
                ticket,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        assert_eq!(
            (stats.docs, stats.indexed_tokens),
            (
                u64::from(owners),
                u64::from(owners)
                    .checked_mul(2)
                    .ok_or("expected token count overflow")?,
            )
        );
        let unchanged_docs = u64::from(owners)
            .checked_sub(1)
            .ok_or("cost corpus must have an owner")?;
        let shared_term_df = match corpus {
            TextCostCorpus::Shared => unchanged_docs,
            TextCostCorpus::Distinct => 0,
        };
        let distinct_term_df = match corpus {
            TextCostCorpus::Distinct if owners >= 2 => 1,
            TextCostCorpus::Shared | TextCostCorpus::Distinct => 0,
        };
        for (term, expected) in [
            ("needle", 1),
            (current_term, 1),
            (absent_term, 0),
            ("plain", unchanged_docs),
            ("stable", shared_term_df),
            ("stableac", distinct_term_df),
        ] {
            assert_eq!(
                reader.inspect_term_df(
                    binding,
                    quanta_index_lexical::native::CaseMode::Sensitive,
                    term,
                    TextReadContext {
                        ticket,
                        objects: objects.as_ref(),
                        maps: objects.as_ref(),
                        budget: &mut budget(),
                    },
                )?,
                expected,
            );
        }
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword(current_term.into()));
        let plan = LexicalPlanner::plan(&query)?;
        let ranked = reader.search_content(
            binding,
            &plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            2,
            Bm25Policy { k1: 1.2, b: 0.75 },
            TextReadContext {
                ticket,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        assert_eq!(
            (ranked.view, ranked.artifact, ranked.matched),
            (view, binding.artifact, 1)
        );
        let [hit] = ranked.hits.as_slice() else {
            return Err("expected exactly one changed-owner text hit".into());
        };
        assert_eq!(
            (hit.owner, durable_text_source(hit.source)?),
            (large_owner(1), source)
        );
        let record: SourceRecord = serde_json::from_slice(
            &objects
                .get(durable_text_source(hit.source)?, &mut budget())?
                .payload,
        )?;
        assert_eq!(record.path, "f1.rs");
        // Ranking keeps its public top-k cap. Independent posting traversal
        // covers every owner, including rows beyond a ranked response window.
        let mut unchanged_query = LqQuery::empty(LqSpan::synthetic(0));
        unchanged_query.expr = LqExpr::Leaf(LqLeaf::Keyword("plain".into()));
        let unchanged_plan = LexicalPlanner::plan(&unchanged_query)?;
        let unchanged = reader.search_content(
            binding,
            &unchanged_plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            owners.min(1024),
            Bm25Policy { k1: 1.2, b: 0.75 },
            TextReadContext {
                ticket,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        assert_eq!(unchanged.matched, unchanged_docs);
        let actual_owners: std::collections::BTreeSet<_> =
            unchanged.hits.iter().map(|hit| hit.owner).collect();
        let expected_ranked: std::collections::BTreeSet<_> = (2..=owners)
            .map(large_owner)
            .take(usize::try_from(owners.min(1024))?)
            .collect();
        assert_eq!(actual_owners, expected_ranked);
        let mut visited = std::collections::BTreeSet::new();
        let scan = reader.visit_term(
            binding,
            quanta_index_lexical::native::CaseMode::Sensitive,
            "plain",
            TextReadContext {
                ticket,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
            |hit| {
                if !visited.insert(hit.owner) {
                    return Err(IndexError::BackendViolation(
                        "unchanged posting yielded duplicate owner".into(),
                    ));
                }
                if hit.tf != 1 || hit.doc_indexed_tokens != 2 || hit.positions != [0] {
                    return Err(IndexError::BackendViolation(
                        "unchanged posting differs from fixed position gold".into(),
                    ));
                }
                Ok(())
            },
        )?;
        assert_eq!(scan.df, unchanged_docs);
        let expected_owners: std::collections::BTreeSet<_> =
            (2..=owners).map(large_owner).collect();
        assert_eq!(visited, expected_owners);
        Ok(())
    };
    verify(
        old_ticket.record(),
        &old_member,
        initial,
        old_source,
        "old",
        "new",
    )?;
    verify(
        new_ticket.record(),
        &new_member,
        updated,
        new_source,
        "new",
        "old",
    )?;
    if owners == 65 {
        let cut = new_ticket.view().source;
        let second_source = objects
            .lookup_object(
                cut.owners,
                &quanta_index_core::index::owner_map_key(large_owner(2)),
                &mut budget(),
            )?
            .ok_or("second source owner missing before refusal")?;
        let rejected_hot = engine.stage_source_text(
            engine.begin_input(operation(4))?,
            "f1.rs",
            b"needle rejected",
            admitted,
            &mut budget(),
        )?;
        let rejected_second = engine.stage_source_text(
            engine.begin_input(operation(4))?,
            "f2.rs",
            b"plain altered",
            admitted,
            &mut budget(),
        )?;
        let failed = native_text_cost_request(
            &engine,
            updated,
            cut,
            operation(4),
            3,
            vec![
                OwnerMutation {
                    owner: large_owner(1),
                    expected: Some(new_source),
                    next: Some(rejected_hot),
                },
                OwnerMutation {
                    owner: large_owner(2),
                    expected: Some(second_source),
                    next: Some(rejected_second),
                },
            ],
        )?;
        spy.refuse_contribution_batch_once
            .store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(
            block_on(engine.submit(failed.clone(), &mut budget()))?,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::ResourceLimited)),
        );
        assert!(
            !spy.refuse_contribution_batch_once
                .load(std::sync::atomic::Ordering::Acquire)
        );
        let latest = engine.pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?;
        assert_eq!(latest.view().identity, updated);
        verify(
            latest.record(),
            &new_member,
            updated,
            new_source,
            "new",
            "old",
        )?;
        for term in ["rejected", "altered"] {
            assert_eq!(
                reader.inspect_term_df(
                    &new_member,
                    quanta_index_lexical::native::CaseMode::Sensitive,
                    term,
                    TextReadContext {
                        ticket: latest.record(),
                        objects: objects.as_ref(),
                        maps: objects.as_ref(),
                        budget: &mut budget(),
                    },
                )?,
                0,
            );
        }
        verify(
            old_ticket.record(),
            &old_member,
            initial,
            old_source,
            "old",
            "new",
        )?;
        let replay_before = io_observer.snapshot()?;
        assert_eq!(
            block_on(engine.submit(failed, &mut budget()))?,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::ResourceLimited)),
        );
        let replay = io_observer.snapshot()?.checked_since(&replay_before)?;
        assert_eq!(
            (
                replay.reserve_calls,
                replay.confirm_calls,
                replay.written_bytes
            ),
            (0, 0, 0)
        );
        latest.release()?;
        // A complete 64-owner group may have confirmed before the final scalar
        // tail fails. Neither its private top edits nor renamed paths publish.
        assert!(matches!(corpus, TextCostCorpus::Shared));
        let tail_operation = OperationKey {
            stream: StreamId([7; 16]),
            sequence: 1,
            ..operation(5)
        };
        let ids: Vec<u32> = (1..=owners).collect();
        let mut renames = Vec::new();
        for chunk in ids.chunks(32) {
            let inputs: Vec<quanta_index_core::index::SourceInput> = chunk
                .iter()
                .map(|id| quanta_index_core::index::SourceInput {
                    path: format!("renamed/f{id}.rs"),
                    bytes: if *id == 1 {
                        b"needle new".to_vec()
                    } else {
                        b"plain stable".to_vec()
                    },
                    text: admitted,
                })
                .collect();
            let references = engine.stage_sources(
                engine.begin_input(tail_operation)?,
                &inputs,
                &mut large_fixture_budget(),
            )?;
            assert_eq!(references.len(), chunk.len());
            for (id, next) in chunk.iter().zip(references) {
                let previous = objects
                    .lookup_object(
                        cut.owners,
                        &quanta_index_core::index::owner_map_key(large_owner(*id)),
                        &mut budget(),
                    )?
                    .ok_or("source owner missing before tail refusal")?;
                renames.push(OwnerMutation {
                    owner: large_owner(*id),
                    expected: Some(previous),
                    next: Some(next),
                });
            }
        }
        // The earlier refusal already owns its exact stream/event identity.
        // Stage and event identities both derive from this original new key.
        let failed_tail =
            native_text_cost_request(&engine, updated, cut, tail_operation, 1, renames)?;
        spy.refuse_contribution_scalar_once
            .store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(
            block_on(engine.submit(failed_tail.clone(), &mut large_fixture_budget()))?,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::ResourceLimited)),
        );
        assert!(
            !spy.refuse_contribution_scalar_once
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert_eq!(
            spy.confirmed_contributions_before_tail
                .load(std::sync::atomic::Ordering::Acquire),
            64
        );
        let latest = engine.pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?;
        assert_eq!(latest.view().identity, updated);
        verify(
            latest.record(),
            &new_member,
            updated,
            new_source,
            "new",
            "old",
        )?;
        for id in 1..=owners {
            let source = objects
                .lookup_object(
                    latest.view().source.owners,
                    &quanta_index_core::index::owner_map_key(large_owner(id)),
                    &mut budget(),
                )?
                .ok_or("published source missing after tail refusal")?;
            let record: SourceRecord =
                serde_json::from_slice(&objects.get(source, &mut budget())?.payload)?;
            assert_eq!(record.path, format!("f{id}.rs"));
        }
        let replay_before = io_observer.snapshot()?;
        assert_eq!(
            block_on(engine.submit(failed_tail, &mut budget()))?,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::ResourceLimited)),
        );
        let replay = io_observer.snapshot()?.checked_since(&replay_before)?;
        assert_eq!(
            (
                replay.reserve_calls,
                replay.confirm_calls,
                replay.written_bytes
            ),
            (0, 0, 0)
        );
        latest.release()?;
    }
    // A fixed one-owner deletion must not visit the unchanged source corpus.
    // Keep both old publications pinned while checking the new live statistics.
    let delete_operation = OperationKey {
        stream: StreamId([8; 16]),
        sequence: 1,
        ..operation(6)
    };
    let delete_capture_baseline = io_observer.snapshot()?;
    let delete_capture_start = std::time::Instant::now();
    let delete_request = native_text_cost_request(
        &engine,
        updated,
        new_ticket.view().source,
        delete_operation,
        1,
        vec![OwnerMutation {
            owner: large_owner(1),
            expected: Some(new_source),
            next: None,
        }],
    )?;
    let delete_capture_elapsed = delete_capture_start.elapsed();
    let delete_capture_io = io_observer
        .snapshot()?
        .checked_since(&delete_capture_baseline)?;
    let mut unrelated = std::collections::BTreeSet::new();
    for id in 2..=owners {
        let reference = objects
            .lookup_object(
                new_ticket.view().source.owners,
                &quanta_index_core::index::owner_map_key(large_owner(id)),
                &mut budget(),
            )?
            .ok_or("unchanged source missing before deletion")?;
        let _inserted = unrelated.insert(reference);
    }
    spy.arm(new_ticket.view().source.owners, unrelated)?;
    let delete_sql_baseline = native_catalog_profile::snapshot()?;
    let delete_start = std::time::Instant::now();
    let deleted = published(block_on(
        engine.submit(delete_request.clone(), &mut budget()),
    )?)?;
    let delete_elapsed = delete_start.elapsed();
    let mut delete_observed = spy.disarm()?;
    delete_observed.delete_sql = native_catalog_profile::since(delete_sql_baseline)?;
    delete_observed.capture_io = Some(delete_capture_io);
    delete_observed.capture_elapsed = Some(delete_capture_elapsed);
    delete_observed.delta_elapsed = Some(delete_elapsed);
    assert_eq!(delete_observed.prior_source_scans, 0);
    assert_eq!(delete_observed.unrelated_record_reads, 0);
    let deleted_ticket = engine.pin_view(ViewSelection::Exact(deleted), &budget())?;
    let deleted_member = member(&engine, &objects, deleted)?;
    delete_observed.delta_artifact = Some(deleted_member.artifact);
    let remaining = u64::from(owners)
        .checked_sub(1)
        .ok_or("delete corpus must have an owner")?;
    let stats = reader.inspect_stats(
        &deleted_member,
        TextReadContext {
            ticket: deleted_ticket.record(),
            objects: objects.as_ref(),
            maps: objects.as_ref(),
            budget: &mut budget(),
        },
    )?;
    assert_eq!(
        (stats.docs, stats.indexed_tokens),
        (
            remaining,
            remaining
                .checked_mul(2)
                .ok_or("delete token gold overflow")?
        )
    );
    for (term, expected) in [
        ("needle", 0),
        ("new", 0),
        ("old", 0),
        ("plain", remaining),
        (
            "stable",
            if matches!(corpus, TextCostCorpus::Shared) {
                remaining
            } else {
                0
            },
        ),
    ] {
        assert_eq!(
            reader.inspect_term_df(
                &deleted_member,
                quanta_index_lexical::native::CaseMode::Sensitive,
                term,
                TextReadContext {
                    ticket: deleted_ticket.record(),
                    objects: objects.as_ref(),
                    maps: objects.as_ref(),
                    budget: &mut budget(),
                },
            )?,
            expected,
        );
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword(term.into()));
        let ranked = reader.search_content(
            &deleted_member,
            &LexicalPlanner::plan(&query)?,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            owners.min(1024),
            Bm25Policy { k1: 1.2, b: 0.75 },
            TextReadContext {
                ticket: deleted_ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        assert_eq!(
            (ranked.view, ranked.artifact, ranked.matched),
            (deleted, deleted_member.artifact, expected)
        );
        let actual: std::collections::BTreeSet<_> =
            ranked.hits.iter().map(|hit| hit.owner).collect();
        let expected: std::collections::BTreeSet<_> = if expected == 0 {
            std::collections::BTreeSet::new()
        } else {
            (2..=owners)
                .map(large_owner)
                .take(usize::try_from(owners.min(1024))?)
                .collect()
        };
        assert_eq!(actual, expected);
        for hit in &ranked.hits {
            let reference = objects
                .lookup_object(
                    new_ticket.view().source.owners,
                    &quanta_index_core::index::owner_map_key(hit.owner),
                    &mut budget(),
                )?
                .ok_or("gold source missing after deletion")?;
            assert_eq!(durable_text_source(hit.source)?, reference);
        }
        let mut visited = std::collections::BTreeSet::new();
        let scan = reader.visit_term(
            &deleted_member,
            quanta_index_lexical::native::CaseMode::Sensitive,
            term,
            TextReadContext {
                ticket: deleted_ticket.record(),
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
            |hit| {
                let position = u32::from(term == "stable");
                if !visited.insert(hit.owner)
                    || hit.tf != 1
                    || hit.doc_indexed_tokens != 2
                    || hit.positions != [position]
                {
                    return Err(IndexError::BackendViolation(
                        "delete posting differs from fixed owner/position gold".into(),
                    ));
                }
                let reference = objects
                    .lookup_object(
                        new_ticket.view().source.owners,
                        &owner_map_key(hit.owner),
                        &mut budget(),
                    )?
                    .ok_or_else(|| {
                        IndexError::BackendViolation("delete posting gold source missing".into())
                    })?;
                if hit.source != reference {
                    return Err(IndexError::BackendViolation(
                        "delete posting changed an unchanged source identity".into(),
                    ));
                }
                Ok(())
            },
        )?;
        assert_eq!(scan.df, ranked.matched);
        let expected_all: std::collections::BTreeSet<_> = if ranked.matched == 0 {
            std::collections::BTreeSet::new()
        } else {
            (2..=owners).map(large_owner).collect()
        };
        assert_eq!(visited, expected_all);
    }
    assert_eq!(
        objects.lookup_object(
            deleted_ticket.view().source.owners,
            &quanta_index_core::index::owner_map_key(large_owner(1)),
            &mut budget(),
        )?,
        None,
    );
    let latest = engine.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert_eq!(latest.view().identity, deleted);
    latest.release()?;
    verify(
        old_ticket.record(),
        &old_member,
        initial,
        old_source,
        "old",
        "new",
    )?;
    verify(
        new_ticket.record(),
        &new_member,
        updated,
        new_source,
        "new",
        "old",
    )?;
    let replay_before = io_observer.snapshot()?;
    assert_eq!(
        published(block_on(engine.submit(delete_request, &mut budget()))?)?,
        deleted,
    );
    let replay = io_observer.snapshot()?.checked_since(&replay_before)?;
    assert_eq!(
        (
            replay.reserve_calls,
            replay.confirm_calls,
            replay.written_bytes
        ),
        (0, 0, 0),
    );
    if owners == 65 {
        let key = OperationKey {
            stream: StreamId([9; 16]),
            sequence: 1,
            ..operation(7)
        };
        let ingress = engine.begin_input(key)?;
        let mut mutations = Vec::with_capacity(owner_count);
        for id in 1..=owners {
            let expected = objects.lookup_object(
                deleted_ticket.view().source.owners,
                &quanta_index_core::index::owner_map_key(large_owner(id)),
                &mut budget(),
            )?;
            mutations.push(OwnerMutation {
                owner: large_owner(id),
                expected,
                next: if id == 1 { Some(new_source) } else { expected },
            });
        }
        // Inline staging has no JSON wrapper installation tail. Refuse the
        // actual fallible producer after64 rows; a consumed prefix must never
        // become a sealed manifest or change the selected source view.
        let staged_prefix = std::cell::Cell::new(0usize);
        let refused = mutations
            .iter()
            .take(64)
            .cloned()
            .map(|mutation| {
                staged_prefix.set(
                    staged_prefix
                        .get()
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?,
                );
                Ok(mutation)
            })
            .chain(std::iter::once(Err(IndexError::ResourceLimited)));
        assert!(matches!(
            engine.stage_mutations(ingress, refused, &mut budget()),
            Err(IndexError::ResourceLimited)
        ));
        assert_eq!(staged_prefix.get(), 64);
        let latest = engine.pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?;
        assert_eq!(latest.view().identity, deleted);
        assert_eq!(latest.view().source, deleted_ticket.view().source);
        assert_eq!(
            member(&engine, &objects, deleted)?.artifact,
            deleted_member.artifact
        );
        latest.release()?;
        // Retry the original ingress from the complete input. Only its returned
        // complete manifest is then an input; no failed prefix is adopted.
        let complete =
            engine.stage_mutations(ingress, mutations.iter().cloned().map(Ok), &mut budget())?;
        assert_eq!(complete.count, u64::from(owners));
        let mut cursor = quanta_index_core::index::OwnerMutationCursor::new(
            complete,
            objects.as_ref(),
            objects.as_ref(),
        );
        for expected in &mutations {
            assert_eq!(cursor.next_mutation(&mut budget())?, Some(expected.clone()));
        }
        assert!(cursor.next_mutation(&mut budget())?.is_none());
        assert_eq!(cursor.sealed_set(), Some(complete));
        let latest = engine.pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?;
        assert_eq!(latest.view().identity, deleted);
        latest.release()?;
    }
    deleted_ticket.release()?;
    old_ticket.release()?;
    new_ticket.release()?;
    Ok((observed, delete_observed))
}

#[test]
fn native_text_one_owner_patch_cost_is_independent_of_unrelated_owner_count() -> TestResult {
    let (one, delete_one) = one_owner_native_text_cost_case(
        1,
        TextCostCorpus::Shared,
        64,
        TextSourceStage::Batch32,
        64,
    )?;
    let (sixty_four, delete_sixty_four) = one_owner_native_text_cost_case(
        64,
        TextCostCorpus::Shared,
        64,
        TextSourceStage::Batch32,
        64,
    )?;
    let (distinct, delete_distinct) = one_owner_native_text_cost_case(
        64,
        TextCostCorpus::Distinct,
        64,
        TextSourceStage::Batch32,
        64,
    )?;
    let (sixty_five, delete_sixty_five) = one_owner_native_text_cost_case(
        65,
        TextCostCorpus::Shared,
        64,
        TextSourceStage::Batch32,
        64,
    )?;
    let (five_twelve, delete_five_twelve) = one_owner_native_text_cost_case(
        512,
        TextCostCorpus::Shared,
        64,
        TextSourceStage::Batch32,
        64,
    )?;
    let (distinct_five_twelve, delete_distinct_five_twelve) = one_owner_native_text_cost_case(
        512,
        TextCostCorpus::Distinct,
        64,
        TextSourceStage::Batch32,
        64,
    )?;
    let (distinct_uncached, delete_distinct_uncached) = one_owner_native_text_cost_case(
        512,
        TextCostCorpus::Distinct,
        0,
        TextSourceStage::Batch32,
        64,
    )?;
    let (uncached, delete_uncached) = one_owner_native_text_cost_case(
        64,
        TextCostCorpus::Shared,
        0,
        TextSourceStage::Batch32,
        64,
    )?;
    let (scalar, delete_scalar) = one_owner_native_text_cost_case(
        64,
        TextCostCorpus::Shared,
        64,
        TextSourceStage::Scalar,
        64,
    )?;
    let (distinct_scalar_map, delete_distinct_scalar_map) = one_owner_native_text_cost_case(
        512,
        TextCostCorpus::Distinct,
        64,
        TextSourceStage::Batch32,
        1,
    )?;
    assert_eq!(
        distinct_five_twelve.cold_artifact,
        distinct_scalar_map.cold_artifact
    );
    assert_eq!(
        distinct_five_twelve.delta_artifact,
        distinct_scalar_map.delta_artifact
    );
    assert_eq!(
        delete_distinct_five_twelve.delta_artifact,
        delete_distinct_scalar_map.delta_artifact
    );
    let grouped = distinct_five_twelve
        .cold_object_io
        .as_ref()
        .ok_or("grouped cold IO missing")?;
    let scalar_map = distinct_scalar_map
        .cold_object_io
        .as_ref()
        .ok_or("scalar map cold IO missing")?;
    assert_eq!(grouped.write_calls, scalar_map.write_calls);
    assert_eq!(grouped.written_bytes, scalar_map.written_bytes);
    // Every two-token inventory fits inline. Neither draft-window setting
    // creates a physical inventory map, so there are no map submissions to batch.
    // The byte-window/confirmed-prefix fixture separately exercises real trees.
    assert_eq!(
        distinct_five_twelve
            .cold_single_map_stream
            .as_ref()
            .ok_or("grouped map shape missing")?
            .calls,
        0
    );
    assert_eq!(
        distinct_scalar_map
            .cold_single_map_stream
            .as_ref()
            .ok_or("scalar map shape missing")?
            .calls,
        0
    );
    assert_eq!(grouped.reserve_calls, scalar_map.reserve_calls);
    assert_eq!(grouped.barrier_calls, scalar_map.barrier_calls);
    // The changed file has fixed token/position gold. Unrelated owners must
    // not add map submissions, logical roots or consumed edit rows. Physical
    // node counts may vary with trie shape; they are observed separately.
    for (delta, deleted) in [
        (&sixty_four, &delete_sixty_four),
        (&distinct, &delete_distinct),
        (&sixty_five, &delete_sixty_five),
        (&five_twelve, &delete_five_twelve),
        (&distinct_five_twelve, &delete_distinct_five_twelve),
        (&distinct_uncached, &delete_distinct_uncached),
        (&uncached, &delete_uncached),
        (&scalar, &delete_scalar),
        (&distinct_scalar_map, &delete_distinct_scalar_map),
    ] {
        assert_eq!(
            one.single_map_stream.shape(),
            delta.single_map_stream.shape()
        );
        assert_eq!(one.multi_map_stream.shape(), delta.multi_map_stream.shape());
        assert_eq!(
            delete_one.single_map_stream.shape(),
            deleted.single_map_stream.shape()
        );
        assert_eq!(
            delete_one.multi_map_stream.shape(),
            deleted.multi_map_stream.shape()
        );
    }
    assert_eq!(one.scanned_entries, sixty_four.scanned_entries);
    assert_eq!(one.scanned_entries, distinct.scanned_entries);
    assert_eq!(one.scanned_entries, sixty_five.scanned_entries);
    assert_eq!(one.scanned_entries, five_twelve.scanned_entries);
    assert_eq!(one.scanned_entries, distinct_five_twelve.scanned_entries);
    assert_eq!(
        distinct_five_twelve.cold_artifact,
        distinct_uncached.cold_artifact
    );
    assert_eq!(
        distinct_five_twelve.delta_artifact,
        distinct_uncached.delta_artifact
    );
    assert_eq!(
        delete_distinct_five_twelve.delta_artifact,
        delete_distinct_uncached.delta_artifact
    );
    assert_eq!(
        distinct_five_twelve.scanned_entries,
        distinct_uncached.scanned_entries
    );
    let cached_distinct_io = distinct_five_twelve
        .cold_object_io
        .as_ref()
        .ok_or("cached distinct IO missing")?;
    let uncached_distinct_io = distinct_uncached
        .cold_object_io
        .as_ref()
        .ok_or("uncached distinct IO missing")?;
    assert_eq!(
        cached_distinct_io.write_calls,
        uncached_distinct_io.write_calls
    );
    assert_eq!(
        cached_distinct_io.written_bytes,
        uncached_distinct_io.written_bytes
    );
    assert!(cached_distinct_io.reserve_calls < uncached_distinct_io.reserve_calls);
    assert!(cached_distinct_io.barrier_calls < uncached_distinct_io.barrier_calls);
    assert_eq!(sixty_four.cold_artifact, uncached.cold_artifact);
    assert_eq!(sixty_four.delta_artifact, uncached.delta_artifact);
    assert_eq!(sixty_four.cold_artifact, scalar.cold_artifact);
    assert_eq!(sixty_four.delta_artifact, scalar.delta_artifact);
    assert_eq!(
        delete_sixty_four.delta_artifact,
        delete_scalar.delta_artifact
    );
    let batched_sources = sixty_four
        .source_stage_io
        .as_ref()
        .ok_or("batched source-stage observation missing")?;
    let scalar_sources = scalar
        .source_stage_io
        .as_ref()
        .ok_or("scalar source-stage observation missing")?;
    assert!(batched_sources.reserve_calls < scalar_sources.reserve_calls);
    assert_eq!(batched_sources.written_bytes, scalar_sources.written_bytes);
    assert_eq!(batched_sources.write_calls, scalar_sources.write_calls);
    assert_eq!(
        delete_sixty_four.delta_artifact,
        delete_uncached.delta_artifact
    );
    for deleted in [
        &delete_sixty_four,
        &delete_distinct,
        &delete_sixty_five,
        &delete_five_twelve,
        &delete_distinct_five_twelve,
        &delete_distinct_uncached,
        &delete_uncached,
        &delete_scalar,
    ] {
        assert_eq!(delete_one.scanned_entries, deleted.scanned_entries);
        assert_eq!(deleted.occurrence_inputs, 0);
    }
    assert_eq!(delete_one.occurrence_inputs, 0);
    let reused_cold = sixty_four
        .cold_object_io
        .as_ref()
        .ok_or("reused cold observation missing")?;
    let uncached_cold = uncached
        .cold_object_io
        .as_ref()
        .ok_or("uncached cold observation missing")?;
    assert!(reused_cold.reserve_calls < uncached_cold.reserve_calls);
    assert!(reused_cold.reserve_objects < uncached_cold.reserve_objects);
    assert_eq!(reused_cold.written_bytes, uncached_cold.written_bytes);
    let singleton_baseline = one
        .cold_object_io
        .as_ref()
        .ok_or("one-owner cold custody observation missing")?
        .single_object_reserves;
    for (observed, tail_allowance) in [(&sixty_four, 0), (&distinct, 0), (&sixty_five, 2)] {
        let cold = observed
            .cold_object_io
            .as_ref()
            .ok_or("multi-owner cold custody observation missing")?;
        // Changed owner count must not create one singleton installation each.
        // The 65-owner case allows one contribution tail and one map-group tail.
        assert!(
            cold.single_object_reserves
                <= singleton_baseline
                    .checked_add(tail_allowance)
                    .ok_or("singleton tail allowance overflowed")?
        );
    }
    for (corpus, owners, reuse_limit, stage, observed, deleted) in [
        ("shared", 1u32, 64, "batch32", &one, &delete_one),
        ("shared", 64, 64, "batch32", &sixty_four, &delete_sixty_four),
        ("distinct", 64, 64, "batch32", &distinct, &delete_distinct),
        ("shared", 65, 64, "batch32", &sixty_five, &delete_sixty_five),
        (
            "shared",
            512,
            64,
            "batch32",
            &five_twelve,
            &delete_five_twelve,
        ),
        (
            "distinct",
            512,
            64,
            "batch32",
            &distinct_five_twelve,
            &delete_distinct_five_twelve,
        ),
        (
            "distinct",
            512,
            0,
            "batch32",
            &distinct_uncached,
            &delete_distinct_uncached,
        ),
        ("shared", 64, 0, "batch32", &uncached, &delete_uncached),
        ("shared", 64, 64, "scalar", &scalar, &delete_scalar),
        (
            "distinct",
            512,
            64,
            "batch32",
            &distinct_scalar_map,
            &delete_distinct_scalar_map,
        ),
    ] {
        print_native_text_cost_case(corpus, owners, reuse_limit, stage, observed, deleted)?;
    }
    Ok(())
}

fn print_native_text_cost_case(
    corpus: &str,
    owners: u32,
    reuse_limit: usize,
    stage: &str,
    observed: &UpdateCostObservation,
    deleted: &UpdateCostObservation,
) -> TestResult {
    let map_batch_limit = observed
        .inventory_batch_limit
        .ok_or("map batch policy missing")?;
    // Fixed independent position gold: each document has two unique tokens;
    // raw/folded surfaces share positions0/1. A same-attempt confirmed cache
    // installs those four ordinal inputs once, then each distinct body may
    // reference them without another durable occurrence group.
    let expected_occurrence_inputs = if reuse_limit == 0 {
        u64::from(owners)
            .checked_mul(4)
            .ok_or("occurrence input gold overflow")?
    } else {
        4
    };
    let expected_occurrence_calls = if reuse_limit == 0 {
        u64::from(owners)
    } else {
        1
    };
    assert_eq!(
        observed.cold_occurrence_inputs,
        Some(expected_occurrence_inputs)
    );
    assert_eq!(
        observed.cold_occurrence_install_calls,
        Some(expected_occurrence_calls)
    );
    assert!(observed.occurrence_install_calls > 0);
    for (phase, single, multi) in [
        (
            "cold",
            observed
                .cold_single_map_stream
                .as_ref()
                .ok_or("cold single map cost missing")?,
            observed
                .cold_multi_map_stream
                .as_ref()
                .ok_or("cold multi map cost missing")?,
        ),
        (
            "delta",
            &observed.single_map_stream,
            &observed.multi_map_stream,
        ),
        (
            "delete",
            &deleted.single_map_stream,
            &deleted.multi_map_stream,
        ),
    ] {
        std::io::Write::write_fmt(
            &mut std::io::stdout().lock(),
            format_args!(
                "native_map_stream corpus={corpus} owners={owners} reuse_limit={reuse_limit} stage={stage} map_batch_limit={map_batch_limit} phase={phase} single_calls={} single_roots={} single_edits={} single_reserves={} single_barriers={} multi_calls={} multi_roots={} multi_edits={} multi_reserves={} multi_barriers={}\n",
                single.calls,
                single.roots,
                single.edits,
                single.reserve_calls,
                single.barrier_calls,
                multi.calls,
                multi.roots,
                multi.edits,
                multi.reserve_calls,
                multi.barrier_calls,
            ),
        )?;
    }
    std::io::Write::write_fmt(
        &mut std::io::stdout().lock(),
        format_args!(
            "native_occurrence_io corpus={corpus} owners={owners} reuse_limit={reuse_limit} stage={stage} map_batch_limit={map_batch_limit} cold_inputs={} cold_install_calls={} delta_inputs={} delta_install_calls={}\n",
            expected_occurrence_inputs,
            expected_occurrence_calls,
            observed.occurrence_inputs,
            observed.occurrence_install_calls,
        ),
    )?;
    let stage_calls = if stage == "batch32" {
        u64::from(owners).div_ceil(32)
    } else {
        u64::from(owners)
            .checked_mul(2)
            .ok_or("stage call gold overflow")?
    };
    let source_stage = observed
        .source_stage_io
        .as_ref()
        .ok_or("source-stage observation missing")?;
    assert_eq!(source_stage.reserve_calls, stage_calls);
    let capture = observed
        .capture_io
        .as_ref()
        .ok_or("capture observation missing")?;
    // Source staging is a subset of ingress capture, never an additive
    // disjoint phase. Capture also includes sealing the owner mutations.
    assert!(capture.reserve_calls > source_stage.reserve_calls);
    assert!(capture.written_bytes > source_stage.written_bytes);
    // These fixed <=65-owner keys need at most two manifest-map groups.
    // Intake must not regress to one durable custody call per mutation.
    let capture_call_limit = stage_calls
        .checked_add(u64::from(owners).div_ceil(64))
        .and_then(|calls| calls.checked_add(2))
        .ok_or("capture call budget overflowed")?;
    if owners <= 65 {
        assert!(capture.reserve_calls <= capture_call_limit);
    }
    assert_eq!(capture.confirm_calls, capture.reserve_calls);
    let initialization = observed
        .initialization_elapsed
        .ok_or("initialization clock missing")?;
    let initial_capture = observed
        .capture_elapsed
        .ok_or("initial capture clock missing")?;
    let initial_publish = observed
        .cold_elapsed
        .ok_or("initial publication clock missing")?;
    let delta_capture = observed
        .delta_capture_elapsed
        .ok_or("delta capture clock missing")?;
    let delta_publish = observed
        .delta_elapsed
        .ok_or("delta publication clock missing")?;
    let delete_capture = deleted
        .capture_elapsed
        .ok_or("delete capture clock missing")?;
    let delete_publish = deleted
        .delta_elapsed
        .ok_or("delete publication clock missing")?;
    // These work intervals are disjoint. Source staging is nested in capture
    // and must not be added again. Corpus generation and independent gold
    // checks are deliberately excluded; this is not daemon wall latency.
    let initial_work = initialization
        .checked_add(initial_capture)
        .and_then(|elapsed| elapsed.checked_add(initial_publish))
        .ok_or("initial work duration overflow")?;
    let delta_work = delta_capture
        .checked_add(delta_publish)
        .ok_or("delta work duration overflow")?;
    let delete_work = delete_capture
        .checked_add(delete_publish)
        .ok_or("delete work duration overflow")?;
    std::io::Write::write_fmt(
        &mut std::io::stdout().lock(),
        format_args!(
            "native_selected_work scope=kernel_setup_ingress_publish oracle_and_corpus_generation=excluded corpus={corpus} owners={owners} reuse_limit={reuse_limit} stage={stage} map_batch_limit={map_batch_limit} initial_us={} initialization_us={} initial_capture_us={} initial_publish_us={} delta_us={} delta_capture_us={} delta_publish_us={} delete_us={} delete_capture_us={} delete_publish_us={}\n",
            initial_work.as_micros(),
            initialization.as_micros(),
            initial_capture.as_micros(),
            initial_publish.as_micros(),
            delta_work.as_micros(),
            delta_capture.as_micros(),
            delta_publish.as_micros(),
            delete_work.as_micros(),
            delete_capture.as_micros(),
            delete_publish.as_micros(),
        ),
    )?;
    for (phase, sample, elapsed) in [
        (
            "source_stage",
            observed.source_stage_io.as_ref(),
            observed.source_stage_elapsed,
        ),
        (
            "ingress_capture",
            observed.capture_io.as_ref(),
            observed.capture_elapsed,
        ),
        (
            "cold_index_publish",
            observed.cold_object_io.as_ref(),
            observed.cold_elapsed,
        ),
        (
            "one_owner_delta_capture",
            observed.delta_capture_io.as_ref(),
            observed.delta_capture_elapsed,
        ),
        (
            "one_owner_delta_publish",
            observed.object_io.as_ref(),
            observed.delta_elapsed,
        ),
        (
            "one_owner_delete_capture",
            deleted.capture_io.as_ref(),
            deleted.capture_elapsed,
        ),
        (
            "one_owner_delete_publish",
            deleted.object_io.as_ref(),
            deleted.delta_elapsed,
        ),
    ] {
        let sample = sample.ok_or("native object IO observation missing")?;
        let elapsed = elapsed.ok_or("native index phase clock missing")?;
        assert!(sample.write_calls > 0 && sample.written_bytes > 0);
        assert!(sample.file_sync_calls > 0 && sample.directory_sync_calls > 0);
        assert_eq!(
            (
                sample.failed_writes,
                sample.failed_file_syncs,
                sample.failed_directory_syncs,
                sample.failed_barriers
            ),
            (0, 0, 0, 0)
        );
        assert!(sample.reserve_calls > 0);
        assert_eq!(sample.reserve_calls, sample.confirm_calls);
        assert_eq!((sample.failed_reserves, sample.failed_confirms), (0, 0));
        assert_eq!(sample.reserve_objects, sample.confirm_objects);
        assert_eq!(sample.single_object_reserves, sample.single_object_confirms);
        assert!(sample.single_object_reserves <= sample.reserve_calls);
        assert!(sample.reserve_objects >= sample.reserve_calls);
        // One synchronous publisher and the native non-reentrant authority:
        // reached IO/custody calls do not overlap in this fixture.
        assert!(sample.elapsed_install_call_time()? <= elapsed);
        // Diagnostic observed-mode values; not a latency SLO or device IO.
        std::io::Write::write_fmt(
            &mut std::io::stdout().lock(),
            format_args!(
                "native_object_io authority=managed corpus={corpus} owners={owners} reuse_limit={reuse_limit} stage={stage} map_batch_limit={map_batch_limit} phase={phase} elapsed_us={} write_calls={} written_bytes={} file_sync={} directory_sync={} barrier={} write_us={} file_sync_us={} directory_sync_us={} barrier_us={} reserve={} confirm={} reserve_us={} confirm_us={} reserve_objects={} confirm_objects={} single_reserves={} single_confirms={}\n",
                elapsed.as_micros(),
                sample.write_calls,
                sample.written_bytes,
                sample.file_sync_calls,
                sample.directory_sync_calls,
                sample.barrier_calls,
                sample.write_elapsed.as_micros(),
                sample.file_sync_elapsed.as_micros(),
                sample.directory_sync_elapsed.as_micros(),
                sample.barrier_elapsed.as_micros(),
                sample.reserve_calls,
                sample.confirm_calls,
                sample.reserve_elapsed.as_micros(),
                sample.confirm_elapsed.as_micros(),
                sample.reserve_objects,
                sample.confirm_objects,
                sample.single_object_reserves,
                sample.single_object_confirms
            ),
        )?;
    }
    Ok(())
}

/// Explicit engine-scale acceptance, distinct from daemon/producer qualification.
///
/// Uses the same managed stores, cost observer and independent output gold as
/// the ordinary cost law; it does not introduce a second benchmark runner.
#[test]
#[ignore = "explicit 4,096-owner Native engine-scale acceptance"]
fn native_text_large_initial_delta_delete_preserve_gold_and_changed_work() -> TestResult {
    for (label, corpus) in [
        ("shared", TextCostCorpus::Shared),
        ("distinct", TextCostCorpus::Distinct),
    ] {
        let (control, control_delete) =
            one_owner_native_text_cost_case(64, corpus, 64, TextSourceStage::Batch32, 64)?;
        let (large, large_delete) =
            one_owner_native_text_cost_case(4096, corpus, 64, TextSourceStage::Batch32, 64)?;
        for (small, expanded) in [(&control, &large), (&control_delete, &large_delete)] {
            assert_eq!(expanded.prior_source_scans, 0);
            assert_eq!(expanded.unrelated_record_reads, 0);
            assert_eq!(expanded.scanned_entries, small.scanned_entries);
            assert_eq!(
                expanded.single_map_stream.shape(),
                small.single_map_stream.shape()
            );
            assert_eq!(
                expanded.multi_map_stream.shape(),
                small.multi_map_stream.shape()
            );
        }
        assert_eq!(large_delete.occurrence_inputs, 0);
        print_native_text_cost_case(label, 64, 64, "batch32", &control, &control_delete)?;
        print_native_text_cost_case(label, 4096, 64, "batch32", &large, &large_delete)?;
    }
    Ok(())
}

#[test]
fn native_text_catalog_profile_separates_statement_commit_from_custody() -> TestResult {
    let profile_guard = native_catalog_profile::Guard::start()?;
    let (observed, deleted) = one_owner_native_text_cost_case(
        128,
        TextCostCorpus::Distinct,
        64,
        TextSourceStage::Batch32,
        64,
    )?;
    for (phase, sql, io, wall) in [
        (
            "cold",
            observed.cold_sql,
            observed.cold_object_io.as_ref(),
            observed.cold_elapsed,
        ),
        (
            "delta",
            observed.delta_sql,
            observed.object_io.as_ref(),
            observed.delta_elapsed,
        ),
        (
            "delete",
            deleted.delete_sql,
            deleted.object_io.as_ref(),
            deleted.delta_elapsed,
        ),
    ] {
        let sql = sql.ok_or("missing explicitly enabled SQL profile")?;
        let io = io.ok_or("missing custody sample")?;
        let wall = wall.ok_or("missing publication clock")?;
        assert!(sql.read.calls > 0);
        assert!(sql.write.calls > 0);
        // Every successful paired managed custody call commits its transaction.
        // Publication also has control transactions; those are not custody.
        assert!(
            sql.commit.calls
                >= io
                    .reserve_calls
                    .checked_add(io.confirm_calls)
                    .ok_or("custody count overflow")?
        );
        std::io::Write::write_fmt(
            &mut std::io::stdout().lock(),
            format_args!(
                "native_catalog_sql_profile scope=kernel_publish statements=profile_elapsed_not_cpu phase={phase} wall_us={} reserve_us={} confirm_us={} commit_calls={} commit_us={} read_calls={} read_us={} write_calls={} write_us={} other_calls={} other_us={}\n",
                wall.as_micros(),
                io.reserve_elapsed.as_micros(),
                io.confirm_elapsed.as_micros(),
                sql.commit.calls,
                sql.commit.elapsed.as_micros(),
                sql.read.calls,
                sql.read.elapsed.as_micros(),
                sql.write.calls,
                sql.write.elapsed.as_micros(),
                sql.other.calls,
                sql.other.elapsed.as_micros(),
            ),
        )?;
    }
    assert!(native_catalog_profile::Guard::start().is_err());
    drop(profile_guard);
    assert!(native_catalog_profile::snapshot()?.is_none());
    Ok(())
}

// Terminal replay may recompute the immutable request identity, but must not
// load a source, projection, index artifact, or map node after work eviction.
struct ReplayOnlyStorage {
    inner: Arc<FileObjectStore>,
    get_observed: Arc<std::sync::atomic::AtomicBool>,
    get_failure: IndexError,
}

impl ImmutableObjectPort for ReplayOnlyStorage {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "terminal replay wrote an object".into(),
        ))
    }

    fn install(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "terminal replay installed an object".into(),
        ))
    }

    fn install_batch(
        &self,
        _inputs: &[quanta_index_core::index::ObjectBatchInput],
        _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        Err(IndexError::BackendViolation(
            "terminal replay installed objects".into(),
        ))
    }

    fn get(
        &self,
        _reference: ObjectRef,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StoredObject, IndexError> {
        self.get_observed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Err(self.get_failure.clone())
    }
}

impl PersistentMapPort for ReplayOnlyStorage {
    fn visit_diff(
        &self,
        _before: quanta_index_core::index::MapRoot,
        _after: quanta_index_core::index::MapRoot,
        _range: quanta_index_core::index::ports::MapDiffRange<'_>,
        _visitor: &mut dyn quanta_index_core::index::ports::MapDiffVisitor,
        _budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        Err(IndexError::BackendViolation(
            "terminal replay visited a map diff".into(),
        ))
    }

    fn lookup(
        &self,
        _root: quanta_index_core::index::MapRoot,
        _key: &[u8],
        _budget: &mut WorkBudget,
    ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
        Err(IndexError::BackendViolation(
            "terminal replay looked up a map".into(),
        ))
    }

    fn scan(
        &self,
        _root: quanta_index_core::index::MapRoot,
        _after: Option<&[u8]>,
        _limit: u32,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::MapPage, IndexError> {
        Err(IndexError::BackendViolation(
            "terminal replay scanned a map".into(),
        ))
    }

    fn apply(
        &self,
        _base: quanta_index_core::index::MapRoot,
        _edits: &[quanta_index_core::index::MapEdit],
        _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::MapWrite, IndexError> {
        Err(IndexError::BackendViolation(
            "terminal replay edited a map".into(),
        ))
    }

    fn apply_batch(
        &self,
        _requests: &[quanta_index_core::index::MapBatchRequest],
        _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::MapBatchWrite, IndexError> {
        Err(IndexError::BackendViolation(
            "terminal replay edited maps".into(),
        ))
    }

    fn apply_stream(
        &self,
        _bases: &[quanta_index_core::index::MapRoot],
        _edits: &mut dyn quanta_index_core::index::ports::MapEditCursor,
        _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
        Err(IndexError::BackendViolation(
            "terminal replay streamed map edits".into(),
        ))
    }
}

#[test]
fn owned_terminal_replay_survives_work_retirement_restart_and_fresh_create() -> TestResult {
    use quanta_index_core::index::{
        OwnedWorkReport, WorkStatus,
        ports::{CatalogExactHistoryPort, CatalogTerminalWorkRetentionPort},
    };

    let root = tempfile::tempdir()?;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (engine, objects, catalog) = owner_kernel(root.path(), Arc::clone(&calls))?;
    let intent = staged_owned_job(&engine, &objects, &catalog)?;
    let assignment = engine
        .acquire_owned_work(&intent, owned_policy(), &budget())?
        .ok_or("missing owned assignment")?;
    let grant = require_owned_grant(
        engine.begin_owned_work(&assignment)?,
        "missing owned dispatch",
    )?;
    let original = grant.assignment.request.clone();
    let complete = published(block_on(
        engine.submit_owned_work(original.clone(), &mut budget()),
    )?)?;
    engine.finish_owned_work(&grant, OwnedWorkReport::Reconcile)?;
    assert_eq!(
        engine.work_status(&intent, &budget())?,
        WorkStatus::Completed(complete)
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    assert_eq!(
        catalog.advance_exact_history_floor(workspace(), 1, complete.sequence, &mut budget(),)?,
        complete.sequence,
    );
    let retired = catalog.retire_terminal_work_page(workspace(), 1, None, 16, &mut budget())?;
    assert_eq!(
        (retired.examined, retired.retired, retired.blocked),
        (1, 1, 0)
    );
    assert_eq!(engine.work_status(&intent, &budget())?, WorkStatus::Unknown);
    let old_views = catalog.retire_exact_history_page(workspace(), 1, None, 16, &mut budget())?;
    assert!(old_views.retired >= 1);
    drop(engine);
    drop(objects);
    drop(catalog);

    let objects = Arc::new(FileObjectStore::open(root.path().join("native-objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(
        root.path(),
        Duration::from_millis(100),
    )?);
    let terminal_get_observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let replay_only = Arc::new(ReplayOnlyStorage {
        inner: Arc::clone(&objects),
        get_observed: Arc::clone(&terminal_get_observed),
        get_failure: IndexError::BackendViolation("terminal replay read an object".into()),
    });
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.path().join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let replay_kernel = Kernel::new(
        KernelServices::new(
            replay_only.clone(),
            replay_only,
            Arc::clone(&catalog),
            scratch,
            Duration::from_secs(30),
        )?,
        AdapterRegistry::new(),
    );
    assert_eq!(
        replay_kernel.status(
            original.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
        )?,
        OperationStatus::Published(complete)
    );
    let mut altered = original.clone();
    assert_ne!(altered.expected_view, complete);
    altered.expected_view = complete;
    assert_eq!(
        block_on(replay_kernel.resume_owned_work(&altered, &mut budget())),
        Err(IndexError::DigestConflict),
    );
    assert_eq!(
        block_on(replay_kernel.submit_owned_work(altered, &mut budget())),
        Err(IndexError::DigestConflict),
    );
    let mut cancelled = budget();
    cancelled.request().cancel_handle().cancel();
    assert_eq!(
        block_on(replay_kernel.resume_owned_work(&original, &mut cancelled)),
        Err(IndexError::Cancelled),
    );
    assert_eq!(
        block_on(replay_kernel.resume_owned_work(&original, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Published(complete)),
    );
    assert_eq!(
        block_on(replay_kernel.submit_owned_work(original.clone(), &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Published(complete)),
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!terminal_get_observed.load(std::sync::atomic::Ordering::SeqCst));
    drop(replay_kernel);
    drop(objects);
    drop(catalog);

    let (engine, _objects, _catalog) = owner_kernel(root.path(), Arc::clone(&calls))?;
    let remove_key = operation(3);
    let remove_profile = engine.stage_profile(
        engine.begin_input(remove_key)?,
        &IndexProfile { instances: vec![] },
        &mut budget(),
    )?;
    let removed = published(block_on(engine.submit(
        UpdateRequest {
            operation: remove_key,
            expected_view: complete,
            kind: UpdateKind::Reconfigure {
                profile: remove_profile,
            },
        },
        &mut budget(),
    ))?)?;
    let add_key = operation(4);
    let add_profile = engine.stage_profile(
        engine.begin_input(add_key)?,
        &IndexProfile {
            instances: vec![async_spec()],
        },
        &mut budget(),
    )?;
    let _recreated = published(block_on(engine.submit(
        UpdateRequest {
            operation: add_key,
            expected_view: removed,
            kind: UpdateKind::Reconfigure {
                profile: add_profile,
            },
        },
        &mut budget(),
    ))?)?;
    assert_eq!(engine.work_status(&intent, &budget())?, WorkStatus::Pending);
    let fresh = engine
        .acquire_owned_work(&intent, owned_policy(), &budget())?
        .ok_or("fresh work did not rearm")?;
    assert_ne!(fresh.request.operation, original.operation);
    assert_eq!(
        block_on(engine.submit_owned_work(original, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Published(complete)),
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn pending_owned_resume_propagates_durable_request_read_failure() -> TestResult {
    use quanta_index_core::index::WorkScope;

    for failure in [
        IndexError::Storage("injected durable request read failure".into()),
        IndexError::CorruptObject("injected durable request corruption".into()),
    ] {
        let root = tempfile::tempdir()?;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (engine, objects, catalog) = owner_kernel(root.path(), Arc::clone(&calls))?;
        let intent = staged_owned_job(&engine, &objects, &catalog)?;
        let assignment = engine
            .acquire_owned_work(&intent, owned_policy(), &budget())?
            .ok_or("missing managed assignment")?;
        let grant = require_owned_grant(
            engine.begin_owned_work(&assignment)?,
            "missing managed dispatch",
        )?;
        let request = grant.assignment.request.clone();
        let UpdateKind::Materialize { claim } = &request.kind else {
            return Err("expected materialize request".into());
        };
        let WorkScope::Owner { expected, .. } = claim.intent.scope else {
            return Err("expected owner work".into());
        };
        let mut children: Vec<ObjectRef> = expected
            .source
            .into_iter()
            .chain(expected.projection)
            .collect();
        children.sort_unstable();
        children.dedup();
        let ingress = catalog.open_ingress(request.operation)?;
        let stored = objects.put(&serde_json::to_vec(&request)?, &children, &mut budget())?;
        catalog.reserve_inputs(ingress, std::slice::from_ref(&stored))?;
        catalog.confirm_inputs(ingress, std::slice::from_ref(&stored))?;
        // Owned admission seals this confirmed root in its own transaction.
        assert!(matches!(
            catalog.admit_owned(
                ImmutableOperation {
                    key: request.operation,
                    request: stored.reference,
                    source_mode: request.kind.source_mode(),
                },
                ingress,
                &request,
                objects.as_ref(),
                &mut budget(),
            )?,
            AdmissionOutcome::New(_)
        ));
        assert!(matches!(
            engine.status(
                request.operation,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
            )?,
            OperationStatus::Pending(_)
        ));
        let get_observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let read_rejecting = Arc::new(ReplayOnlyStorage {
            inner: Arc::clone(&objects),
            get_observed: Arc::clone(&get_observed),
            get_failure: failure.clone(),
        });
        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.path().join("replay-scratch"),
            16 * 1024 * 1024 * 1024,
        )?);
        let replay_kernel = Kernel::new(
            KernelServices::new(
                read_rejecting.clone(),
                read_rejecting,
                Arc::clone(&catalog),
                scratch,
                Duration::from_secs(30),
            )?,
            AdapterRegistry::new(),
        );
        assert_eq!(
            block_on(replay_kernel.resume_owned_work(&request, &mut budget())),
            Err(failure)
        );
        assert!(get_observed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(matches!(
            replay_kernel.status(
                request.operation,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
            )?,
            OperationStatus::Pending(_)
        ));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    Ok(())
}

mod genesis_sink_test {
    use super::*;
    use quanta_index_core::index::{
        BootstrapRef, ObjectBatchInput, ObjectInstallScope, ObjectNamespaceBinding,
        ReclaimCandidate, ReclaimDrainPage, ReclaimPermit, ReclaimTarget, ReclaimWorkPage,
        RegisteredObject, RetiringRef, StoredObject,
        ports::{
            CatalogGenesisPort, CatalogReclaimPort, ManagedObjectAuthority,
            ManagedObjectInstallPort, ObjectWriteSink,
        },
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct GenesisObjectSpy {
        inner: Arc<FileObjectStore>,
        direct_put_calls: AtomicUsize,
        sink_install_calls: AtomicUsize,
        cancel_after_install: bool,
    }

    impl ImmutableObjectPort for GenesisObjectSpy {
        fn identify(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            self.inner.identify(payload, children, budget)
        }

        fn put(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            let _previous = self.direct_put_calls.fetch_add(1, Ordering::SeqCst);
            Err(IndexError::BackendViolation(
                "genesis used direct object put without reserved custody".into(),
            ))
        }

        fn install(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            let installed = self.inner.install(payload, children, sink, budget)?;
            let _previous = self.sink_install_calls.fetch_add(1, Ordering::SeqCst);
            if self.cancel_after_install {
                budget.request().cancel_handle().cancel();
            }
            Ok(installed)
        }

        fn install_batch(
            &self,
            inputs: &[ObjectBatchInput],
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            let installed = self.inner.install_batch(inputs, sink, budget)?;
            let _previous = self.sink_install_calls.fetch_add(1, Ordering::SeqCst);
            Ok(installed)
        }

        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            self.inner.get(reference, budget)
        }
    }

    #[test]
    fn genesis_profile_uses_reserved_object_installation_before_publication() -> TestResult {
        let root = tempfile::tempdir()?;
        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.path().join("scratch"),
            16 * 1024 * 1024,
        )?);
        let catalog = Arc::new(SqliteCatalog::open(
            root.path(),
            Duration::from_millis(100),
        )?);
        let objects = Arc::new(FileObjectStore::open_managed_with_scratch(
            root.path().join("native-objects"),
            scratch.clone(),
            catalog.clone(),
        )?);
        let spy = Arc::new(GenesisObjectSpy {
            inner: Arc::clone(&objects),
            direct_put_calls: AtomicUsize::new(0),
            sink_install_calls: AtomicUsize::new(0),
            cancel_after_install: false,
        });
        let engine = Kernel::new(
            KernelServices::new(
                spy.clone(),
                objects.clone(),
                Arc::clone(&catalog),
                scratch,
                Duration::from_secs(30),
            )?,
            AdapterRegistry::new(),
        );
        let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
        let ticket = engine.pin_view(ViewSelection::Exact(genesis), &budget())?;
        ticket.validate()?;
        assert_eq!(ticket.view().identity, genesis);
        let profile_object = objects.get(ticket.view().profile, &mut budget())?;
        assert!(profile_object.registration.children.is_empty());
        let profile: IndexProfile = serde_json::from_slice(&profile_object.payload)?;
        assert!(profile.instances.is_empty());
        assert_eq!(spy.direct_put_calls.load(Ordering::SeqCst), 0);
        assert_eq!(spy.sink_install_calls.load(Ordering::SeqCst), 1);
        ticket.release()?;
        Ok(())
    }

    #[test]
    fn cancelled_after_genesis_install_retains_custody_without_publishing_head() -> TestResult {
        let root = tempfile::tempdir()?;
        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.path().join("scratch"),
            16 * 1024 * 1024,
        )?);
        let catalog = Arc::new(SqliteCatalog::open(
            root.path(),
            Duration::from_millis(100),
        )?);
        let objects = Arc::new(FileObjectStore::open_managed_with_scratch(
            root.path().join("native-objects"),
            scratch.clone(),
            catalog.clone(),
        )?);
        let spy = Arc::new(GenesisObjectSpy {
            inner: Arc::clone(&objects),
            direct_put_calls: AtomicUsize::new(0),
            sink_install_calls: AtomicUsize::new(0),
            cancel_after_install: true,
        });
        let engine = Kernel::new(
            KernelServices::new(
                spy.clone(),
                objects,
                Arc::clone(&catalog),
                scratch,
                Duration::from_secs(30),
            )?,
            AdapterRegistry::new(),
        );

        assert_eq!(
            engine.create_workspace(workspace(), 1, &mut budget()),
            Err(IndexError::Cancelled)
        );
        assert_eq!(spy.direct_put_calls.load(Ordering::SeqCst), 0);
        assert_eq!(spy.sink_install_calls.load(Ordering::SeqCst), 1);
        let connection = rusqlite::Connection::open(catalog.path())?;
        let heads: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_workspace_v1", [], |row| {
                row.get(0)
            })?;
        let stages: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_bootstrap_v1", [], |row| {
                row.get(0)
            })?;
        let confirmed: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_bootstrap_object_v1 WHERE confirmed=1",
            [],
            |row| row.get(0),
        )?;
        assert_eq!((heads, stages, confirmed), (0, 1, 1));
        Ok(())
    }

    #[test]
    fn failed_genesis_write_budget_resumes_same_bootstrap_after_restart() -> TestResult {
        let root = tempfile::tempdir()?;
        let first_stage = {
            let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
                root.path().join("scratch"),
                16 * 1024 * 1024,
            )?);
            let catalog = Arc::new(SqliteCatalog::open(
                root.path(),
                Duration::from_millis(100),
            )?);
            let objects = Arc::new(FileObjectStore::open_managed_with_scratch(
                root.path().join("native-objects"),
                scratch.clone(),
                catalog.clone(),
            )?);
            let engine = Kernel::new(
                KernelServices::new(
                    objects.clone(),
                    objects.clone(),
                    Arc::clone(&catalog),
                    scratch,
                    Duration::from_secs(30),
                )?,
                AdapterRegistry::new(),
            );
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?;
            let mut denied = WorkBudget::new(
                WorkLimits {
                    reads: 100,
                    writes: 0,
                    bytes: 1_000_000,
                    mutations: 100,
                },
                deadline,
            );
            assert_eq!(
                engine.create_workspace(workspace(), 1, &mut denied),
                Err(IndexError::ResourceLimited)
            );
            let stage = catalog.begin_bootstrap(workspace(), 1, objects.as_ref(), &mut budget())?;
            assert!(matches!(
                objects.get(stage.profile, &mut budget()),
                Err(IndexError::CorruptObject(_))
            ));
            let connection = rusqlite::Connection::open(catalog.path())?;
            let heads: i64 =
                connection.query_row("SELECT COUNT(*) FROM native_workspace_v1", [], |row| {
                    row.get(0)
                })?;
            let stages: i64 =
                connection.query_row("SELECT COUNT(*) FROM native_bootstrap_v1", [], |row| {
                    row.get(0)
                })?;
            let installed: i64 = connection.query_row(
                "SELECT COUNT(*) FROM native_bootstrap_object_v1",
                [],
                |row| row.get(0),
            )?;
            assert_eq!((heads, stages, installed), (0, 1, 0));
            stage
        };

        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.path().join("scratch"),
            16 * 1024 * 1024,
        )?);
        let catalog = Arc::new(SqliteCatalog::open(
            root.path(),
            Duration::from_millis(100),
        )?);
        let objects = Arc::new(FileObjectStore::open_managed_with_scratch(
            root.path().join("native-objects"),
            scratch.clone(),
            catalog.clone(),
        )?);
        let resumed_stage =
            catalog.begin_bootstrap(workspace(), 1, objects.as_ref(), &mut budget())?;
        assert_eq!(resumed_stage, first_stage);
        let engine = Kernel::new(
            KernelServices::new(
                objects.clone(),
                objects.clone(),
                Arc::clone(&catalog),
                scratch,
                Duration::from_secs(30),
            )?,
            AdapterRegistry::new(),
        );
        let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
        let ticket = engine.pin_view(ViewSelection::Exact(genesis), &budget())?;
        ticket.validate()?;
        assert_eq!(ticket.view().profile, first_stage.profile);
        let stored = objects.get(ticket.view().profile, &mut budget())?;
        let profile: IndexProfile = serde_json::from_slice(&stored.payload)?;
        assert!(profile.instances.is_empty());
        ticket.release()?;
        let connection = rusqlite::Connection::open(catalog.path())?;
        let heads: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_workspace_v1", [], |row| {
                row.get(0)
            })?;
        let stages: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_bootstrap_v1", [], |row| {
                row.get(0)
            })?;
        assert_eq!((heads, stages), (1, 0));
        Ok(())
    }
    fn require_equal<T: std::fmt::Debug + PartialEq>(
        actual: &T,
        expected: &T,
        label: &str,
    ) -> TestResult {
        if actual == expected {
            Ok(())
        } else {
            Err(format!("{label}: actual={actual:?}, expected={expected:?}").into())
        }
    }

    struct RefuseConfirmationCatalog {
        catalog: Arc<SqliteCatalog>,
    }

    impl ManagedObjectInstallPort for RefuseConfirmationCatalog {
        fn reserve_managed(
            &self,
            scope: ObjectInstallScope,
            objects: &[RegisteredObject],
            budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            self.catalog.reserve_managed(scope, objects, budget)
        }

        fn confirm_managed(
            &self,
            scope: ObjectInstallScope,
            objects: &[RegisteredObject],
            budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            if matches!(scope, ObjectInstallScope::Bootstrap(_)) {
                return Err(IndexError::Storage("injected post-fsync crash cut".into()));
            }
            self.catalog.confirm_managed(scope, objects, budget)
        }
    }

    impl CatalogReclaimPort for RefuseConfirmationCatalog {
        fn reclaim_work_page(
            &self,
            after: Option<ObjectRef>,
            limit: u32,
            budget: &mut WorkBudget,
        ) -> Result<ReclaimWorkPage, IndexError> {
            self.catalog.reclaim_work_page(after, limit, budget)
        }

        fn retire_candidate(
            &self,
            candidate: ReclaimCandidate,
            budget: &mut WorkBudget,
        ) -> Result<Option<RetiringRef>, IndexError> {
            self.catalog.retire_candidate(candidate, budget)
        }

        fn enter_reclaiming(
            &self,
            retiring: RetiringRef,
            target: ReclaimTarget,
            budget: &mut WorkBudget,
        ) -> Result<ReclaimPermit, IndexError> {
            self.catalog.enter_reclaiming(retiring, target, budget)
        }

        fn validate_reclaiming(
            &self,
            permit: ReclaimPermit,
            budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            self.catalog.validate_reclaiming(permit, budget)
        }

        fn mark_unlinked(
            &self,
            permit: ReclaimPermit,
            budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            self.catalog.mark_unlinked(permit, budget)
        }

        fn drain_unlinked(
            &self,
            permit: ReclaimPermit,
            limit: u32,
            budget: &mut WorkBudget,
        ) -> Result<ReclaimDrainPage, IndexError> {
            self.catalog.drain_unlinked(permit, limit, budget)
        }
    }

    impl ManagedObjectAuthority for RefuseConfirmationCatalog {
        fn bind_namespace(&self, binding: ObjectNamespaceBinding) -> Result<(), IndexError> {
            self.catalog.bind_namespace(binding)
        }
    }

    struct RefuseConfirmation {
        stage: BootstrapRef,
    }

    impl ObjectWriteSink for RefuseConfirmation {
        fn managed_scope(&self) -> Option<ObjectInstallScope> {
            Some(ObjectInstallScope::Bootstrap(self.stage))
        }

        fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "managed store called untrusted reserve callback".into(),
            ))
        }

        fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation(
                "managed store called untrusted confirm callback".into(),
            ))
        }

        fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::InvalidInput(
                "single genesis profile only".into(),
            ))
        }

        fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::InvalidInput(
                "single genesis profile only".into(),
            ))
        }
    }

    #[test]
    fn post_fsync_preconfirmation_genesis_reopens_and_retries_same_profile() -> TestResult {
        let root = tempfile::tempdir()?;
        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.path().join("scratch"),
            16 * 1024 * 1024,
        )?);
        let catalog = Arc::new(SqliteCatalog::open(
            root.path(),
            Duration::from_millis(100),
        )?);
        let objects = Arc::new(FileObjectStore::open_managed_with_scratch(
            root.path().join("native-objects"),
            scratch.clone(),
            Arc::new(RefuseConfirmationCatalog {
                catalog: catalog.clone(),
            }),
        )?);
        let profile = serde_json::to_vec(&IndexProfile {
            instances: Vec::new(),
        })?;
        let reference = objects.identify(&profile, &[], &mut budget())?.reference;
        let stage = catalog.begin_bootstrap(workspace(), 1, objects.as_ref(), &mut budget())?;
        require_equal(&stage.profile, &reference, "frozen bootstrap profile")?;
        let cut = objects.install(&profile, &[], &RefuseConfirmation { stage }, &mut budget());
        require_equal(
            &matches!(cut, Err(IndexError::Storage(message)) if message == "injected post-fsync crash cut"),
            &true,
            "post-fsync cut",
        )?;
        require_equal(
            &objects.get(reference, &mut budget())?.payload,
            &profile,
            "physical profile before reopen",
        )?;
        let catalog_path = root.path().join("catalog/catalog-v1.sqlite");
        let connection = rusqlite::Connection::open(&catalog_path)?;
        let key = reference.0.0.to_vec();
        let before: (i64, i64, i64, i64, i64) = connection.query_row(
            "SELECT (SELECT COUNT(*) FROM native_bootstrap_v1),
                    (SELECT COUNT(*) FROM native_bootstrap_object_v1 WHERE confirmed=0),
                    (SELECT COUNT(*) FROM native_object_install_v1 WHERE object=?1),
                    (SELECT COUNT(*) FROM native_object_v1 WHERE object=?1),
                    (SELECT COUNT(*) FROM native_workspace_v1)",
            rusqlite::params![&key],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        require_equal(
            &before,
            &(1, 1, 1, 0, 0),
            "bootstrap, reservation, provisional, registered, workspace before reopen",
        )?;
        drop(connection);
        drop(catalog);
        drop(objects);
        drop(scratch);

        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.path().join("scratch"),
            16 * 1024 * 1024,
        )?);
        let reopened_catalog = Arc::new(SqliteCatalog::open(
            root.path(),
            Duration::from_millis(100),
        )?);
        let reopened_objects = Arc::new(FileObjectStore::open_managed_with_scratch(
            root.path().join("native-objects"),
            scratch.clone(),
            reopened_catalog.clone(),
        )?);
        let replayed_stage = reopened_catalog.begin_bootstrap(
            workspace(),
            1,
            reopened_objects.as_ref(),
            &mut budget(),
        )?;
        require_equal(&replayed_stage, &stage, "same durable stage after reopen")?;
        let engine = Kernel::new(
            KernelServices::new(
                reopened_objects.clone(),
                reopened_objects.clone(),
                reopened_catalog.clone(),
                scratch,
                Duration::from_secs(30),
            )?,
            AdapterRegistry::new(),
        );
        let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
        let ticket = engine.pin_view(ViewSelection::Exact(genesis), &budget())?;
        ticket.validate()?;
        require_equal(
            &ticket.view().profile,
            &reference,
            "pinned profile after reopen",
        )?;
        require_equal(
            &reopened_objects.get(reference, &mut budget())?.payload,
            &profile,
            "physical profile after reopen",
        )?;
        ticket.release()?;
        drop(engine);
        drop(reopened_catalog);
        let connection = rusqlite::Connection::open(&catalog_path)?;
        let after: (i64, i64, i64, i64) = connection.query_row(
            "SELECT (SELECT COUNT(*) FROM native_bootstrap_v1),
                    (SELECT COUNT(*) FROM native_bootstrap_object_v1),
                    (SELECT COUNT(*) FROM native_object_install_v1 WHERE object=?1),
                    (SELECT COUNT(*) FROM native_workspace_v1)",
            rusqlite::params![&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        require_equal(
            &after,
            &(0, 0, 0, 1),
            "bootstrap, reservation, provisional, workspace after retry",
        )?;
        Ok(())
    }
}

// Coupled Catalog/managed-store behavior; not a capacity benchmark.
mod physical_gc_test {
    use super::*;
    use quanta_index_core::index::{
        ObjectInstallScope, ReclaimWork, RegisteredObject,
        ports::{CatalogReclaimPort, ManagedObjectReclaimPort, ObjectWriteSink},
    };
    use std::{
        fmt::Write as _,
        path::{Path, PathBuf},
    };

    // Callbacks are deliberately unusable. A managed installation must use
    // its paired catalog and the exact scope, rather than trust this sink.
    struct ScopeOnly(ObjectInstallScope);
    impl ObjectWriteSink for ScopeOnly {
        fn managed_scope(&self) -> Option<ObjectInstallScope> {
            Some(self.0)
        }
        fn reserve(&self, _: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation("untrusted sink called".into()))
        }
        fn installed(&self, _: &RegisteredObject) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation("untrusted sink called".into()))
        }
        fn reserve_batch(&self, _: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation("untrusted sink called".into()))
        }
        fn installed_batch(&self, _: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation("untrusted sink called".into()))
        }
    }

    fn open_managed(root: &Path) -> TestResult<(Arc<SqliteCatalog>, Arc<FileObjectStore>)> {
        let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.join("scratch"),
            32 * 1024 * 1024,
        )?);
        let objects = Arc::new(FileObjectStore::open_managed_with_scratch(
            root.join("managed"),
            scratch,
            catalog.clone(),
        )?);
        Ok((catalog, objects))
    }

    fn initialize(root: &Path) -> TestResult<(Arc<SqliteCatalog>, Arc<FileObjectStore>)> {
        let (catalog, objects) = open_managed(root)?;
        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.join("kernel-scratch"),
            32 * 1024 * 1024,
        )?);
        let kernel = Kernel::new(
            KernelServices::new(
                objects.clone(),
                objects.clone(),
                catalog.clone(),
                scratch,
                Duration::from_secs(30),
            )?,
            AdapterRegistry::new(),
        );
        let _view = kernel.create_workspace(workspace(), 1, &mut budget())?;
        Ok((catalog, objects))
    }

    fn work_for(catalog: &SqliteCatalog, object: ObjectRef) -> TestResult<Option<ReclaimWork>> {
        let mut after = None;
        loop {
            let page = catalog.reclaim_work_page(after, 8, &mut budget())?;
            for item in page.items {
                after = Some(item.object());
                if item.object() == object {
                    return Ok(Some(item));
                }
            }
            if !page.has_more {
                return Ok(None);
            }
        }
    }

    // The object layout is digest hex under its first-byte shard.
    // Calculate the path independently of FileObjectStore::path_for.
    fn object_file(root: &Path, object: ObjectRef) -> TestResult<PathBuf> {
        let bytes = (object.0).0;
        let mut name = String::with_capacity(64);
        for byte in bytes {
            write!(&mut name, "{byte:02x}")?;
        }
        let shard = format!("{:02x}", bytes[0]);
        Ok(root.join("managed/objects").join(shard).join(name))
    }

    fn assert_object_absent(path: &Path) -> TestResult {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
            Ok(_) => Err(std::io::Error::other(format!(
                "reclaimed object still occupies {}",
                path.display()
            ))
            .into()),
        }
    }

    fn delete_one(
        root: &Path,
        catalog: &SqliteCatalog,
        objects: &FileObjectStore,
        object: ObjectRef,
    ) -> TestResult {
        let file = object_file(root, object)?;
        assert!(file.is_file(), "object was absent before physical reclaim");
        let work = work_for(catalog, object)?.ok_or("missing durable GC work")?;
        let page = objects
            .reclaim_one(work, 32, &mut budget())?
            .ok_or("unreferenced object unexpectedly readopted")?;
        assert!(!page.has_more);
        assert_object_absent(&file)?;
        assert!(matches!(
            objects.get(object, &mut budget()),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn shared_child_survives_first_parent_and_is_queued_after_last_physical_unlink() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, objects) = initialize(root.path())?;
        let child_stage = catalog.open_ingress(operation(101))?;
        let left_stage = catalog.open_ingress(operation(102))?;
        let right_stage = catalog.open_ingress(operation(103))?;
        let child = objects.install(
            b"shared child",
            &[],
            &ScopeOnly(ObjectInstallScope::Ingress(child_stage)),
            &mut budget(),
        )?;
        let left = objects.install(
            b"left parent",
            &[child.reference],
            &ScopeOnly(ObjectInstallScope::Ingress(left_stage)),
            &mut budget(),
        )?;
        let right = objects.install(
            b"right parent",
            &[child.reference],
            &ScopeOnly(ObjectInstallScope::Ingress(right_stage)),
            &mut budget(),
        )?;
        catalog.discard_inputs(child_stage, &mut budget())?;
        catalog.discard_inputs(left_stage, &mut budget())?;
        delete_one(root.path(), &catalog, &objects, left.reference)?;
        assert_eq!(
            objects.get(child.reference, &mut budget())?.payload,
            b"shared child"
        );
        if let Some(ReclaimWork::Candidate(candidate)) = work_for(&catalog, child.reference)? {
            assert!(
                catalog
                    .retire_candidate(candidate, &mut budget())?
                    .is_none()
            );
        }
        catalog.discard_inputs(right_stage, &mut budget())?;
        delete_one(root.path(), &catalog, &objects, right.reference)?;
        delete_one(root.path(), &catalog, &objects, child.reference)?;
        assert!(work_for(&catalog, child.reference)?.is_none());
        Ok(())
    }

    #[test]
    fn seventy_child_drain_restarts_after_unlink_and_commits_each_edge_once() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, objects) = initialize(root.path())?;
        let child_stage = catalog.open_ingress(operation(110))?;
        let parent_stage = catalog.open_ingress(operation(111))?;
        let mut children = Vec::new();
        for id in 0_u32..70 {
            children.push(
                objects
                    .install(
                        &id.to_be_bytes(),
                        &[],
                        &ScopeOnly(ObjectInstallScope::Ingress(child_stage)),
                        &mut budget(),
                    )?
                    .reference,
            );
        }
        children.sort_unstable();
        let parent = objects.install(
            b"seventy child parent",
            &children,
            &ScopeOnly(ObjectInstallScope::Ingress(parent_stage)),
            &mut budget(),
        )?;
        catalog.discard_inputs(child_stage, &mut budget())?;
        catalog.discard_inputs(parent_stage, &mut budget())?;
        let work = work_for(&catalog, parent.reference)?.ok_or("parent not queued")?;
        let first = objects
            .reclaim_one(work, 32, &mut budget())?
            .ok_or("parent readopted")?;
        assert_eq!(first.released_children, 32);
        assert!(first.has_more);
        assert!(matches!(
            objects.get(parent.reference, &mut budget()),
            Err(IndexError::CorruptObject(_))
        ));
        drop(objects);
        drop(catalog);
        let (catalog, objects) = open_managed(root.path())?;
        let mut released = first.released_children;
        let mut last_permit = None;
        for expected in [32, 6] {
            let work =
                work_for(&catalog, parent.reference)?.ok_or("draining parent lost on restart")?;
            let ReclaimWork::Draining(permit) = &work else {
                return Err("expected persisted Draining phase".into());
            };
            last_permit = Some(permit.clone());
            let page = objects
                .reclaim_one(work, 32, &mut budget())?
                .ok_or("draining parent readopted")?;
            assert_eq!(page.released_children, expected);
            released = released
                .checked_add(page.released_children)
                .ok_or("fixture released-child count overflow")?;
            assert_eq!(page.has_more, expected == 32);
        }
        assert_eq!(released, 70);
        assert!(work_for(&catalog, parent.reference)?.is_none());
        drop(objects);
        drop(catalog);
        let (catalog, objects) = open_managed(root.path())?;
        let connection = rusqlite::Connection::open(catalog.path())?;
        let remaining: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_gc_object_v1 WHERE object=?1",
            rusqlite::params![parent.reference.0.0.to_vec()],
            |row| row.get(0),
        )?;
        assert_eq!(
            remaining, 0,
            "completed reclaim must not retain per-object tombstones"
        );
        drop(connection);
        let old = last_permit.ok_or("missing completed drain permit")?;
        assert_eq!(
            catalog.mark_unlinked(old.clone(), &mut budget()),
            Err(IndexError::Conflict)
        );
        assert_eq!(
            catalog.drain_unlinked(old, 32, &mut budget()),
            Err(IndexError::Conflict)
        );
        for child in children {
            assert!(work_for(&catalog, child)?.is_some());
            assert!(objects.get(child, &mut budget()).is_ok());
        }
        Ok(())
    }

    #[test]
    fn cancelling_retiring_by_new_ingress_custody_preserves_physical_object() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, objects) = initialize(root.path())?;
        let old = catalog.open_ingress(operation(120))?;
        let stored = objects.install(
            b"retiring readoption",
            &[],
            &ScopeOnly(ObjectInstallScope::Ingress(old)),
            &mut budget(),
        )?;
        catalog.discard_inputs(old, &mut budget())?;
        let Some(ReclaimWork::Candidate(candidate)) = work_for(&catalog, stored.reference)? else {
            return Err("missing retirement candidate".into());
        };
        assert!(
            catalog
                .retire_candidate(candidate, &mut budget())?
                .is_some()
        );
        let new = catalog.open_ingress(operation(121))?;
        let adopted = objects.install(
            b"retiring readoption",
            &[],
            &ScopeOnly(ObjectInstallScope::Ingress(new)),
            &mut budget(),
        )?;
        assert_eq!(adopted, stored);
        assert!(
            objects
                .reclaim_one(ReclaimWork::Candidate(candidate), 32, &mut budget())?
                .is_none()
        );
        assert_eq!(
            objects.get(stored.reference, &mut budget())?.payload,
            b"retiring readoption"
        );
        catalog.discard_inputs(new, &mut budget())?;
        delete_one(root.path(), &catalog, &objects, stored.reference)?;
        Ok(())
    }
    #[test]
    fn scope_hint_cannot_authorize_installation_with_a_forged_ingress_fence() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, objects) = initialize(root.path())?;
        let genuine = catalog.open_ingress(operation(130))?;
        let forged = IngressRef {
            fence: genuine
                .fence
                .checked_add(1)
                .ok_or("fixture fence overflow")?,
            ..genuine
        };
        let object = objects.identify(b"forged scope", &[], &mut budget())?;
        assert!(
            objects
                .install(
                    b"forged scope",
                    &[],
                    &ScopeOnly(ObjectInstallScope::Ingress(forged)),
                    &mut budget()
                )
                .is_err()
        );
        assert!(matches!(
            objects.get(object.reference, &mut budget()),
            Err(IndexError::CorruptObject(_))
        ));
        let connection = rusqlite::Connection::open(catalog.path())?;
        let provisional: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_install_v1 WHERE object=?1",
            rusqlite::params![object.reference.0.0.to_vec()],
            |row| row.get(0),
        )?;
        assert_eq!(provisional, 0);
        assert!(work_for(&catalog, object.reference)?.is_none());
        Ok(())
    }
    #[test]
    fn managed_batch_confirms_parent_with_earlier_provisional_child_atomically() -> TestResult {
        use quanta_index_core::index::ObjectBatchInput;
        let root = tempfile::tempdir()?;
        let (catalog, objects) = initialize(root.path())?;
        let stage = catalog.open_ingress(operation(140))?;
        let leaf = objects.identify(b"batch child", &[], &mut budget())?;
        let inputs = [
            ObjectBatchInput {
                payload: b"batch child".to_vec(),
                children: vec![],
            },
            ObjectBatchInput {
                payload: b"batch parent".to_vec(),
                children: vec![leaf.reference],
            },
        ];
        let installed = objects.install_batch(
            &inputs,
            &ScopeOnly(ObjectInstallScope::Ingress(stage)),
            &mut budget(),
        )?;
        assert_eq!(installed.len(), 2);
        let [first, second] = installed.as_slice() else {
            return Err("managed batch did not return exactly two objects".into());
        };
        assert_eq!(first, &leaf);
        assert_eq!(second.children, vec![leaf.reference]);
        catalog.discard_inputs(stage, &mut budget())?;
        delete_one(root.path(), &catalog, &objects, second.reference)?;
        delete_one(root.path(), &catalog, &objects, leaf.reference)?;
        Ok(())
    }
    #[test]
    fn exact_ticket_keeps_retired_source_alive_until_pin_release_then_gc_reclaims_it() -> TestResult
    {
        use quanta_index_core::index::ports::{
            CatalogCustodyMaintenancePort, CatalogExactHistoryPort,
        };
        let root = tempfile::tempdir()?;
        let (catalog, objects) = open_managed(root.path())?;
        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.path().join("kernel-scratch"),
            32 * 1024 * 1024,
        )?);
        let kernel = Kernel::new(
            KernelServices::new(
                objects.clone(),
                objects.clone(),
                catalog.clone(),
                scratch,
                Duration::from_secs(30),
            )?,
            AdapterRegistry::new(),
        );
        let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
        let ingress = kernel.begin_input(operation(150))?;
        let record =
            kernel.stage_source(ingress, "old.rs", b"old pinned content", &mut budget())?;
        let body =
            serde_json::from_slice::<SourceRecord>(&objects.get(record, &mut budget())?.payload)?
                .bytes;
        let record_file = object_file(root.path(), record)?;
        let body_file = object_file(root.path(), body)?;
        assert!(record_file.is_file());
        assert!(body_file.is_file());
        let mutations = kernel.stage_mutations(
            ingress,
            std::iter::once(Ok(OwnerMutation {
                owner: owner(150),
                expected: None,
                next: Some(record),
            })),
            &mut budget(),
        )?;
        let mut transition = SourceTransition {
            expected_source: source(&kernel, genesis)?,
            owners: mutations,
            events: vec![],
        };
        transition.events.push(SourceEvent {
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            predecessor: 0,
            sequence: 1,
            payload: transition.owner_digest()?,
        });
        let first = published(block_on(kernel.submit(
            UpdateRequest {
                operation: operation(150),
                expected_view: genesis,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &mut budget(),
        ))?)?;
        let pinned = kernel.pin_view(ViewSelection::Exact(first), &budget())?;
        let ingress = kernel.begin_input(operation(151))?;
        let mutations = kernel.stage_mutations(
            ingress,
            std::iter::once(Ok(OwnerMutation {
                owner: owner(150),
                expected: Some(record),
                next: None,
            })),
            &mut budget(),
        )?;
        let mut transition = SourceTransition {
            expected_source: pinned.view().source,
            owners: mutations,
            events: vec![],
        };
        transition.events.push(SourceEvent {
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            predecessor: 1,
            sequence: 2,
            payload: transition.owner_digest()?,
        });
        let latest = published(block_on(kernel.submit(
            UpdateRequest {
                operation: operation(151),
                expected_view: first,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &mut budget(),
        ))?)?;
        for key in [operation(150), operation(151)] {
            loop {
                if !catalog
                    .release_terminal_custody(key, 256, &mut budget())?
                    .has_more
                {
                    break;
                }
            }
        }
        let _floor =
            catalog.advance_exact_history_floor(workspace(), 1, latest.sequence, &mut budget())?;
        let retired = catalog.retire_exact_history_page(workspace(), 1, None, 16, &mut budget())?;
        assert!(retired.blocked >= 1);
        for reference in [record, body] {
            if let Some(work) = work_for(&catalog, reference)? {
                assert!(objects.reclaim_one(work, 32, &mut budget())?.is_none());
            }
        }
        pinned.validate()?;
        assert_eq!(
            objects.get(body, &mut budget())?.payload,
            b"old pinned content"
        );
        assert!(record_file.is_file());
        assert!(body_file.is_file());
        pinned.release()?;
        let retired = catalog.retire_exact_history_page(workspace(), 1, None, 16, &mut budget())?;
        assert!(retired.retired >= 1);
        // A fixed bound makes lost queue custody fail, rather than loop forever.
        let mut reclaimed = false;
        for _ in 0..128 {
            let page = catalog.reclaim_work_page(None, 8, &mut budget())?;
            if page.items.is_empty() {
                break;
            }
            for work in page.items {
                let _page = objects.reclaim_one(work, 32, &mut budget())?;
            }
            if matches!(
                objects.get(body, &mut budget()),
                Err(IndexError::CorruptObject(_))
            ) {
                reclaimed = true;
                break;
            }
        }
        assert!(
            reclaimed,
            "old body was never reclaimed after releasing its last pinned root"
        );
        assert_object_absent(&body_file)?;
        assert!(matches!(
            objects.get(record, &mut budget()),
            Err(IndexError::CorruptObject(_))
        ));
        assert_object_absent(&record_file)?;
        let current = kernel.pin_view(ViewSelection::Exact(latest), &budget())?;
        current.validate()?;
        assert!(
            objects
                .lookup_object(
                    current.view().source.owners,
                    &owner_map_key(owner(150)),
                    &mut budget()
                )?
                .is_none()
        );
        current.release()?;
        Ok(())
    }
    #[test]
    fn aborted_bootstrap_profile_can_be_reinstalled_after_completed_physical_gc() -> TestResult {
        use quanta_index_core::index::ports::CatalogGenesisPort;
        let root = tempfile::tempdir()?;
        let (catalog, objects) = open_managed(root.path())?;
        let profile = serde_json::to_vec(&IndexProfile { instances: vec![] })?;
        let stage = catalog.begin_bootstrap(workspace(), 1, objects.as_ref(), &mut budget())?;
        let stored = objects.install(
            &profile,
            &[],
            &ScopeOnly(ObjectInstallScope::Bootstrap(stage)),
            &mut budget(),
        )?;
        assert_eq!(stored.reference, stage.profile);
        catalog.abort_bootstrap(stage, &mut budget())?;
        let Some(ReclaimWork::Candidate(old)) = work_for(&catalog, stored.reference)? else {
            return Err("aborted profile not queued".into());
        };
        delete_one(root.path(), &catalog, &objects, stored.reference)?;
        let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
            root.path().join("kernel-scratch"),
            32 * 1024 * 1024,
        )?);
        let kernel = Kernel::new(
            KernelServices::new(
                objects.clone(),
                objects.clone(),
                catalog.clone(),
                scratch,
                Duration::from_secs(30),
            )?,
            AdapterRegistry::new(),
        );
        let recreated = kernel.create_workspace(workspace(), 1, &mut budget())?;
        let ticket = kernel.pin_view(ViewSelection::Exact(recreated), &budget())?;
        ticket.validate()?;
        assert_eq!(ticket.view().profile, stored.reference);
        assert_eq!(
            objects.get(stored.reference, &mut budget())?.payload,
            profile
        );
        let connection = rusqlite::Connection::open(catalog.path())?;
        let generation: i64 = connection.query_row(
            "SELECT generation FROM native_gc_object_v1 WHERE object=?1",
            rusqlite::params![stored.reference.0.0.to_vec()],
            |row| row.get(0),
        )?;
        assert!(u64::try_from(generation)? > old.generation);
        ticket.release()?;
        Ok(())
    }
}

fn fixed_position_text_term_cost_case(terms: usize) -> TestResult<UpdateCostObservation> {
    use quanta_index_contract::{LqExpr, LqLeaf, LqQuery, LqSpan};
    use quanta_index_core::index::{SourceTextAdmission, TicketRecord};
    use quanta_index_lexical::{
        native::{Bm25Policy, NativeTextIndex, TextReadContext, TextRecipe},
        planner::LexicalPlanner,
    };

    let directory = tempfile::tempdir()?;
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        directory.path().join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        directory.path().join("native-objects"),
        scratch.clone(),
    )?);
    let spy = Arc::new(UpdateCostSpy::new(Arc::clone(&objects)));
    let catalog = Arc::new(SqliteCatalog::open(
        directory.path(),
        Duration::from_millis(100),
    )?);
    let admission = Digest([31; 32]);
    let recipe = TextRecipe::new(admission);
    let text_spec = InstanceSpec {
        kind: IndexKind([28; 16]),
        policy: recipe.output_policy(),
        ..spec()
    };
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: text_spec.kind,
            schema: text_spec.schema,
            format: text_spec.format,
        },
        NativeTextIndex::new(recipe),
        |delta| Ok(delta.clone()),
    )?;
    let engine = Kernel::new(
        KernelServices::new(
            spy.clone(),
            spy.clone(),
            catalog.clone(),
            scratch,
            Duration::from_secs(30),
        )?,
        registry,
    );
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let profile = engine.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![text_spec],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let mut words = Vec::with_capacity(terms);
    for number in 0..terms {
        let a = u8::try_from(number.div_euclid(676).rem_euclid(26))?;
        let b = u8::try_from(number.div_euclid(26).rem_euclid(26))?;
        let c = u8::try_from(number.rem_euclid(26))?;
        words.push(format!(
            "w{}{}{}",
            char::from(b'a'.checked_add(a).ok_or("invalid term letter")?),
            char::from(b'a'.checked_add(b).ok_or("invalid term letter")?),
            char::from(b'a'.checked_add(c).ok_or("invalid term letter")?)
        ));
    }
    let replaced = terms.checked_div(2).ok_or("invalid replacement ordinal")?;
    let first_unchanged = words.first().ok_or("missing first term")?.clone();
    let last_unchanged = words.last().ok_or("missing last term")?.clone();
    let old_term = words.get(replaced).ok_or("missing changed term")?.clone();
    let old_body = words.join(" ");
    *words.get_mut(replaced).ok_or("missing changed term")? = "zzzz".into();
    let new_body = words.join(" ");
    assert_eq!(old_body.len(), new_body.len());
    let admitted = SourceTextAdmission::Utf8 { policy: admission };
    let old_source = engine.stage_source_text(
        engine.begin_input(operation(2))?,
        "hot.rs",
        old_body.as_bytes(),
        admitted,
        &mut budget(),
    )?;
    let initial = published(block_on(engine.submit(
        request(
            &engine,
            &catalog,
            &objects,
            configured,
            source(&engine, configured)?,
            2,
            1,
            vec![OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(old_source),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let old_ticket = engine.pin_view(ViewSelection::Exact(initial), &budget())?;
    let old_member = member(&engine, &objects, initial)?;
    let new_source = engine.stage_source_text(
        engine.begin_input(operation(3))?,
        "hot.rs",
        new_body.as_bytes(),
        admitted,
        &mut budget(),
    )?;
    let next = request(
        &engine,
        &catalog,
        &objects,
        initial,
        old_ticket.view().source,
        3,
        2,
        vec![OwnerMutation {
            owner: owner(1),
            expected: Some(old_source),
            next: Some(new_source),
        }],
    )?;
    spy.arm(
        old_ticket.view().source.owners,
        std::collections::BTreeSet::new(),
    )?;
    let updated = published(block_on(engine.submit(next, &mut budget()))?)?;
    let observed = spy.disarm()?;
    let current_ticket = engine.pin_view(ViewSelection::Exact(updated), &budget())?;
    let current_member = member(&engine, &objects, updated)?;
    let index = NativeTextIndex::new(recipe);
    let score = (4.0_f64 / 3.0).ln();
    let check = |ticket: &TicketRecord,
                 binding: &MemberBinding,
                 term: &str,
                 absent: &str,
                 expected_view: ViewRef,
                 expected_source: ObjectRef|
     -> TestResult {
        assert_eq!(
            index.inspect_term_df(
                binding,
                quanta_index_lexical::native::CaseMode::Sensitive,
                absent,
                TextReadContext {
                    ticket,
                    objects: objects.as_ref(),
                    maps: objects.as_ref(),
                    budget: &mut budget(),
                },
            )?,
            0
        );
        let mut query = LqQuery::empty(LqSpan::synthetic(0));
        query.expr = LqExpr::Leaf(LqLeaf::Keyword(term.into()));
        let plan = LexicalPlanner::plan(&query)?;
        let ranked = index.search_content(
            binding,
            &plan,
            &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            1,
            Bm25Policy { k1: 1.2, b: 0.75 },
            TextReadContext {
                ticket,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget(),
            },
        )?;
        assert_eq!(
            (ranked.view, ranked.artifact, ranked.matched),
            (expected_view, binding.artifact, 1)
        );
        let [hit] = ranked.hits.as_slice() else {
            return Err("fixed-position term query must return exactly one hit".into());
        };
        assert_eq!(
            (hit.owner, durable_text_source(hit.source)?),
            (owner(1), expected_source)
        );
        assert!(score_within_16_ulps(hit.score, score));
        Ok(())
    };
    check(
        old_ticket.record(),
        &old_member,
        &old_term,
        "zzzz",
        initial,
        old_source,
    )?;
    check(
        current_ticket.record(),
        &current_member,
        "zzzz",
        &old_term,
        updated,
        new_source,
    )?;
    for term in [&first_unchanged, &last_unchanged] {
        check(
            old_ticket.record(),
            &old_member,
            term,
            "notpresent",
            initial,
            old_source,
        )?;
        check(
            current_ticket.record(),
            &current_member,
            term,
            "notpresent",
            updated,
            new_source,
        )?;
    }
    old_ticket.release()?;
    current_ticket.release()?;
    Ok(observed)
}

#[test]
fn native_text_same_position_term_replace_installs_only_changed_occurrences() -> TestResult {
    let small = fixed_position_text_term_cost_case(32)?;
    let large = fixed_position_text_term_cost_case(256)?;
    // Two surfaces (sensitive/folded); old term removals do not create occurrences.
    assert!(
        small.occurrence_inputs <= 2,
        "small: {}",
        small.occurrence_inputs
    );
    assert!(
        large.occurrence_inputs <= 2,
        "large: {}",
        large.occurrence_inputs
    );
    // A changed-file inventory must be edited from its published prior root,
    // rather than rebuilt from an empty root for every unchanged term.
    assert!(
        small.empty_base_edits <= 16,
        "small: {}",
        small.empty_base_edits
    );
    assert!(
        large.empty_base_edits <= 16,
        "large: {}",
        large.empty_base_edits
    );
    Ok(())
}

#[path = "native_lifecycle/inline_mutations.rs"]
mod inline_mutations;
#[path = "native_lifecycle/text_entry_maps.rs"]
mod text_entry_maps;
