//! Real immutable-store and Kernel witnesses for deferred searchable custody.

use super::*;
use quanta_index_core::index::{
    AttemptContext, AttemptId, AttemptRef, MapEdit, MemoryLease, MemoryLeaseOwner, MemoryLifetime,
    MemoryScope, MemoryScopePort, ObjectBatchInput, RegisteredObject, RegisteredObjectBatch,
    SearchablePreparation, StagedPayload, StoredObject,
    ports::{IndexFuture, IndexOwnerBuildPort, ObjectCustodyPort, ObjectWriteSink},
};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

const LAYOUT: &[u8] = b"prepared-layout";

struct Objects {
    inner: Arc<FileObjectStore>,
    fault: AtomicUsize,
    layout_batches: AtomicUsize,
    identifies: AtomicUsize,
    scalar_installs: AtomicUsize,
    all_batches: AtomicUsize,
}

impl Objects {
    fn new(inner: Arc<FileObjectStore>, fault: usize) -> Self {
        Self {
            inner,
            fault: AtomicUsize::new(fault),
            layout_batches: AtomicUsize::new(0),
            identifies: AtomicUsize::new(0),
            scalar_installs: AtomicUsize::new(0),
            all_batches: AtomicUsize::new(0),
        }
    }
}

impl ImmutableObjectPort for Objects {
    fn identify(
        &self,
        p: &[u8],
        c: &[ObjectRef],
        b: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let _previous = self.identifies.fetch_add(1, Ordering::SeqCst);
        self.inner.identify(p, c, b)
    }
    fn put(
        &self,
        p: &[u8],
        c: &[ObjectRef],
        b: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.put(p, c, b)
    }
    fn install(
        &self,
        p: &[u8],
        c: &[ObjectRef],
        s: &dyn ObjectWriteSink,
        b: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let _previous = self.scalar_installs.fetch_add(1, Ordering::SeqCst);
        self.inner.install(p, c, s, b)
    }
    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        s: &dyn ObjectWriteSink,
        b: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        let _previous = self.all_batches.fetch_add(1, Ordering::SeqCst);
        let layout = inputs.first().is_some_and(|input| input.payload == LAYOUT);
        if layout {
            let _previous = self.layout_batches.fetch_add(1, Ordering::SeqCst);
        }
        let result = self.inner.install_batch(inputs, s, b)?;
        if !layout || self.fault.load(Ordering::SeqCst) == 0 {
            return Ok(result);
        }
        let mut rows: Vec<_> = result.into_iter().collect();
        match self.fault.load(Ordering::SeqCst) {
            1 => {
                let _last = rows.pop();
            }
            2 => rows.reverse(),
            3 => {
                let first = rows.first_mut().ok_or(IndexError::Conflict)?;
                *first = RegisteredObject::unscoped(
                    ObjectRef(Digest([255; 32])),
                    first.bytes,
                    first.children.clone(),
                );
            }
            _ => return Err(IndexError::Storage("injected installation failure".into())),
        }
        Ok(RegisteredObjectBatch::unscoped(rows))
    }
    fn get(&self, r: ObjectRef, b: &mut WorkBudget) -> Result<StoredObject, IndexError> {
        self.inner.get(r, b)
    }
}

#[derive(Default)]
struct Custody {
    failure: usize,
    reserved: Mutex<Vec<Vec<ObjectRef>>>,
    confirms: AtomicUsize,
}
impl ObjectCustodyPort for Custody {
    fn reserve_objects(&self, _a: AttemptRef, rows: &[RegisteredObject]) -> Result<(), IndexError> {
        self.reserved
            .lock()
            .map_err(|e| IndexError::Storage(e.to_string()))?
            .push(rows.iter().map(|r| r.reference).collect());
        if self.failure == 1 {
            return Err(IndexError::Storage("injected reserve failure".into()));
        }
        Ok(())
    }
    fn register_objects(
        &self,
        _a: AttemptRef,
        _rows: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        let _previous = self.confirms.fetch_add(1, Ordering::SeqCst);
        if self.failure == 2 {
            return Err(IndexError::Storage("injected confirm failure".into()));
        }
        Ok(())
    }
}

struct Ledger {
    live: AtomicU64,
    peak: AtomicU64,
    cap: u64,
}
impl MemoryLeaseOwner for Ledger {
    fn release(&self, bytes: u64) {
        let _previous = self.live.fetch_sub(bytes, Ordering::SeqCst);
    }
}
struct Scope(Arc<Ledger>);
impl MemoryScopePort for Scope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let previous = self
            .0
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_add(bytes).filter(|next| *next <= self.0.cap)
            })
            .map_err(|_old| IndexError::ResourceLimited)?;
        let next = previous
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let _previous = self.0.peak.fetch_max(next, Ordering::SeqCst);
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}
fn scoped(cap: u64) -> (WorkBudget, Arc<Ledger>) {
    let mut b = budget();
    let ledger = Arc::new(Ledger {
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        cap,
    });
    b.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
    (b, ledger)
}
fn context<'a>(
    objects: &'a Objects,
    custody: &'a Custody,
    scratch: &'a quanta_index_storage::FileScratchSpace,
    b: &'a mut WorkBudget,
) -> AttemptContext<'a> {
    AttemptContext {
        attempt: AttemptRef {
            operation: operation(1),
            attempt: AttemptId([8; 16]),
            fence: 1,
        },
        objects,
        maps: objects.inner.as_ref(),
        custody,
        scratch,
        budget: b,
    }
}

