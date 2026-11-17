//! Real physical admission and Catalog confirmation for a dense Text window.

use super::*;
use std::{
    error::Error,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use quanta_index_core::index::{
    CollectionId, Digest, GroupActivation, IndexKind, IndexProfile, InputDomain, InstanceSpec,
    MemberBinding, ObjectBatchInput, OwnerKey, OwnerMutation, ProducerId, PublicationGroupId,
    RegisteredObject, RegisteredObjectBatch, SchemaId, SlotId, SourceCut, SourceEvent, SourceInput,
    SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId, StoredObject, StreamId,
    TenantId, UpdateKind, UpdateRequest, ViewRef, WorkspaceKey, WorktreeId,
    effort::ResourceNeed,
    ports::{ImmutableObjectPort, ObjectInstallMemoryShape, ObjectWriteSink},
};
use quanta_index_lexical::native::{CaseMode, NativeTextIndex, TextReadContext, TextRecipe};

#[path = "../../../quanta-index-kernel/tests/common/native_catalog_profile.rs"]
mod native_catalog_profile;

type TestResult<T = ()> = std::result::Result<T, Box<dyn Error>>;
const POLICY: Digest = Digest([31; 32]);
const MIB: u64 = 1024 * 1024;

fn phase<T, E: std::fmt::Display>(label: &str, result: Result<T, E>) -> TestResult<T> {
    result.map_err(|error| format!("{label}: {error}").into())
}
const POST_CONFIRM_FAULT: &str = "dense install post-confirm fixture fault";

#[derive(Clone, Copy, Debug)]
struct DenseSample {
    leaves: usize,
    directories: usize,
    commits: u64,
}

struct DenseInstallSpy {
    inner: Arc<FileObjectStore>,
    forward_bound: bool,
    samples: Mutex<Vec<DenseSample>>,
    fail_after_confirm: AtomicBool,
    fail_small_after_confirm_block: AtomicUsize,
    small_objects: AtomicUsize,
    shared_small_blocks: AtomicUsize,
    occurrence_blocks: AtomicUsize,
}

impl DenseInstallSpy {
    fn samples(&self) -> TestResult<Vec<DenseSample>> {
        Ok(self.samples.lock().map_err(|e| e.to_string())?.clone())
    }

    fn record_small_objects(&self, inputs: &[ObjectBatchInput]) {
        let _prior = self.occurrence_blocks.fetch_add(
            inputs
                .iter()
                .filter(|input| input.payload.starts_with(b"QITXOC01"))
                .count(),
            Ordering::Relaxed,
        );
        let shared = inputs
            .iter()
            .filter(|input| input.payload.starts_with(b"QITXSP01"))
            .count();
        let standalone = inputs
            .iter()
            .filter(|input| {
                serde_json::from_slice::<serde_json::Value>(&input.payload)
                    .ok()
                    .is_some_and(|value| {
                        value.get("root").is_some()
                            && value
                                .get("df")
                                .and_then(serde_json::Value::as_u64)
                                .is_some_and(|df| (2..=8).contains(&df))
                    })
            })
            .count();
        let _prior = self
            .small_objects
            .fetch_add(standalone + shared, Ordering::Relaxed);
        let _prior = self
            .shared_small_blocks
            .fetch_add(shared, Ordering::Relaxed);
    }
}

impl ImmutableObjectPort for DenseInstallSpy {
    fn install_batch_memory_bound(
        &self,
        shape: ObjectInstallMemoryShape,
    ) -> std::result::Result<Option<quanta_index_core::index::MemoryCreditLimits>, IndexError> {
        if self.forward_bound {
            self.inner.install_batch_memory_bound(shape)
        } else {
            Ok(None)
        }
    }

    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> std::result::Result<RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> std::result::Result<RegisteredObject, IndexError> {
        self.inner.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> std::result::Result<RegisteredObject, IndexError> {
        self.inner.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> std::result::Result<RegisteredObjectBatch, IndexError> {
        let leaves = inputs
            .iter()
            .filter(|input| input.payload.starts_with(b"QITXDP01"))
            .count();
        let directories = inputs
            .iter()
            .filter(|input| input.payload.starts_with(b"QITXDD01"))
            .count();
        if leaves + directories == 0 {
            let confirmed = self.inner.install_batch(inputs, sink, budget)?;
            self.record_small_objects(inputs);
            let shared = inputs
                .iter()
                .filter(|input| input.payload.starts_with(b"QITXSP01"))
                .count();
            if shared != 0
                && self
                    .fail_small_after_confirm_block
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                        (remaining != 0).then(|| remaining.saturating_sub(shared))
                    })
                    .is_ok_and(|remaining| remaining <= shared)
            {
                return Err(IndexError::Storage(POST_CONFIRM_FAULT.into()));
            }
            return Ok(confirmed);
        }
        let before = native_catalog_profile::snapshot()
            .map_err(|error| IndexError::Storage(error.into()))?
            .ok_or_else(|| IndexError::BackendViolation("Catalog profile absent".into()))?;
        let confirmed = self.inner.install_batch(inputs, sink, budget)?;
        self.record_small_objects(inputs);
        let sql = native_catalog_profile::since(Some(before))
            .map_err(|error| IndexError::Storage(error.into()))?
            .ok_or_else(|| IndexError::BackendViolation("Catalog profile absent".into()))?;
        self.samples
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?
            .push(DenseSample {
                leaves,
                directories,
                commits: sql.commit.calls,
            });
        if leaves > 0 && directories > 0 && self.fail_after_confirm.swap(false, Ordering::AcqRel) {
            return Err(IndexError::Storage(POST_CONFIRM_FAULT.into()));
        }
        Ok(confirmed)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> std::result::Result<StoredObject, IndexError> {
        self.inner.get(reference, budget)
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> std::result::Result<StoredObject, IndexError> {
        self.inner.read_retained(object, budget)
    }
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

fn spec() -> InstanceSpec {
    let recipe = TextRecipe::new(POLICY);
    InstanceSpec {
        instance: IndexInstanceId([7; 16]),
        kind: IndexKind([28; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: recipe.output_policy(),
        format: StorageFormatId([12; 16]),
    }
}

fn work(scope: &quanta_index_core::index::MemoryScope) -> WorkBudget {
    let mut budget = WorkBudget::with_request(
        WorkLimits {
            reads: 20_000_000,
            writes: 20_000_000,
            bytes: 2 * 1024 * MIB,
            mutations: 20_000_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(120)),
    );
    budget.bind_memory_scope(Some(scope.clone()));
    budget
}

struct Fixture {
    kernel: Kernel<SqliteCatalog>,
    objects: Arc<FileObjectStore>,
    _catalog: Arc<SqliteCatalog>,
    spy: Arc<DenseInstallSpy>,
    scope: quanta_index_core::index::MemoryScope,
    _guard: ReservationGuard<NativeClassScheduler>,
    _physical: Arc<PublicationFileCapacity>,
}

fn open_fixture(directory: &Path, forward_bound: bool) -> TestResult<Fixture> {
    open_fixture_with_memory(directory, forward_bound, 64 * MIB)
}

fn open_fixture_with_memory(
    directory: &Path,
    forward_bound: bool,
    request_memory_bytes: u64,
) -> TestResult<Fixture> {
    open_fixture_with_memory_and_batch(directory, forward_bound, request_memory_bytes, 64)
}

fn open_fixture_with_memory_and_batch(
    directory: &Path,
    forward_bound: bool,
    request_memory_bytes: u64,
    inventory_batch_limit: usize,
) -> TestResult<Fixture> {
    let need = ResourceNeed {
        cpu_units: 1,
        io_bytes: 64 * MIB,
        memory_bytes: request_memory_bytes,
        provider_units: 0,
    };
    let plan = NativeResourcePlan {
        capacity: ResourceNeed {
            cpu_units: 4,
            io_bytes: 256 * MIB,
            memory_bytes: 512 * MIB,
            provider_units: 0,
        },
        control: need,
        interactive: need,
        core: need,
        enrichment: need,
    };
    let ledger = NativeResourceLedger::new(plan)?;
    let guard = ReservationGuard::try_acquire(ledger.scheduler(NativeResourceClass::Core), need)
        .ok_or("Core reservation unavailable")?;
    let scope = guard
        .memory_scope()
        .ok_or("Original memory scope unavailable")?;
    let lease = StateRootLease::acquire(&directory.join("state"))?;
    let root = lease.state_root_identity_v1().to_path_buf();
    let inventory = PublicationBootInventoryLimits {
        max_bytes: 512 * MIB,
        max_files: 8192,
        max_pending: 1024,
        max_entries: 16384,
        max_depth: 8,
        max_pending_frames: 1024,
    };
    let capacity =
        seed_publication_capacity(&lease, &root, inventory, &[], &RequestBudgetV1::unbounded())?;
    let physical = PublicationFileCapacity::new(lease, capacity, 32, 16384, 256)?;
    let io = NativeScratchIo::register(
        Arc::clone(&physical),
        std::ffi::CString::new("quanta-publication-capacity")?,
        512,
    )?;
    let scratch = Arc::new(FileScratchSpace::open_managed_with_io(
        root.join("scratch"),
        16 * MIB,
        io.clone(),
    )?);
    let catalog = Arc::new(SqliteCatalog::open_with_io_admission(
        &root,
        Duration::from_secs(2),
        io.clone(),
    )?);
    native_catalog_profile::attach(&catalog)?;
    let object_root = root.join("object-store");
    let lock = object_root.join(".object-namespace.lease");
    io.prepare_lock(&lock)?;
    let read = Arc::new(NativeObjectReadIo::new(Arc::clone(&physical)));
    let scratch_port: Arc<dyn quanta_index_core::index::ScratchSpacePort> = scratch.clone();
    let authority: Arc<dyn quanta_index_core::index::ports::ManagedObjectAuthority> =
        catalog.clone();
    let objects = Arc::new(FileObjectStore::open_managed_with_io(
        object_root,
        scratch_port.clone(),
        authority,
        ManagedObjectIo {
            namespace_lock: read.open(&lock, &mut work(&scope))?,
            read,
            write: Arc::new(NativeObjectWriteIo::new(Arc::clone(&physical))),
            reclaim: Arc::new(NativeObjectReclaimIo::new(Arc::clone(&physical))),
        },
    )?);
    let spy = Arc::new(DenseInstallSpy {
        inner: Arc::clone(&objects),
        forward_bound,
        samples: Mutex::new(Vec::new()),
        fail_after_confirm: AtomicBool::new(false),
        fail_small_after_confirm_block: AtomicUsize::new(0),
        small_objects: AtomicUsize::new(0),
        shared_small_blocks: AtomicUsize::new(0),
        occurrence_blocks: AtomicUsize::new(0),
    });
    let text_spec = spec();
    let mut registry = AdapterRegistry::new();
    registry.register(
        AdapterDescriptor {
            kind: text_spec.kind,
            schema: text_spec.schema,
            format: text_spec.format,
        },
        NativeTextIndex::new(TextRecipe::new(POLICY))
            .with_inventory_reuse_limit(64)?
            .with_inventory_batch_limit(inventory_batch_limit)?,
        |delta| Ok(delta.clone()),
    )?;
    let object_port: Arc<dyn ImmutableObjectPort> = spy.clone();
    let map_port: Arc<dyn PersistentMapPort> = objects.clone();
    let kernel = Kernel::new(
        KernelServices::new(
            object_port,
            map_port,
            Arc::clone(&catalog),
            scratch_port,
            Duration::from_secs(30),
        )?,
        registry,
    );
    Ok(Fixture {
        kernel,
        objects,
        _catalog: catalog,
        spy,
        scope,
        _guard: guard,
        _physical: physical,
    })
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other => Err(format!("expected published view, got {other:?}").into()),
    }
}

fn member(fixture: &Fixture, view: ViewRef) -> TestResult<MemberBinding> {
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work(&fixture.scope))?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&spec().instance.0);
    let reference = fixture
        .objects
        .lookup_object(ticket.view().members, &key, &mut work(&fixture.scope))?
        .ok_or("Text member absent")?;
    let binding = serde_json::from_slice(
        &fixture
            .objects
            .get(reference, &mut work(&fixture.scope))?
            .payload,
    )?;
    ticket.release()?;
    Ok(binding)
}

fn check_membership(fixture: &Fixture, view: ViewRef) -> TestResult<ObjectRef> {
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work(&fixture.scope))?;
    let binding = member(fixture, view)?;
    let reader = NativeTextIndex::new(TextRecipe::new(POLICY));
    for (term, position) in [("alpha", 0_u32), ("beta", 1), ("gamma", 2)] {
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let mut hits = Vec::new();
            let scan = reader.visit_term(
                &binding,
                case,
                term,
                TextReadContext {
                    ticket: ticket.record(),
                    objects: &*fixture.objects,
                    maps: &*fixture.objects,
                    budget: &mut work(&fixture.scope),
                },
                |hit| {
                    hits.push((hit.owner, hit.tf, hit.positions.to_vec()));
                    Ok(())
                },
            )?;
            assert_eq!(scan.df, 9);
            assert_eq!(
                hits,
                (1..=9_u8)
                    .map(|id| (owner(id), 1_u64, vec![position]))
                    .collect::<Vec<_>>()
            );
        }
    }
    ticket.release()?;
    Ok(binding.artifact)
}

fn latest_view(fixture: &Fixture) -> TestResult<ViewRef> {
    let ticket = fixture.kernel.pin_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &work(&fixture.scope),
    )?;
    let view = ticket.view().identity;
    ticket.release()?;
    Ok(view)
}

