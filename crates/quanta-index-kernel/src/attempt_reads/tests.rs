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
    wrong_retained: AtomicBool,
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
        if self.wrong_retained.swap(false, Ordering::SeqCst) {
            let other = *object
                .registration
                .children
                .first()
                .ok_or(IndexError::ResourceLimited)?;
            return self.get(other, budget);
        }
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
            wrong_retained: AtomicBool::new(false),
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
fn primary_collision_reuses_both_real_objects_without_changing_read_cost() -> TestResult {
    let (_root, objects, _reference) = fixture()?;
    let reads = AttemptObjectReads::with_bounds(&objects, 16, 4096);
    let mut seen = Vec::new();
    let mut pair = None;
    for seed in 0..128 {
        let payload = format!("collision-candidate-{seed}");
        let reference = objects
            .put(payload.as_bytes(), &[], &mut budget())?
            .reference;
        // The original one-slot lookup uses the low four bits of the digest's
        // first byte for this fixed 16-slot table. Input selection must not
        // depend on the new two-candidate implementation.
        let primary = reference
            .0
            .0
            .first()
            .copied()
            .ok_or(IndexError::ResourceLimited)?
            & 15;
        if let Some((_, prior_seed, prior)) = seen
            .iter()
            .find(|(seen_primary, _, _)| *seen_primary == primary)
        {
            pair = Some((*prior, *prior_seed, reference, seed));
            break;
        }
        seen.push((primary, seed, reference));
    }
    let (first_ref, first_seed, second_ref, second_seed) = pair.ok_or_else(|| {
        IndexError::BackendViolation("real object primary collision pair absent".into())
    })?;
    let mut cold = budget();
    let first = reads.get(first_ref, &mut cold)?;
    let mut second_cold = budget();
    let second = reads.get(second_ref, &mut second_cold)?;
    let mut warm = budget();
    let again = reads.get(first_ref, &mut warm)?;
    let mut second_warm = budget();
    let second_again = reads.get(second_ref, &mut second_warm)?;
    require(
        first == again && first.payload.as_ptr() == again.payload.as_ptr(),
        "primary collision did not retain the original authenticated DATA",
    )?;
    require(
        first.payload.as_slice() == format!("collision-candidate-{first_seed}").as_bytes()
            && second.payload.as_slice() == format!("collision-candidate-{second_seed}").as_bytes(),
        "colliding objects changed their fixed input payloads",
    )?;
    require(
        second.registration.reference == second_ref
            && second == second_again
            && second.payload.as_ptr() == second_again.payload.as_ptr(),
        "alternate slot returned another identity",
    )?;
    require(
        objects.gets.load(Ordering::SeqCst) == 2,
        "primary collision reopened a retained object",
    )?;
    require(
        cold.usage() == warm.usage() && second_cold.usage() == second_warm.usage(),
        "alternate slot changed the original logical read cost",
    )?;
    Ok(())
}

