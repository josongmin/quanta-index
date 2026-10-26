use super::*;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_core::index::{
    Digest, MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits, WorkUsage,
};
use quanta_index_storage::FileObjectStore;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn budget() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 64 * 1024 * 1024,
            mutations: 100_000,
        },
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
    )
}

struct Reads {
    store: FileObjectStore,
    gets: AtomicUsize,
}

impl ImmutableObjectPort for Reads {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.store.identify(payload, children, budget)
    }
    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.store.put(payload, children, budget)
    }
    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.store.install(payload, children, sink, budget)
    }
    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        self.store.install_batch(inputs, sink, budget)
    }
    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let _prior = self.gets.fetch_add(1, Ordering::SeqCst);
        self.store.get(reference, budget)
    }
    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        self.store.read_retained(object, budget)
    }
}

/// A different port has independent costs and does not opt into DATA reuse.
struct NoReuse<'a>(&'a Reads);

impl ImmutableObjectPort for NoReuse<'_> {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.0.identify(payload, children, budget)
    }
    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.0.put(payload, children, budget)
    }
    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.0.install(payload, children, sink, budget)
    }
    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        self.0.install_batch(inputs, sink, budget)
    }
    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        budget.charge(WorkUsage {
            reads: 7,
            bytes: 17,
            ..WorkUsage::default()
        })?;
        self.0.get(reference, budget)
    }
}

fn fixture() -> Result<(tempfile::TempDir, Reads, ObjectRef), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = FileObjectStore::open(root.path().join("store"))?;
    let mut work = budget();
    let child = store.put(b"fixed-child", &[], &mut work)?.reference;
    let parent = store.put(b"fixed-parent", &[child], &mut work)?.reference;
    Ok((
        root,
        Reads {
            store,
            gets: AtomicUsize::new(0),
        },
        parent,
    ))
}

fn require(value: bool, message: &str) -> Result<(), IndexError> {
    if value {
        Ok(())
    } else {
        Err(IndexError::BackendViolation(message.into()))
    }
}

#[test]
fn same_attempt_reuses_real_object_without_reopening_or_reducing_logical_charge() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let reads = AttemptObjectReads::new(&objects, 1)?;
    let mut cold = budget();
    let first = reads.get(reference, &mut cold)?;
    let mut warm = budget();
    let second = reads.get(reference, &mut warm)?;
    require(first == second, "reused immutable object differs")?;
    require(
        first.payload.as_slice() == b"fixed-parent",
        "parent payload differs from golden",
    )?;
    require(
        first.registration.children.len() == 1,
        "parent child graph lost",
    )?;
    require(
        cold.usage() == warm.usage(),
        "reuse changed original file-store read costs",
    )?;
    require(
        objects.gets.load(Ordering::SeqCst) == 1,
        "same attempt reopened immutable DATA",
    )?;
    require(
        first.payload.as_ptr() == second.payload.as_ptr(),
        "reuse copied admitted DATA",
    )?;
    Ok(())
}

#[test]
fn another_attempt_reauthenticates_and_refunds_prior_data() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let memory = Arc::new(Memory::default());
    let mut work = budget();
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(Arc::clone(&memory))))));
    {
        let reads = AttemptObjectReads::new(&objects, 1)?;
        drop(reads.get(reference, &mut work)?);
        require(
            memory.0.load(Ordering::SeqCst) > 0,
            "first window retained no admitted DATA",
        )?;
    }
    require(
        memory.0.load(Ordering::SeqCst) == 0,
        "old window retained DATA or table memory",
    )?;
    let reads = AttemptObjectReads::new(&objects, 1)?;
    drop(reads.get(reference, &mut work)?);
    require(
        objects.gets.load(Ordering::SeqCst) == 2,
        "DATA reuse escaped the previous attempt",
    )?;
    drop(reads);
    require(
        memory.0.load(Ordering::SeqCst) == 0,
        "retry window did not refund admitted memory",
    )?;
    Ok(())
}

#[test]
fn reuse_defers_to_the_selected_ports_original_cost_and_refusal() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let port = NoReuse(&objects);
    let reads = AttemptObjectReads::new(&port, 1)?;
    let mut cold = budget();
    drop(reads.get(reference, &mut cold)?);
    let mut warm = budget();
    drop(reads.get(reference, &mut warm)?);
    require(
        cold.usage() == warm.usage(),
        "Kernel replaced another port's read costs",
    )?;
    require(warm.usage().reads == 8, "independent port was not consumed")?;
    require(
        objects.gets.load(Ordering::SeqCst) == 2,
        "unsupported reuse bypassed original get",
    )?;
    let mut denied = WorkBudget::with_request(
        WorkLimits {
            reads: 7,
            writes: 100,
            bytes: 4096,
            mutations: 100,
        },
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    require(
        reads.get(reference, &mut denied) == Err(IndexError::ResourceLimited),
        "reuse bypassed the selected port's original refusal",
    )?;
    Ok(())
}