fn check_empty_text(fixture: &Fixture, view: ViewRef) -> TestResult {
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work(&fixture.scope))?;
    assert!(ticket.view().source.owners.0.is_none());
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&spec().instance.0);
    if let Some(reference) =
        fixture
            .objects
            .lookup_object(ticket.view().members, &key, &mut work(&fixture.scope))?
    {
        let member: MemberBinding = serde_json::from_slice(
            &fixture
                .objects
                .get(reference, &mut work(&fixture.scope))?
                .payload,
        )?;
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let mut hits = Vec::new();
            let scan = NativeTextIndex::new(TextRecipe::new(POLICY)).visit_term(
                &member,
                case,
                "alpha",
                TextReadContext {
                    ticket: ticket.record(),
                    objects: &*fixture.objects,
                    maps: &*fixture.objects,
                    budget: &mut work(&fixture.scope),
                },
                |hit| {
                    hits.push(hit.owner);
                    Ok(())
                },
            )?;
            assert_eq!(scan.df, 0);
            assert!(hits.is_empty());
        }
    }
    ticket.release()?;
    Ok(())
}

fn configure_dense(fixture: &Fixture, runtime: &tokio::runtime::Runtime) -> TestResult<ViewRef> {
    let genesis = fixture
        .kernel
        .create_workspace(workspace(), 1, &mut work(&fixture.scope))?;
    let configure = operation(1);
    let profile = fixture.kernel.stage_profile(
        fixture.kernel.begin_input(configure)?,
        &IndexProfile {
            instances: vec![spec()],
        },
        &mut work(&fixture.scope),
    )?;
    published(runtime.block_on(fixture.kernel.submit(
        UpdateRequest {
            operation: configure,
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut work(&fixture.scope),
    ))?)
}

fn dense_request(
    fixture: &Fixture,
    configured: ViewRef,
    next: OperationKey,
) -> TestResult<UpdateRequest> {
    let inputs = (1..=9_u8)
        .map(|id| SourceInput {
            path: format!("dense-{id}.rs"),
            bytes: b"alpha beta gamma".to_vec(),
            text: SourceTextAdmission::Utf8 { policy: POLICY },
        })
        .collect();
    source_request(fixture, configured, next, inputs)
}

fn repeated_dense_request(
    fixture: &Fixture,
    configured: ViewRef,
    next: OperationKey,
) -> TestResult<UpdateRequest> {
    let body = (0_u32..65)
        .map(|slot| format!("term{slot:03}"))
        .collect::<Vec<_>>()
        .join(" ")
        .into_bytes();
    source_request(
        fixture,
        configured,
        next,
        (1..=9_u8)
            .map(|id| SourceInput {
                path: format!("dense-repeat-{id}.rs"),
                bytes: body.clone(),
                text: SourceTextAdmission::Utf8 { policy: POLICY },
            })
            .collect(),
    )
}

fn check_repeated_dense_membership(fixture: &Fixture, view: ViewRef) -> TestResult<ObjectRef> {
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work(&fixture.scope))?;
    let binding = member(fixture, view)?;
    let reader = NativeTextIndex::new(TextRecipe::new(POLICY));
    // Check every output slot, including the interior and end of both full
    // case-paired pages, against the fixed source token order.
    for position in 0_u32..65 {
        let term = format!("term{position:03}");
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let mut hits = Vec::new();
            let scan = reader.visit_term(
                &binding,
                case,
                &term,
                TextReadContext {
                    ticket: ticket.record(),
                    objects: &*fixture.objects,
                    maps: &*fixture.objects,
                    budget: &mut work(&fixture.scope),
                },
                |hit| {
                    hits.push((hit.owner, hit.tf, hit.positions.to_vec()));
                    Ok(())
                },
            )?;
            assert_eq!(scan.df, 9, "{term} {case:?}");
            assert_eq!(
                hits,
                (1..=9_u8)
                    .map(|id| (owner(id), 1_u64, vec![position]))
                    .collect::<Vec<_>>(),
                "{term} {case:?}"
            );
        }
    }
    ticket.release()?;
    Ok(binding.artifact)
}