#[test]
fn byte_full_alternate_falls_back_to_primary_eviction() -> TestResult {
    let (_root, objects, _reference) = fixture()?;
    let mut seen = Vec::new();
    let mut pair = None;
    for seed in 0..128 {
        let payload = format!("byte-bound-candidate-{seed}");
        let reference = objects
            .put(payload.as_bytes(), &[], &mut budget())?
            .reference;
        // Fixed 16-slot original primary address, independent of the new
        // alternate calculation and insertion policy.
        let primary = reference
            .0
            .0
            .first()
            .copied()
            .ok_or(IndexError::ResourceLimited)?
            & 15;
        if let Some((_, first_seed, first_ref)) = seen
            .iter()
            .find(|(seen_primary, _, _)| *seen_primary == primary)
        {
            pair = Some((*first_ref, *first_seed, reference, seed));
            break;
        }
        seen.push((primary, seed, reference));
    }
    let (first_ref, first_seed, second_ref, second_seed) = pair.ok_or_else(|| {
        IndexError::BackendViolation("real object primary collision pair absent".into())
    })?;
    let first_payload = format!("byte-bound-candidate-{first_seed}");
    let second_payload = format!("byte-bound-candidate-{second_seed}");
    let first_bytes = StoredObject::allocation_bytes(first_payload.len(), 0)?;
    let second_bytes = StoredObject::allocation_bytes(second_payload.len(), 0)?;
    let byte_limit = first_bytes.max(second_bytes);
    require(
        first_bytes
            .checked_add(second_bytes)
            .is_some_and(|sum| sum > byte_limit),
        "byte cap does not force alternate refusal",
    )?;
    let reads = AttemptObjectReads::with_bounds(&objects, 16, byte_limit);
    let mut first_work = budget();
    let first = reads.get(first_ref, &mut first_work)?;
    let second = reads.get(second_ref, &mut budget())?;
    let mut repeated_work = budget();
    let repeated = reads.get(first_ref, &mut repeated_work)?;
    require(
        first.payload.as_slice() == first_payload.as_bytes()
            && second.payload.as_slice() == second_payload.as_bytes()
            && repeated == first,
        "byte-bound collision changed authenticated object DATA",
    )?;
    require(
        objects.gets.load(Ordering::SeqCst) == 3,
        "byte-full alternate retained a sticky primary instead of evicting it",
    )?;
    require(
        first_work.usage() == repeated_work.usage(),
        "byte-bound eviction changed the original logical read charge",
    )?;
    require(
        reads.lock()?.retained == first_bytes,
        "byte-bound replacement exceeded the cap or failed to retain the latest DATA",
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
fn retained_read_survives_zero_slot_and_eviction_with_original_custody() -> TestResult {
    let (_root, objects, first_ref) = fixture()?;
    let second_ref = objects
        .put(b"independent-second", &[], &mut budget())?
        .reference;
    let memory = Arc::new(Memory::default());
    let scope = MemoryScope::new(Arc::new(Scope(Arc::clone(&memory))));
    let mut first_work = budget();
    first_work.bind_memory_scope(Some(scope.clone()));
    let zero = AttemptObjectReads::with_bounds(&objects, 0, 0);
    let first = zero.get(first_ref, &mut first_work)?;
    let cold = first_work.usage();
    let reused = zero.read_retained(&first, &mut first_work)?;
    require(
        first.payload.as_ptr() == reused.payload.as_ptr()
            && first_work.usage().reads
                == cold
                    .reads
                    .checked_mul(2)
                    .ok_or(IndexError::ResourceLimited)?
            && first_work.usage().bytes
                == cold
                    .bytes
                    .checked_mul(2)
                    .ok_or(IndexError::ResourceLimited)?
            && objects.gets.load(Ordering::SeqCst) == 1,
        "zero-slot retained DATA reopened or changed the selected port's charge",
    )?;
    drop(reused);
    drop(first);
    drop(zero);
    require(
        memory.0.load(Ordering::SeqCst) == 0,
        "zero-slot DATA lease leaked",
    )?;

    let window = AttemptObjectReads::with_bounds(&objects, 1, 4096);
    let mut second_work = budget();
    second_work.bind_memory_scope(Some(scope));
    let first = window.get(first_ref, &mut second_work)?;
    let second = window.get(second_ref, &mut second_work)?;
    let again = window.read_retained(&first, &mut second_work)?;
    require(
        again == first
            && again.payload.as_ptr() == first.payload.as_ptr()
            && objects.gets.load(Ordering::SeqCst) == 3,
        "evicted retained DATA reopened or changed identity",
    )?;
    require(
        memory.0.load(Ordering::SeqCst) > 0,
        "retained DATA lost its original allocation grant",
    )?;
    drop(again);
    drop(first);
    drop(second);
    drop(window);
    require(
        memory.0.load(Ordering::SeqCst) == 0,
        "last retained DATA or window grant was not refunded",
    )?;
    Ok(())
}

#[test]
fn retained_read_preserves_no_reuse_cost_and_refusal() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let port = NoReuse(&objects);
    let reads = AttemptObjectReads::with_bounds(&port, 0, 0);
    let mut work = budget();
    let original = reads.get(reference, &mut work)?;
    let cold_reads = work.usage().reads;
    let reread = reads.read_retained(&original, &mut work)?;
    require(
        reread == original
            && work.usage().reads.checked_sub(cold_reads) == Some(8)
            && objects.gets.load(Ordering::SeqCst) == 2,
        "NoReuse retained read bypassed its fresh read or logical charge",
    )?;
    let mut denied = WorkBudget::with_request(
        WorkLimits {
            reads: 7,
            writes: 100,
            bytes: 4096,
            mutations: 100,
        },
        work.request().clone(),
    );
    require(
        reads.read_retained(&original, &mut denied) == Err(IndexError::ResourceLimited),
        "NoReuse retained read bypassed its original refusal",
    )?;
    Ok(())
}

#[test]
fn retained_read_checks_reference_and_cancel_before_reuse() -> TestResult {
    let (_root, objects, reference) = fixture()?;
    let memory = Arc::new(Memory::default());
    let mut work = budget();
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(Arc::clone(&memory))))));
    let reads = AttemptObjectReads::with_bounds(&objects, 0, 0);
    let original = reads.get(reference, &mut work)?;
    let prior_gets = objects.gets.load(Ordering::SeqCst);
    objects.wrong_retained.store(true, Ordering::SeqCst);
    require(
        matches!(
            reads.read_retained(&original, &mut work),
            Err(IndexError::CorruptObject(_))
        ),
        "retained port changed the requested identity without refusal",
    )?;
    let mut cancelled = WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 64 * 1024 * 1024,
            mutations: 100_000,
        },
        work.request().clone(),
    );
    cancelled.request().cancel_handle().cancel();
    require(
        reads.read_retained(&original, &mut cancelled) == Err(IndexError::Cancelled)
            && Some(objects.gets.load(Ordering::SeqCst)) == prior_gets.checked_add(1),
        "cancelled retained read reached the selected port",
    )?;
    drop(original);
    drop(reads);
    require(
        memory.0.load(Ordering::SeqCst) == 0,
        "retained refusal leaked the original allocation grant",
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
        let prior = warm.usage();
        let retained = selected.read_retained(&reread, &mut warm)?;
        require(
            retained.payload.as_ptr() == original.payload.as_ptr()
                && warm.usage().reads.checked_sub(prior.reads) == Some(cold.usage().reads)
                && warm.usage().bytes.checked_sub(prior.bytes) == Some(cold.usage().bytes)
                && objects.gets.load(Ordering::SeqCst) == 1,
            "selected retained DATA lost the original pinned read policy",
        )?;
        drop(retained);
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