#[test]
fn collision_evicts_data_without_aliasing_another_identity() -> TestResult {
    let (_root, objects, first_ref) = fixture()?;
    let second_ref = objects
        .put(b"independent-second", &[], &mut budget())?
        .reference;
    let reads = AttemptObjectReads::with_bounds(&objects, 1, 4096);
    let first = reads.get(first_ref, &mut budget())?;
    let second = reads.get(second_ref, &mut budget())?;
    let again = reads.get(first_ref, &mut budget())?;
    require(
        second.payload.as_slice() == b"independent-second",
        "collision returned prior payload",
    )?;
    require(
        again == first && objects.gets.load(Ordering::SeqCst) == 3,
        "evicted identity was treated as a hit",
    )?;
    Ok(())
}

#[test]
fn cancelled_or_unfunded_hit_never_returns_retained_data() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let reads = AttemptObjectReads::new(&objects, 1)?;
    drop(reads.get(reference, &mut budget())?);
    let mut expired = WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 4096,
            mutations: 100,
        },
        Instant::now()
            .checked_sub(Duration::from_secs(1))
            .ok_or(IndexError::ResourceLimited)?,
    );
    require(
        reads.get(reference, &mut expired) == Err(IndexError::ResourceLimited),
        "expired hit changed the original deadline refusal",
    )?;
    let mut cancelled = budget();
    cancelled.request().cancel_handle().cancel();
    require(
        reads.get(reference, &mut cancelled) == Err(IndexError::Cancelled),
        "cancelled hit returned DATA",
    )?;
    let mut denied = WorkBudget::with_request(
        WorkLimits {
            reads: 0,
            writes: 100,
            bytes: 4096,
            mutations: 100,
        },
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    require(
        reads.get(reference, &mut denied) == Err(IndexError::ResourceLimited),
        "read-unfunded hit returned DATA",
    )?;
    require(
        objects.gets.load(Ordering::SeqCst) == 1,
        "failed hit reopened the underlying object",
    )?;
    Ok(())
}

#[test]
fn oversized_data_and_backend_failures_are_never_retained() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let reads = AttemptObjectReads::with_bounds(&objects, 4, 1);
    drop(reads.get(reference, &mut budget())?);
    drop(reads.get(reference, &mut budget())?);
    require(
        objects.gets.load(Ordering::SeqCst) == 2,
        "oversized DATA exceeded the window",
    )?;
    require(
        reads.lock()?.slots.is_empty(),
        "oversized DATA allocated a table",
    )?;
    let absent = ObjectRef(Digest([0; 32]));
    let one = reads.get(absent, &mut budget());
    let two = reads.get(absent, &mut budget());
    require(
        one.is_err() && one == two,
        "failed read was hidden or changed",
    )?;
    require(
        objects.gets.load(Ordering::SeqCst) == 4,
        "backend failure was retained",
    )?;
    Ok(())
}

#[test]
fn disabled_window_retains_the_original_read_path() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let reads = AttemptObjectReads::new(&objects, 0)?;
    drop(reads.get(reference, &mut budget())?);
    drop(reads.get(reference, &mut budget())?);
    require(
        objects.gets.load(Ordering::SeqCst) == 2,
        "disabled window retained DATA",
    )?;
    require(
        reads.lock()?.slots.is_empty(),
        "disabled window allocated backing",
    )?;
    Ok(())
}

#[test]
fn contended_window_refuses_without_waiting_or_reopening() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let reads = AttemptObjectReads::new(&objects, 1)?;
    drop(reads.get(reference, &mut budget())?);
    let _held = reads.lock()?;
    require(
        reads.get(reference, &mut budget()) == Err(IndexError::ResourceLimited),
        "contended DATA window did not refuse",
    )?;
    require(
        objects.gets.load(Ordering::SeqCst) == 1,
        "contended window reopened an object",
    )?;
    Ok(())
}

#[derive(Default)]
struct Memory(AtomicU64);

impl MemoryLeaseOwner for Memory {
    fn release(&self, bytes: u64) {
        let _prior = self.0.fetch_sub(bytes, Ordering::SeqCst);
    }
}

struct Scope(Arc<Memory>);

impl MemoryScopePort for Scope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _prior = self.0.0.fetch_add(bytes, Ordering::SeqCst);
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

struct RefuseTable(Arc<Memory>);

impl MemoryScopePort for RefuseTable {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if lifetime == MemoryLifetime::Temporary && bytes > 512 {
            return Err(IndexError::ResourceLimited);
        }
        Scope(Arc::clone(&self.0)).reserve(bytes, lifetime)
    }
}

struct RefuseNewMemory {
    memory: Arc<Memory>,
    deny: AtomicBool,
}