fn source_request(
    fixture: &Fixture,
    configured: ViewRef,
    next: OperationKey,
    inputs: Vec<SourceInput>,
) -> TestResult<UpdateRequest> {
    let owner_count = u8::try_from(inputs.len())?;
    let references = fixture.kernel.stage_sources(
        fixture.kernel.begin_input(next)?,
        inputs,
        &mut work(&fixture.scope),
    )?;
    assert_eq!(references.len(), usize::from(owner_count));
    let source_ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(configured), &work(&fixture.scope))?;
    let expected_source: SourceCut = source_ticket.view().source;
    source_ticket.release()?;
    let mut transition = SourceTransition {
        expected_source,
        owners: fixture.kernel.stage_mutations(
            fixture.kernel.begin_input(next)?,
            (1..=owner_count).zip(references).map(|(id, source)| {
                Ok(OwnerMutation {
                    owner: owner(id),
                    expected: None,
                    next: Some(source),
                })
            }),
            &mut work(&fixture.scope),
        )?,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: next.producer,
        stream: next.stream,
        stream_epoch: next.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    Ok(UpdateRequest {
        operation: next,
        expected_view: configured,
        kind: UpdateKind::SourceAdvance(transition),
    })
}

fn build_dense_view(fixture: &Fixture, runtime: &tokio::runtime::Runtime) -> TestResult<ViewRef> {
    let configured = configure_dense(fixture, runtime)?;
    let request = dense_request(fixture, configured, operation(2))?;
    published(runtime.block_on(fixture.kernel.submit(request, &mut work(&fixture.scope)))?)
}