#[test]
fn prepared_searchable_batch_preserves_canonical_graph_and_refunds_custody() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = Arc::new(FileObjectStore::open(temp.path().join("objects"))?);
    let objects = Objects::new(store.clone(), 0);
    let custody = Custody::default();
    let scratch = quanta_index_storage::FileScratchSpace::open_standalone(
        temp.path().join("scratch"),
        1_048_576,
    )?;
    let child = store.identify(LAYOUT, &[], &mut budget())?.reference;
    let parent = store
        .identify(b"coverage", &[child], &mut budget())?
        .reference;
    let (mut work, ledger) = scoped(1_048_576);
    let mut ctx = context(&objects, &custody, &scratch, &mut work);
    let payload = StagedPayload::copy(LAYOUT, ctx.budget)?;
    let prep = SearchablePreparation::object(payload, &[], &mut ctx)?;
    assert_eq!(prep.reference(), Some(child));
    assert!(matches!(
        store.get(child, &mut budget()),
        Err(IndexError::CorruptObject(_))
    ));
    assert_eq!(objects.layout_batches.load(Ordering::SeqCst), 0);
    assert!(ledger.live.load(Ordering::SeqCst) > 0);
    let parent_payload = StagedPayload::copy(b"coverage", ctx.budget)?;
    assert_eq!(
        prep.install_parent(parent_payload, &[child], &mut ctx)?,
        parent
    );
    assert_eq!(objects.layout_batches.load(Ordering::SeqCst), 1);
    assert_eq!(
        *custody.reserved.lock().map_err(|e| e.to_string())?,
        vec![vec![child, parent]]
    );
    assert_eq!(custody.confirms.load(Ordering::SeqCst), 1);
    assert_eq!(store.get(child, &mut budget())?.payload, LAYOUT);
    assert_eq!(
        store.get(parent, &mut budget())?.registration.children,
        [child]
    );
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn prepared_searchable_refuses_missing_child_cross_attempt_and_cancel_before_install() -> TestResult
{
    for mode in 0..5 {
        let temp = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(temp.path().join("objects"))?);
        let objects = Objects::new(store, 0);
        let custody = Custody::default();
        let scratch = quanta_index_storage::FileScratchSpace::open_standalone(
            temp.path().join("scratch"),
            1_048_576,
        )?;
        let (mut work, ledger) = scoped(1_048_576);
        let mut ctx = context(&objects, &custody, &scratch, &mut work);
        let prep =
            SearchablePreparation::object(StagedPayload::copy(LAYOUT, ctx.budget)?, &[], &mut ctx)?;
        let child = prep.reference().ok_or("missing identity")?;
        if mode == 0 {
            drop(prep);
        } else if mode == 4 {
            assert!(matches!(
                quanta_index_core::index::encode_staged_json(&"coverage", 1, ctx.budget),
                Err(IndexError::ResourceLimited)
            ));
            drop(prep);
        } else {
            let parent = StagedPayload::copy(b"coverage", ctx.budget)?;
            let children = [child];
            if mode == 2 {
                ctx.attempt.fence = 2;
            }
            if mode == 3 {
                ctx.budget.request().cancel_handle().cancel();
            }
            let result =
                prep.install_parent(parent, if mode == 1 { &[] } else { &children }, &mut ctx);
            if mode == 3 {
                assert!(matches!(result, Err(IndexError::Cancelled)));
            } else {
                assert!(matches!(result, Err(IndexError::BackendViolation(_))));
            }
        }
        assert_eq!(objects.layout_batches.load(Ordering::SeqCst), 0);
        assert!(
            custody
                .reserved
                .lock()
                .map_err(|e| e.to_string())?
                .is_empty()
        );
        assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[test]
fn prepared_searchable_refuses_bad_batch_outputs_and_preserves_storage_errors() -> TestResult {
    for mode in 1..7 {
        let temp = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(temp.path().join("objects"))?);
        let objects = Objects::new(store, if mode <= 4 { mode } else { 0 });
        let custody = Custody {
            failure: if mode == 5 {
                1
            } else if mode == 6 {
                2
            } else {
                0
            },
            ..Custody::default()
        };
        let scratch = quanta_index_storage::FileScratchSpace::open_standalone(
            temp.path().join("scratch"),
            1_048_576,
        )?;
        let (mut work, ledger) = scoped(1_048_576);
        let mut ctx = context(&objects, &custody, &scratch, &mut work);
        let prep =
            SearchablePreparation::object(StagedPayload::copy(LAYOUT, ctx.budget)?, &[], &mut ctx)?;
        let child = prep.reference().ok_or("missing identity")?;
        let result = prep.install_parent(
            StagedPayload::copy(b"coverage", ctx.budget)?,
            &[child],
            &mut ctx,
        );
        if mode <= 3 {
            assert!(matches!(result, Err(IndexError::BackendViolation(_))));
        } else {
            let message = match mode {
                4 => "injected installation failure",
                5 => "injected reserve failure",
                _ => "injected confirm failure",
            };
            assert_eq!(result, Err(IndexError::Storage(message.into())));
        }
        assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[test]
fn prepared_searchable_existing_roots_and_memory_refusal_preserve_original_routes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = Arc::new(FileObjectStore::open(temp.path().join("objects"))?);
    let old = store.put(LAYOUT, &[], &mut budget())?.reference;
    let objects = Objects::new(store.clone(), 0);
    let custody = Custody::default();
    let scratch = quanta_index_storage::FileScratchSpace::open_standalone(
        temp.path().join("scratch"),
        1_048_576,
    )?;
    let (mut work, ledger) = scoped(1_048_576);
    let mut ctx = context(&objects, &custody, &scratch, &mut work);
    assert_eq!(
        SearchablePreparation::existing(Some(old)).install(&mut ctx)?,
        Some(old)
    );
    assert_eq!(
        SearchablePreparation::existing(None).install(&mut ctx)?,
        None
    );
    assert!(
        custody
            .reserved
            .lock()
            .map_err(|e| e.to_string())?
            .is_empty()
    );
    let parent = SearchablePreparation::existing(Some(old)).install_parent(
        StagedPayload::copy(b"coverage", ctx.budget)?,
        &[old],
        &mut ctx,
    )?;
    assert_eq!(
        store.get(parent, &mut budget())?.registration.children,
        [old]
    );
    assert_eq!(objects.layout_batches.load(Ordering::SeqCst), 0);
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    let (mut denied, ledger) = scoped(0);
    let mut ctx = context(&objects, &custody, &scratch, &mut denied);
    let result =
        SearchablePreparation::object(StagedPayload::copy(LAYOUT, &budget())?, &[], &mut ctx);
    assert!(matches!(result, Err(IndexError::ResourceLimited)));
    assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn prepared_searchable_existing_parent_keeps_original_scalar_budget_and_memory() -> TestResult {
    let mut golden = None;
    for via_preparation in [false, true] {
        let temp = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(temp.path().join("objects"))?);
        let old = store.put(LAYOUT, &[], &mut budget())?.reference;
        let objects = Objects::new(store.clone(), 0);
        let custody = Custody::default();
        let scratch = quanta_index_storage::FileScratchSpace::open_standalone(
            temp.path().join("scratch"),
            1_048_576,
        )?;
        let (mut work, ledger) = scoped(1_048_576);
        let (parent, root) = {
            let mut ctx = context(&objects, &custody, &scratch, &mut work);
            let payload = StagedPayload::copy(b"coverage", ctx.budget)?;
            let parent = if via_preparation {
                SearchablePreparation::existing(Some(old)).install_parent(
                    payload,
                    &[old],
                    &mut ctx,
                )?
            } else {
                // The original independent scalar sequence is the oracle.
                // It identifies and installs one parent without batch backing.
                let expected = ctx
                    .objects
                    .identify(&payload.bytes, &[old], ctx.budget)?
                    .reference;
                let actual = ctx.put(&payload.bytes, &[old])?;
                assert_eq!(actual, expected);
                drop(payload);
                actual
            };
            let edit = MapEdit::objects(b"member".to_vec(), None, Some(parent));
            let root = ctx.apply(quanta_index_core::index::MapRoot(None), &[edit])?;
            (parent, root)
        };
        assert_eq!(objects.identifies.load(Ordering::SeqCst), 1);
        assert_eq!(objects.scalar_installs.load(Ordering::SeqCst), 1);
        assert_eq!(objects.all_batches.load(Ordering::SeqCst), 0);
        assert_eq!(ledger.live.load(Ordering::SeqCst), 0);
        let observed = (
            parent,
            root,
            work.usage(),
            ledger.peak.load(Ordering::SeqCst),
        );
        if let Some(expected) = golden.as_ref() {
            assert_eq!(&observed, expected);
        } else {
            golden = Some(observed);
        }
    }
    Ok(())
}

struct PreparedLayoutOwner(CountingOwnerAdapter);
impl IndexOwnerBuildPort for PreparedLayoutOwner {
    type Candidate = quanta_index_core::index::OwnerMemberBinding;
    fn resource_demand(&self) -> ResourceNeed {
        self.0.resource_demand()
    }
    fn validate_instance(&self, i: &InstanceSpec) -> Result<(), IndexError> {
        self.0.validate_instance(i)
    }
    fn prepare_owner<'a>(
        &'a self,
        i: &'a InstanceSpec,
        o: OwnerKey,
        input: quanta_index_core::index::OwnerInputBinding,
        prior: Option<Self::Candidate>,
        ctx: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        self.0.prepare_owner(i, o, input, prior, ctx)
    }
    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        ctx: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        self.0.verify_owner(candidate, ctx)
    }
    fn prepare_searchable<'a>(
        &'a self,
        _i: &'a InstanceSpec,
        _prior: Option<&'a quanta_index_core::index::CoverageBinding>,
        owners: quanta_index_core::index::MapRoot,
        _materialized: Option<(OwnerKey, ObjectRef)>,
        _object_maps: Option<&'a Arc<dyn quanta_index_core::index::ports::ObjectMapBatchPort>>,
        ctx: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, SearchablePreparation> {
        Box::pin(async move {
            let payload = StagedPayload::copy(LAYOUT, ctx.budget)?;
            match owners.0 {
                Some(root) => SearchablePreparation::object(payload, &[root], ctx),
                None => SearchablePreparation::object(payload, &[], ctx),
            }
        })
    }
}