impl MemoryScopePort for RefuseNewMemory {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if self.deny.load(Ordering::SeqCst) {
            return Err(IndexError::ResourceLimited);
        }
        Scope(Arc::clone(&self.memory)).reserve(bytes, lifetime)
    }
}

#[test]
fn borrowed_selected_artifact_needs_no_new_memory_and_reauthenticates_next_pin() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let memory = Arc::new(Memory::default());
    let scope = Arc::new(RefuseNewMemory {
        memory: Arc::clone(&memory),
        deny: AtomicBool::new(false),
    });
    let mut cold = budget();
    cold.bind_memory_scope(Some(MemoryScope::new(scope.clone())));
    let original = objects.get(reference, &mut cold)?;
    let held = memory.0.load(Ordering::SeqCst);
    require(held > 0, "original DATA had no owned memory")?;

    // The first get has consumed every available memory allowance. A selected
    // reread can only succeed by borrowing that exact authenticated DATA.
    scope.deny.store(true, Ordering::SeqCst);
    let mut warm = budget();
    warm.bind_memory_scope(Some(MemoryScope::new(scope.clone())));
    {
        let selected = AttemptObjectReads::selected(&objects, &original);
        let reread = selected.get(reference, &mut warm)?;
        require(reread == original, "selected DATA changed")?;
        require(
            cold.usage() == warm.usage(),
            "selected DATA changed the original read fee",
        )?;
        require(
            objects.gets.load(Ordering::SeqCst) == 1,
            "selected DATA reopened its original object",
        )?;
        require(
            memory.0.load(Ordering::SeqCst) == held,
            "selected DATA reserved additional memory",
        )?;
        let mut cancelled = budget();
        cancelled.request().cancel_handle().cancel();
        require(
            selected.get(reference, &mut cancelled) == Err(IndexError::Cancelled),
            "cancelled selected read returned DATA",
        )?;
        let mut denied = WorkBudget::with_request(
            WorkLimits {
                reads: 0,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        require(
            selected.get(reference, &mut denied) == Err(IndexError::ResourceLimited),
            "unfunded selected read returned DATA",
        )?;
        drop(reread);
    }
    drop(original);
    require(
        memory.0.load(Ordering::SeqCst) == 0,
        "selected view retained the original DATA after drop",
    )?;
    scope.deny.store(false, Ordering::SeqCst);
    let mut next = budget();
    next.bind_memory_scope(Some(MemoryScope::new(scope)));
    let next_original = objects.get(reference, &mut next)?;
    require(
        objects.gets.load(Ordering::SeqCst) == 2,
        "next pin inherited prior DATA without a physical authentication",
    )?;
    drop(next_original);
    require(
        memory.0.load(Ordering::SeqCst) == 0,
        "next pin did not refund DATA",
    )?;
    Ok(())
}

#[test]
fn borrowed_selected_mode_passes_other_identity_to_original_port() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let original = objects.get(reference, &mut budget())?;
    let child = *original
        .registration
        .children
        .first()
        .ok_or(IndexError::ResourceLimited)?;
    let selected = AttemptObjectReads::selected(&objects, &original);
    let actual = selected.get(child, &mut budget())?;
    require(
        actual.registration.reference == child && actual.payload.as_slice() == b"fixed-child",
        "unselected identity was not authenticated by the original port",
    )?;
    require(
        objects.gets.load(Ordering::SeqCst) == 2,
        "selected mode retained another identity",
    )?;
    Ok(())
}

#[test]
fn table_memory_denial_preserves_refusal_and_refunds_read_data() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let memory = Arc::new(Memory::default());
    let mut work = budget();
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(RefuseTable(Arc::clone(
        &memory,
    ))))));
    let reads = AttemptObjectReads::new(&objects, 8)?;
    require(
        reads.get(reference, &mut work) == Err(IndexError::ResourceLimited),
        "table memory denial was hidden",
    )?;
    require(
        reads.lock()?.slots.is_empty(),
        "refused table allocated or retained DATA",
    )?;
    require(
        memory.0.load(Ordering::SeqCst) == 0,
        "table denial leaked the original read allocation",
    )?;
    Ok(())
}

#[test]
fn data_retains_original_memory_until_last_shared_value_drops() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let memory = Arc::new(Memory::default());
    let mut work = budget();
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(Arc::clone(&memory))))));
    let reads = AttemptObjectReads::new(&objects, 1)?;
    let object = reads.get(reference, &mut work)?;
    let payload_bytes = object.memory_bytes().ok_or(IndexError::ResourceLimited)?;
    require(
        memory.0.load(Ordering::SeqCst) > payload_bytes,
        "table backing was not admitted",
    )?;
    drop(reads);
    require(
        memory.0.load(Ordering::SeqCst) == payload_bytes,
        "window DATA or table reservation leaked",
    )?;
    drop(object);
    require(
        memory.0.load(Ordering::SeqCst) == 0,
        "last shared DATA did not refund original memory",
    )?;
    Ok(())
}