fn small_inputs() -> Vec<SourceInput> {
    [
        "alpha alpha beta gamma",
        "alpha beta beta gamma",
        "beta gamma",
        "gamma",
        "gamma",
        "gamma",
        "gamma",
        "gamma",
    ]
    .into_iter()
    .enumerate()
    .map(|(slot, text)| SourceInput {
        path: format!("small-{}.rs", slot + 1),
        bytes: text.as_bytes().to_vec(),
        text: SourceTextAdmission::Utf8 { policy: POLICY },
    })
    .collect()
}

fn small_page_boundary_inputs(term_count: u32) -> Vec<SourceInput> {
    let body = (0_u32..term_count)
        .map(|position| format!("term{position:03}"))
        .collect::<Vec<_>>()
        .join(" ")
        .into_bytes();
    (1..=2_u8)
        .map(|id| SourceInput {
            path: format!("small-page-boundary-{id}.rs"),
            bytes: body.clone(),
            text: SourceTextAdmission::Utf8 { policy: POLICY },
        })
        .collect()
}

fn check_small_page_boundary_membership(
    fixture: &Fixture,
    view: ViewRef,
    term_count: u32,
) -> TestResult<ObjectRef> {
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work(&fixture.scope))?;
    let binding = member(fixture, view)?;
    let reader = NativeTextIndex::new(TextRecipe::new(POLICY));
    for position in 0_u32..term_count {
        let term = format!("term{position:03}");
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let mut hits = Vec::new();
            let scan = reader.visit_term(
                &binding,
                case,
                &term,
                TextReadContext {
                    ticket: ticket.record(),
                    objects: &*fixture.objects,
                    maps: &*fixture.objects,
                    budget: &mut work(&fixture.scope),
                },
                |hit| {
                    hits.push((hit.owner, hit.tf, hit.positions.to_vec()));
                    Ok(())
                },
            )?;
            assert_eq!(scan.df, 2, "{term} {case:?}");
            assert_eq!(
                hits,
                (1..=2_u8)
                    .map(|id| (owner(id), 1_u64, vec![position]))
                    .collect::<Vec<_>>(),
                "{term} {case:?}"
            );
        }
    }
    ticket.release()?;
    Ok(binding.artifact)
}

fn check_empty_small_page_boundary_text(
    fixture: &Fixture,
    view: ViewRef,
    term_count: u32,
) -> TestResult {
    check_empty_text(fixture, view)?;
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work(&fixture.scope))?;
    let binding = member(fixture, view)?;
    let reader = NativeTextIndex::new(TextRecipe::new(POLICY));
    for position in 0_u32..term_count {
        let term = format!("term{position:03}");
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let mut hits = Vec::new();
            let scan = reader.visit_term(
                &binding,
                case,
                &term,
                TextReadContext {
                    ticket: ticket.record(),
                    objects: &*fixture.objects,
                    maps: &*fixture.objects,
                    budget: &mut work(&fixture.scope),
                },
                |hit| {
                    hits.push(hit.owner);
                    Ok(())
                },
            )?;
            assert_eq!(scan.df, 0, "{term} {case:?}");
            assert!(hits.is_empty(), "{term} {case:?}");
        }
    }
    ticket.release()?;
    Ok(())
}

fn check_small_membership(fixture: &Fixture, view: ViewRef) -> TestResult<ObjectRef> {
    check_small_membership_with_owner8(fixture, view, true)
}

fn check_small_membership_with_owner8(
    fixture: &Fixture,
    view: ViewRef,
    owner8_present: bool,
) -> TestResult<ObjectRef> {
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work(&fixture.scope))?;
    let binding = member(fixture, view)?;
    let reader = NativeTextIndex::new(TextRecipe::new(POLICY));
    let gold = [
        (
            "alpha",
            2_u64,
            vec![(owner(1), 2_u64, vec![0_u32, 1]), (owner(2), 1, vec![0])],
        ),
        (
            "beta",
            3,
            vec![
                (owner(1), 1, vec![2]),
                (owner(2), 2, vec![1, 2]),
                (owner(3), 1, vec![0]),
            ],
        ),
        (
            "gamma",
            8,
            vec![
                (owner(1), 1, vec![3]),
                (owner(2), 1, vec![3]),
                (owner(3), 1, vec![1]),
                (owner(4), 1, vec![0]),
                (owner(5), 1, vec![0]),
                (owner(6), 1, vec![0]),
                (owner(7), 1, vec![0]),
                (owner(8), 1, vec![0]),
            ],
        ),
    ];
    for (term, mut df, mut expected) in gold {
        if term == "gamma" && !owner8_present {
            assert_eq!(expected.pop(), Some((owner(8), 1, vec![0])));
            df = 7;
        }
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let mut hits = Vec::new();
            let scan = reader.visit_term(
                &binding,
                case,
                term,
                TextReadContext {
                    ticket: ticket.record(),
                    objects: &*fixture.objects,
                    maps: &*fixture.objects,
                    budget: &mut work(&fixture.scope),
                },
                |hit| {
                    hits.push((hit.owner, hit.tf, hit.positions.to_vec()));
                    Ok(())
                },
            )?;
            assert_eq!(scan.df, df);
            assert_eq!(hits, expected);
        }
    }
    ticket.release()?;
    Ok(binding.artifact)
}