#[test]
fn materialize_prepared_searchable_batches_metadata_and_replays_original_attempt() -> TestResult {
    for injected_failure in [false, true] {
        let temp = tempfile::tempdir()?;
        let store = Arc::new(FileObjectStore::open(temp.path().join("objects"))?);
        let objects = Arc::new(Objects::new(store.clone(), 0));
        let catalog = Arc::new(SqliteCatalog::open(
            temp.path(),
            Duration::from_millis(100),
        )?);
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = AdapterRegistry::new();
        registry.register_owner(
            AdapterDescriptor {
                kind: spec().kind,
                schema: spec().schema,
                format: spec().format,
            },
            PreparedLayoutOwner(CountingOwnerAdapter(calls.clone())),
        )?;
        let services = KernelServices::new(
            objects.clone(),
            store.clone(),
            catalog.clone(),
            Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
                temp.path().join("scratch"),
                1_048_576,
            )?),
            Duration::from_secs(30),
        )?;
        let engine = Kernel::new(services, registry);
        let configured = configured_owner(&engine, &store)?;
        let record = stage(&store, "selected.rs", b"selected owner")?;
        let pending = published(block_on(engine.submit(
            request(
                &engine,
                &catalog,
                &store,
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
        let before = coverage(&engine, &store, pending)?;
        let pin = engine.pin_view(ViewSelection::Exact(pending), &budget())?;
        let _previous = objects.layout_batches.swap(0, Ordering::SeqCst);
        let claim = engine.claim_work(
            &intent_for(&engine, owner(1))?,
            Duration::from_secs(120),
            &budget(),
        )?;
        let request = UpdateRequest {
            operation: operation(3),
            expected_view: pending,
            kind: UpdateKind::Materialize { claim },
        };
        let mut work = budget();
        if injected_failure {
            objects.fault.store(4, Ordering::SeqCst);
        }
        let result = block_on(engine.submit(request, &mut work));
        let result = if injected_failure {
            assert_eq!(result?, SubmitOutcome::Unknown(operation(3)));
            assert!(format!("{work:?}").contains("failed_stage: Some(Finalize)"));
            assert!(matches!(
                catalog.inspect(
                    operation(3),
                    quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
                )?,
                OperationStatus::Pending(_)
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
                pending
            );
            assert_eq!(coverage(&engine, &store, pending)?, before);
            objects.fault.store(0, Ordering::SeqCst);
            // Same operation and cumulative budget; the staged owner artifact
            // must survive and the provider must not be called again.
            block_on(engine.resume(operation(3), &mut work))?
        } else {
            result?
        };
        let completed = published(result)?;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            objects.layout_batches.load(Ordering::SeqCst),
            if injected_failure { 2 } else { 1 }
        );
        let after = coverage(&engine, &store, completed)?;
        assert_eq!(
            after.status,
            quanta_index_core::index::CoverageStatus::Complete
        );
        let root = after.searchable.ok_or("missing prepared layout")?;
        assert_eq!(store.get(root, &mut budget())?.payload, LAYOUT);
        assert_eq!(
            store.get(root, &mut budget())?.registration.children,
            [after.owners.0.ok_or("missing owners")?]
        );
        assert_eq!(coverage(&engine, &store, pending)?, before);
        assert_eq!(pin.view().identity, pending);
    }
    Ok(())
}