fn small_block(fixture: &Fixture, view: ViewRef, term: &[u8]) -> TestResult<ObjectRef> {
    let binding = member(fixture, view)?;
    let artifact = fixture
        .objects
        .get(binding.artifact, &mut work(&fixture.scope))?;
    let root: serde_json::Value = serde_json::from_slice(&artifact.payload)?;
    let terms = serde_json::from_value(root.get("terms").ok_or("terms absent")?.clone())?;
    let value = fixture
        .objects
        .lookup(terms, term, &mut work(&fixture.scope))?
        .ok_or("posting absent")?;
    let quanta_index_core::index::MapValue::Inline(value) = value else {
        return Err("small posting is not inline".into());
    };
    assert_eq!(value.data()?.first(), Some(&3));
    Ok(value.single_child()?.ok_or("shared block absent")?)
}

#[test]
fn small_text_delete_preserves_untouched_shared_slots_old_view_and_reopen() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let directory = tempfile::tempdir()?;
    let fixture = open_fixture(directory.path(), true)?;
    let configured = configure_dense(&fixture, &runtime)?;
    let request = source_request(&fixture, configured, operation(2), small_inputs())?;
    let initial =
        published(runtime.block_on(fixture.kernel.submit(request, &mut work(&fixture.scope)))?)?;
    let initial_artifact = check_small_membership(&fixture, initial)?;
    let old_pin = fixture
        .kernel
        .pin_view(ViewSelection::Exact(initial), &work(&fixture.scope))?;
    let old_block = small_block(&fixture, initial, b"\0alpha")?;
    assert_eq!(small_block(&fixture, initial, b"\0beta")?, old_block);
    assert_eq!(small_block(&fixture, initial, b"\0gamma")?, old_block);
    let source = old_pin.view().source;
    let prior = fixture
        .objects
        .lookup_object(
            source.owners,
            &quanta_index_core::index::owner_map_key(owner(8)),
            &mut work(&fixture.scope),
        )?
        .ok_or("owner8 source absent")?;
    let remove = operation(3);
    let mut transition = SourceTransition {
        expected_source: source,
        owners: fixture.kernel.stage_mutations(
            fixture.kernel.begin_input(remove)?,
            [Ok(OwnerMutation {
                owner: owner(8),
                expected: Some(prior),
                next: None,
            })],
            &mut work(&fixture.scope),
        )?,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: remove.producer,
        stream: remove.stream,
        stream_epoch: remove.stream_epoch,
        predecessor: 1,
        sequence: 2,
        payload: transition.owner_digest()?,
    });
    let latest = published(runtime.block_on(fixture.kernel.submit(
        UpdateRequest {
            operation: remove,
            expected_view: initial,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut work(&fixture.scope),
    ))?)?;
    let artifact = check_small_membership_with_owner8(&fixture, latest, false)?;
    assert_eq!(check_small_membership(&fixture, initial)?, initial_artifact);
    for term in [b"\0alpha".as_slice(), b"\x01alpha", b"\0beta", b"\x01beta"] {
        assert_eq!(small_block(&fixture, latest, term)?, old_block);
    }
    assert_ne!(small_block(&fixture, latest, b"\0gamma")?, old_block);
    old_pin.release()?;
    drop(fixture);
    let reopened = open_fixture(directory.path(), true)?;
    assert_eq!(latest_view(&reopened)?, latest);
    assert_eq!(
        check_small_membership_with_owner8(&reopened, latest, false)?,
        artifact
    );
    assert_eq!(small_block(&reopened, latest, b"\0alpha")?, old_block);
    assert_eq!(
        check_small_membership(&reopened, initial)?,
        initial_artifact
    );
    Ok(())
}

#[test]
fn small_text_post_confirm_error_preserves_current_and_resumes_original_operation() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let directory = tempfile::tempdir()?;
    let fixture = open_fixture(directory.path(), true)?;
    let configured = configure_dense(&fixture, &runtime)?;
    let failed = operation(2);
    let request = source_request(&fixture, configured, failed, small_inputs())?;
    fixture
        .spy
        .fail_small_after_confirm_block
        .store(1, Ordering::Release);
    let outcome = runtime.block_on(fixture.kernel.submit(request, &mut work(&fixture.scope)))?;
    assert_eq!(outcome, SubmitOutcome::Unknown(failed));
    assert_eq!(
        fixture
            .spy
            .fail_small_after_confirm_block
            .load(Ordering::Acquire),
        0
    );
    assert_eq!(fixture.spy.shared_small_blocks.load(Ordering::Relaxed), 1);
    assert_eq!(latest_view(&fixture)?, configured);
    check_empty_text(&fixture, configured)?;
    drop(fixture);
    let reopened = open_fixture(directory.path(), true)?;
    assert_eq!(latest_view(&reopened)?, configured);
    check_empty_text(&reopened, configured)?;
    let view =
        published(runtime.block_on(reopened.kernel.resume(failed, &mut work(&reopened.scope)))?)?;
    assert_eq!(latest_view(&reopened)?, view);
    let artifact = check_small_membership(&reopened, view)?;
    drop(reopened);
    let after_resume = open_fixture(directory.path(), true)?;
    assert_eq!(latest_view(&after_resume)?, view);
    assert_eq!(check_small_membership(&after_resume, view)?, artifact);
    Ok(())
}

#[test]
fn small_text_second_block_post_confirm_error_resumes_original_operation() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut expected_artifact = None;
    for memory_bytes in [64 * MIB, 8 * MIB] {
        let directory = tempfile::tempdir()?;
        let fixture = open_fixture_with_memory(directory.path(), true, memory_bytes)?;
        let configured = configure_dense(&fixture, &runtime)?;
        let failed = operation(2);
        let request = source_request(
            &fixture,
            configured,
            failed,
            small_page_boundary_inputs(129),
        )?;
        fixture
            .spy
            .fail_small_after_confirm_block
            .store(2, Ordering::Release);
        let outcome =
            runtime.block_on(fixture.kernel.submit(request, &mut work(&fixture.scope)))?;
        assert_eq!(outcome, SubmitOutcome::Unknown(failed));
        assert_eq!(
            fixture
                .spy
                .fail_small_after_confirm_block
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(fixture.spy.shared_small_blocks.load(Ordering::Relaxed), 2);
        assert_eq!(latest_view(&fixture)?, configured);
        check_empty_small_page_boundary_text(&fixture, configured, 129)?;
        drop(fixture);

        let reopened = open_fixture_with_memory(directory.path(), true, memory_bytes)?;
        assert_eq!(latest_view(&reopened)?, configured);
        check_empty_small_page_boundary_text(&reopened, configured, 129)?;
        let view = published(
            runtime.block_on(reopened.kernel.resume(failed, &mut work(&reopened.scope)))?,
        )?;
        assert_eq!(latest_view(&reopened)?, view);
        let artifact = check_small_page_boundary_membership(&reopened, view, 129)?;
        if let Some(expected) = expected_artifact {
            assert_eq!(artifact, expected);
        }
        expected_artifact = Some(artifact);
        drop(reopened);

        let after_resume = open_fixture_with_memory(directory.path(), true, memory_bytes)?;
        assert_eq!(latest_view(&after_resume)?, view);
        assert_eq!(
            check_small_page_boundary_membership(&after_resume, view, 129)?,
            artifact
        );
    }
    Ok(())
}

#[test]
fn small_text_real_original_shared_block_preserves_df_tf_positions_and_reopens() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut expected_artifact = None;
    for memory_bytes in [64 * MIB, 8 * MIB] {
        let root = tempfile::tempdir()?;
        let fixture = open_fixture_with_memory(root.path(), true, memory_bytes)?;
        let configured = configure_dense(&fixture, &runtime)?;
        let request = source_request(&fixture, configured, operation(2), small_inputs())?;
        let view = published(
            runtime.block_on(fixture.kernel.submit(request, &mut work(&fixture.scope)))?,
        )?;
        let artifact = check_small_membership(&fixture, view)?;
        if let Some(expected) = expected_artifact {
            assert_eq!(artifact, expected);
        }
        expected_artifact = Some(artifact);
        let objects = fixture.spy.small_objects.load(Ordering::Relaxed);
        let blocks = fixture.spy.shared_small_blocks.load(Ordering::Relaxed);
        drop(fixture);
        let reopened = open_fixture_with_memory(root.path(), true, memory_bytes)?;
        assert_eq!(check_small_membership(&reopened, view)?, artifact);
        drop(reopened);
        assert_eq!(
            objects, 1,
            "DF2/3/8 must share one bounded physical posting object"
        );
        assert_eq!(
            blocks, 1,
            "the fixed shared-block wire must be used in both Original scopes"
        );
    }
    Ok(())
}

#[test]
fn small_text_packs_across_posting_pages_with_real_original_credit() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    for (term_count, expected_blocks) in [(129_u32, 2_usize), (300, 3)] {
        let mut expected_artifact = None;
        for memory_bytes in [64 * MIB, 8 * MIB] {
            let directory = tempfile::tempdir()?;
            let fixture = open_fixture_with_memory(directory.path(), true, memory_bytes)?;
            let configured = configure_dense(&fixture, &runtime)?;
            let request = source_request(
                &fixture,
                configured,
                operation(2),
                small_page_boundary_inputs(term_count),
            )?;
            let mut budget = work(&fixture.scope);
            let view = published(runtime.block_on(fixture.kernel.submit(request, &mut budget))?)?;
            assert_eq!(budget.first_denial(), None);
            let artifact = check_small_page_boundary_membership(&fixture, view, term_count)?;
            if let Some(expected) = expected_artifact {
                assert_eq!(
                    artifact, expected,
                    "{term_count} terms, {memory_bytes} bytes"
                );
            }
            expected_artifact = Some(artifact);
            assert_eq!(
                fixture.spy.shared_small_blocks.load(Ordering::Relaxed),
                expected_blocks,
                "{term_count} terms, {memory_bytes} bytes"
            );
            drop(fixture);
            let reopened = open_fixture_with_memory(directory.path(), true, memory_bytes)?;
            assert_eq!(latest_view(&reopened)?, view);
            assert_eq!(
                check_small_page_boundary_membership(&reopened, view, term_count)?,
                artifact
            );
        }
    }
    Ok(())
}

const PREVIOUS_SMALL_POLICY: Digest = Digest([
    136, 124, 165, 228, 60, 19, 159, 52, 121, 219, 144, 171, 182, 45, 138, 119, 129, 73, 194, 24,
    244, 60, 76, 177, 237, 183, 112, 209, 38, 113, 45, 158,
]);

const PREVIOUS_PAGE_SMALL_POLICY: Digest = Digest([
    0xa0, 0xca, 0xd6, 0xdf, 0xaf, 0xdb, 0x11, 0x94, 0xa5, 0xb2, 0x7a, 0x8c, 0x27, 0xe0, 0xf7, 0x47,
    0xc2, 0x96, 0x33, 0x28, 0xa3, 0x3b, 0x3d, 0xb5, 0x57, 0x40, 0x76, 0x73, 0x90, 0x30, 0x2a, 0x63,
]);

#[test]
fn directory_text_real_original_shared_sources_preserve_positions_and_reopen() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let body = (0..257_u16)
        .map(|slot| format!("term{slot:03}"))
        .collect::<Vec<_>>()
        .join(" ");
    // Different raw bodies bypass whole-inventory body reuse, while equal
    // term positions reuse the confirmed occurrence blocks in the next window.
    let second_body = body.replace(' ', "\n");
    let mut expected_artifact = None;
    for memory_bytes in [64 * MIB, 8 * MIB] {
        let directory = tempfile::tempdir()?;
        let fixture = open_fixture_with_memory_and_batch(directory.path(), true, memory_bytes, 1)?;
        let configured = configure_dense(&fixture, &runtime)?;
        let inputs = [("directory-a.rs", &body), ("directory-b.rs", &second_body)]
            .into_iter()
            .map(|(path, text)| SourceInput {
                path: path.into(),
                bytes: text.as_bytes().to_vec(),
                text: SourceTextAdmission::Utf8 { policy: POLICY },
            })
            .collect();
        let request = source_request(&fixture, configured, operation(2), inputs)?;
        let mut budget = work(&fixture.scope);
        let view = published(runtime.block_on(fixture.kernel.submit(request, &mut budget))?)?;
        assert_eq!(budget.first_denial(), None);
        assert_eq!(
            fixture.spy.occurrence_blocks.load(Ordering::Relaxed),
            2,
            "two inventory windows must install the same two occurrence blocks only once"
        );
        let artifact = member(&fixture, view)?.artifact;
        if let Some(expected) = expected_artifact {
            assert_eq!(artifact, expected);
        }
        expected_artifact = Some(artifact);
        check_directory_positions(&fixture, view)?;
        drop(fixture);
        let reopened = open_fixture_with_memory(directory.path(), true, memory_bytes)?;
        assert_eq!(latest_view(&reopened)?, view);
        assert_eq!(member(&reopened, view)?.artifact, artifact);
        check_directory_positions(&reopened, view)?;
    }
    Ok(())
}

fn check_directory_positions(fixture: &Fixture, view: ViewRef) -> TestResult {
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work(&fixture.scope))?;
    let binding = member(fixture, view)?;
    for (term, position) in [("term000", 0_u32), ("term128", 128), ("term256", 256)] {
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let mut hits = Vec::new();
            let scan = NativeTextIndex::new(TextRecipe::new(POLICY)).visit_term(
                &binding,
                case,
                term,
                TextReadContext {
                    ticket: ticket.record(),
                    objects: &*fixture.objects,
                    maps: &*fixture.objects,
                    budget: &mut work(&fixture.scope),
                },
                |hit| {
                    hits.push((hit.owner, hit.tf, hit.positions.to_vec()));
                    Ok(())
                },
            )?;
            assert_eq!(scan.df, 2);
            assert_eq!(
                hits,
                vec![(owner(1), 1, vec![position]), (owner(2), 1, vec![position])]
            );
        }
    }
    ticket.release()?;
    Ok(())
}

#[test]
fn small_text_layout_policy_matches_independent_digest() {
    // Fixed SHA-256 contract vector: normalizer 2.0, the existing three layout
    // stamps, then QITXSP01 fixed bounds, per-case streams and fitting tail merge.
    assert_eq!(
        TextRecipe::new(POLICY).output_policy(),
        Digest([
            0x0b, 0x22, 0xe8, 0xda, 0xc2, 0x8b, 0xbf, 0x92, 0x4e, 0x64, 0x75, 0x40, 0x8c, 0x8d,
            0xa9, 0x73, 0xca, 0xf3, 0x2d, 0xdc, 0xb3, 0x2b, 0xd0, 0xdf, 0x85, 0xe6, 0xa8, 0x11,
            0xb9, 0xc0, 0xaf, 0xc5,
        ])
    );
}

#[test]
fn small_text_previous_layout_policy_is_refused_by_producer() {
    for policy in [PREVIOUS_SMALL_POLICY, PREVIOUS_PAGE_SMALL_POLICY] {
        let mut previous = spec();
        previous.policy = policy;
        assert!(matches!(
            quanta_index_core::index::ports::IndexBuildPort::validate_instance(
                &NativeTextIndex::new(TextRecipe::new(POLICY)),
                &previous,
            ),
            Err(IndexError::InvalidInput(_))
        ));
    }
}

#[test]
fn small_text_previous_layout_policy_is_refused_by_reader() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let root = tempfile::tempdir()?;
    let fixture = open_fixture(root.path(), true)?;
    let view = configure_dense(&fixture, &runtime)?;
    let ticket = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &work(&fixture.scope))?;
    for policy in [PREVIOUS_SMALL_POLICY, PREVIOUS_PAGE_SMALL_POLICY] {
        let mut previous = member(&fixture, view)?;
        previous.input.policy = policy;
        assert!(matches!(
            NativeTextIndex::new(TextRecipe::new(POLICY)).inspect_term_df(
                &previous,
                CaseMode::Sensitive,
                "alpha",
                TextReadContext {
                    ticket: ticket.record(),
                    objects: &*fixture.objects,
                    maps: &*fixture.objects,
                    budget: &mut work(&fixture.scope)
                },
            ),
            Err(IndexError::Conflict)
        ));
    }
    ticket.release()?;
    Ok(())
}

#[test]
fn dense_text_real_original_credit_coalesces_and_reopens() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let fast_dir = tempfile::tempdir()?;
    let fast = open_fixture(fast_dir.path(), true)?;
    let fast_view = build_dense_view(&fast, &runtime)?;
    let fast_artifact = check_membership(&fast, fast_view)?;
    let fast_samples = fast.spy.samples()?;
    assert_eq!(fast_samples.len(), 1, "fast dense calls: {fast_samples:?}");
    assert!(fast_samples[0].leaves >= 2, "{fast_samples:?}");
    assert!(fast_samples[0].directories >= 2, "{fast_samples:?}");
    assert_eq!(fast_samples[0].commits, 2, "{fast_samples:?}");
    drop(fast);
    let reopened = open_fixture(fast_dir.path(), true)?;
    assert_eq!(check_membership(&reopened, fast_view)?, fast_artifact);
    drop(reopened);

    let fallback_dir = tempfile::tempdir()?;
    let fallback = open_fixture(fallback_dir.path(), false)?;
    let fallback_view = build_dense_view(&fallback, &runtime)?;
    let fallback_artifact = check_membership(&fallback, fallback_view)?;
    let fallback_samples = fallback.spy.samples()?;
    assert_eq!(
        fallback_samples.len(),
        2,
        "fallback dense calls: {fallback_samples:?}"
    );
    assert_eq!(
        fallback_samples
            .iter()
            .map(|sample| sample.commits)
            .sum::<u64>(),
        4,
        "{fallback_samples:?}"
    );
    assert_eq!(fast_artifact, fallback_artifact);
    Ok(())
}

#[test]
fn dense_text_confirmed_root_reuse_crosses_posting_pages_with_real_original_credit() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let fast_dir = tempfile::tempdir()?;
    let fast = open_fixture_with_memory(fast_dir.path(), true, 64 * MIB)?;
    let configured = configure_dense(&fast, &runtime)?;
    let request = repeated_dense_request(&fast, configured, operation(2))?;
    let mut fast_budget = work(&fast.scope);
    let fast_view = published(runtime.block_on(fast.kernel.submit(request, &mut fast_budget))?)?;
    assert_eq!(fast_budget.first_denial(), None);
    let fast_artifact = check_repeated_dense_membership(&fast, fast_view)?;
    let fast_samples = fast.spy.samples()?;
    assert_eq!(
        fast_samples.len(),
        1,
        "64 MiB dense calls: {fast_samples:?}"
    );
    assert!(fast_samples[0].leaves > 0 && fast_samples[0].directories > 0);
    assert_eq!(fast_samples[0].commits, 2, "{fast_samples:?}");
    drop(fast);
    let fast_reopened = open_fixture_with_memory(fast_dir.path(), true, 64 * MIB)?;
    assert_eq!(
        check_repeated_dense_membership(&fast_reopened, fast_view)?,
        fast_artifact
    );
    drop(fast_reopened);

    let low_dir = tempfile::tempdir()?;
    let low = open_fixture_with_memory(low_dir.path(), true, 8 * MIB)?;
    let configured = configure_dense(&low, &runtime)?;
    let request = repeated_dense_request(&low, configured, operation(2))?;
    let mut low_budget = work(&low.scope);
    let low_view = published(runtime.block_on(low.kernel.submit(request, &mut low_budget))?)?;
    assert_eq!(low_budget.first_denial(), None);
    let low_artifact = check_repeated_dense_membership(&low, low_view)?;
    let low_samples = low.spy.samples()?;
    assert_eq!(low_samples.len(), 2, "8 MiB dense calls: {low_samples:?}");
    assert!(low_samples.iter().any(|sample| sample.leaves > 0));
    assert!(low_samples.iter().any(|sample| sample.directories > 0));
    assert_eq!(
        low_samples.iter().map(|sample| sample.commits).sum::<u64>(),
        4,
        "{low_samples:?}"
    );
    assert_eq!(low_artifact, fast_artifact);
    drop(low);
    let low_reopened = open_fixture_with_memory(low_dir.path(), true, 8 * MIB)?;
    assert_eq!(
        check_repeated_dense_membership(&low_reopened, low_view)?,
        fast_artifact
    );
    Ok(())
}

#[test]
fn dense_text_post_confirm_error_preserves_current_and_resumes_original_operation() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let directory = tempfile::tempdir()?;
    let fixture = phase("fault initial open", open_fixture(directory.path(), true))?;
    let configured = phase("fault configure", configure_dense(&fixture, &runtime))?;
    phase(
        "fault initial empty view",
        check_empty_text(&fixture, configured),
    )?;
    let failed_key = operation(2);
    let request = phase(
        "fault initial request",
        dense_request(&fixture, configured, failed_key),
    )?;
    fixture
        .spy
        .fail_after_confirm
        .store(true, Ordering::Release);
    let outcome = phase(
        "fault first submit",
        runtime.block_on(fixture.kernel.submit(request, &mut work(&fixture.scope))),
    )?;
    assert_eq!(outcome, SubmitOutcome::Unknown(failed_key));
    assert!(!fixture.spy.fail_after_confirm.load(Ordering::Acquire));
    let samples = fixture.spy.samples()?;
    assert_eq!(samples.len(), 1, "post-confirm dense calls: {samples:?}");
    assert!(samples[0].leaves >= 2, "{samples:?}");
    assert!(samples[0].directories >= 2, "{samples:?}");
    assert_eq!(samples[0].commits, 2, "{samples:?}");
    assert_eq!(latest_view(&fixture)?, configured);
    check_empty_text(&fixture, configured)?;
    drop(fixture);

    let reopened = phase("fault reopen", open_fixture(directory.path(), true))?;
    assert_eq!(latest_view(&reopened)?, configured);
    check_empty_text(&reopened, configured)?;
    // Unknown keeps the event claim. A fresh fence must replay this original
    // immutable request; submitting the same event under a new key conflicts.
    let outcome = phase(
        "fault resume original",
        runtime.block_on(
            reopened
                .kernel
                .resume(failed_key, &mut work(&reopened.scope)),
        ),
    )?;
    let published = phase("fault resumed published", published(outcome))?;
    assert_eq!(latest_view(&reopened)?, published);
    let artifact = check_membership(&reopened, published)?;
    drop(reopened);
    let after_retry = open_fixture(directory.path(), true)?;
    assert_eq!(latest_view(&after_retry)?, published);
    assert_eq!(check_membership(&after_retry, published)?, artifact);
    Ok(())
}

#[test]
fn dense_text_real_original_credit_miss_falls_back_without_denial() -> TestResult {
    let _profile = native_catalog_profile::Guard::start()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let fast_dir = tempfile::tempdir()?;
    let fast = open_fixture(fast_dir.path(), true)?;
    let fast_view = build_dense_view(&fast, &runtime)?;
    let fast_artifact = check_membership(&fast, fast_view)?;
    let fast_samples = fast.spy.samples()?;
    assert_eq!(
        fast_samples.len(),
        1,
        "64 MiB dense calls: {fast_samples:?}"
    );
    assert_eq!(fast_samples[0].commits, 2, "{fast_samples:?}");
    drop(fast);

    let low_dir = tempfile::tempdir()?;
    let low = open_fixture_with_memory(low_dir.path(), true, 8 * MIB)?;
    let configured = configure_dense(&low, &runtime)?;
    let request = dense_request(&low, configured, operation(2))?;
    let mut submit_budget = work(&low.scope);
    let low_view = published(runtime.block_on(low.kernel.submit(request, &mut submit_budget))?)?;
    assert_eq!(submit_budget.first_denial(), None);
    let low_samples = low.spy.samples()?;
    assert_eq!(low_samples.len(), 2, "8 MiB dense calls: {low_samples:?}");
    assert!(
        low_samples
            .iter()
            .map(|sample| sample.leaves)
            .sum::<usize>()
            >= 2
    );
    assert!(
        low_samples
            .iter()
            .map(|sample| sample.directories)
            .sum::<usize>()
            >= 2
    );
    assert_eq!(
        low_samples.iter().map(|sample| sample.commits).sum::<u64>(),
        4,
        "{low_samples:?}"
    );
    assert_eq!(check_membership(&low, low_view)?, fast_artifact);
    drop(low);
    let reopened = open_fixture_with_memory(low_dir.path(), true, 8 * MIB)?;
    assert_eq!(check_membership(&reopened, low_view)?, fast_artifact);
    Ok(())
}
