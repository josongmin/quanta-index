use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
    MemoryScopePort, ObjectBatchInput, ObjectRef, RegisteredObject, WorkBudget, WorkLimits,
    ports::{ImmutableObjectPort, ObjectWriteSink},
};

use super::super::batch_metadata::{BatchIndex, BatchReferences, RegistrationBatch};
use crate::{FileObjectStore, ObjectIoObservation};

macro_rules! check {
    ($condition:expr) => {
        if !$condition {
            return Err(IndexError::BackendViolation(stringify!($condition).into()).into());
        }
    };
}

struct Ledger {
    ceiling: u64,
    live: AtomicU64,
    peak: AtomicU64,
    refunds: AtomicU64,
}

impl MemoryLeaseOwner for Ledger {
    fn release(&self, bytes: u64) {
        let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
        let _prior_refunds = self.refunds.fetch_add(1, Ordering::SeqCst);
    }
}

struct Scope(Arc<Ledger>);

impl MemoryScopePort for Scope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let mut prior = self.0.live.load(Ordering::SeqCst);
        loop {
            let next = prior
                .checked_add(bytes)
                .ok_or(IndexError::ResourceLimited)?;
            if next > self.0.ceiling {
                return Err(IndexError::ResourceLimited);
            }
            match self
                .0
                .live
                .compare_exchange(prior, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => {
                    let _old_peak = self.0.peak.fetch_max(next, Ordering::SeqCst);
                    return Ok(MemoryLease::new(bytes, self.0.clone()));
                }
                Err(actual) => prior = actual,
            }
        }
    }
}

fn budget(ceiling: u64) -> Result<(WorkBudget, Arc<Ledger>), IndexError> {
    let ledger = Arc::new(Ledger {
        ceiling,
        live: AtomicU64::new(0),
        peak: AtomicU64::new(0),
        refunds: AtomicU64::new(0),
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 1000,
            writes: 1000,
            bytes: 10_000_000,
            mutations: 1000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
    Ok((work, ledger))
}

struct Sink {
    ledger: Arc<Ledger>,
    reserve: AtomicU64,
    confirm: AtomicU64,
    panic_on_reserve: bool,
    gate_store: Option<Arc<FileObjectStore>>,
}

impl Sink {
    fn inject_reservation_fault(&self) {
        assert!(
            !self.panic_on_reserve,
            "intentional write reservation unwind"
        );
    }

    fn observe(&self, objects: &[RegisteredObject], confirming: bool) -> Result<(), IndexError> {
        if let Some(store) = &self.gate_store {
            for object in objects {
                let gate = store
                    .gates
                    .get(FileObjectStore::gate_index(object.reference))
                    .ok_or_else(|| {
                        IndexError::BackendViolation("invalid test gate index".into())
                    })?;
                match gate.try_lock() {
                    Err(std::sync::TryLockError::WouldBlock) => {}
                    _ => {
                        return Err(IndexError::BackendViolation(
                            "batch gate ended before confirmation".into(),
                        ));
                    }
                }
            }
        }
        let bytes = objects.iter().try_fold(0u64, |sum, object| {
            sum.checked_add(object.bytes)
                .ok_or(IndexError::ResourceLimited)
        })?;
        if self.ledger.live.load(Ordering::SeqCst) < bytes {
            return Err(IndexError::BackendViolation(
                "wire custody ended before installation".into(),
            ));
        }
        if confirming {
            let _prior = self.confirm.fetch_add(1, Ordering::SeqCst);
        } else {
            let _prior = self.reserve.fetch_add(1, Ordering::SeqCst);
            self.inject_reservation_fault();
        }
        Ok(())
    }
}

impl ObjectWriteSink for Sink {
    fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.observe(std::slice::from_ref(object), false)
    }
    fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.observe(std::slice::from_ref(object), true)
    }
    fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        self.observe(objects, false)
    }
    fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        self.observe(objects, true)
    }
}

fn sink(ledger: &Arc<Ledger>, panic_on_reserve: bool) -> Sink {
    Sink {
        ledger: ledger.clone(),
        reserve: AtomicU64::new(0),
        confirm: AtomicU64::new(0),
        panic_on_reserve,
        gate_store: None,
    }
}

#[test]
fn streaming_identity_preserves_fixed_wire_gold_without_wire_memory()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut work, ledger) = budget(1000)?;
    let cases = [
        (
            b"".as_slice(),
            Vec::new(),
            16,
            [
                98, 49, 101, 59, 108, 218, 160, 95, 90, 219, 102, 118, 254, 172, 165, 251, 141,
                177, 67, 79, 150, 150, 118, 3, 58, 220, 240, 85, 247, 94, 203, 113,
            ],
        ),
        (
            b"wire".as_slice(),
            Vec::new(),
            20,
            [
                172, 62, 140, 183, 172, 249, 116, 224, 71, 173, 41, 40, 234, 139, 129, 7, 136, 141,
                170, 43, 41, 49, 61, 32, 208, 189, 10, 154, 22, 151, 2, 166,
            ],
        ),
        (
            b"wire".as_slice(),
            vec![
                ObjectRef(Digest([7; 32])),
                ObjectRef(Digest([8; 32])),
                ObjectRef(Digest([7; 32])),
            ],
            116,
            [
                112, 64, 153, 229, 121, 26, 105, 104, 101, 139, 13, 22, 93, 114, 56, 54, 65, 56,
                137, 28, 103, 110, 226, 138, 234, 96, 229, 239, 18, 21, 131, 53,
            ],
        ),
    ];
    for (payload, children, bytes, gold) in cases {
        let actual = super::identify(payload, &children, &mut work)?;
        check!(actual.reference == ObjectRef(Digest(gold)));
        check!(actual.bytes == bytes && actual.children == children);
        let registration_bytes = RegisteredObject::allocation_bytes(children.len())?;
        check!(ledger.live.load(Ordering::SeqCst) == registration_bytes);
        let (mut admitted, live) = budget(bytes + registration_bytes)?;
        let prepared = super::prepare(payload, &children, &mut admitted)?;
        check!(prepared.registration == actual);
        check!(live.live.load(Ordering::SeqCst) == bytes + registration_bytes);
        drop(prepared);
        check!(live.live.load(Ordering::SeqCst) == 0);
    }
    check!(ledger.live.load(Ordering::SeqCst) == 0);
    Ok(())
}

#[test]
fn scalar_and_batch_wire_denial_precede_durable_reservation_and_io()
-> Result<(), Box<dyn std::error::Error>> {
    for batch in [false, true] {
        let directory = tempfile::tempdir()?;
        let observer = Arc::new(ObjectIoObservation::default());
        let store = FileObjectStore::open(directory.path().join("store"))?
            .with_io_observation(observer.clone());
        // One packet fits; the second wire refuses before any batch installation.
        let staged = if batch {
            2 * u64::try_from(
                std::mem::size_of::<super::PreparedObject>()
                    + std::mem::size_of::<&super::PreparedObject>(),
            )? + quanta_index_core::index::RegisteredObjectBatch::allocation_bytes(2)?
                + 21
                + RegisteredObject::allocation_bytes(0)?
        } else {
            0
        };
        let (mut work, ledger) = budget(staged)?;
        let sink = sink(&ledger, false);
        let outcome = if batch {
            store
                .install_batch(
                    &[
                        ObjectBatchInput {
                            payload: b"first".to_vec(),
                            children: Vec::new(),
                        },
                        ObjectBatchInput {
                            payload: b"other".to_vec(),
                            children: Vec::new(),
                        },
                    ],
                    &sink,
                    &mut work,
                )
                .map(|_result| ())
        } else {
            store
                .install(b"first", &[], &sink, &mut work)
                .map(|_result| ())
        };
        check!(matches!(outcome, Err(IndexError::ResourceLimited)));
        check!(ledger.live.load(Ordering::SeqCst) == 0);
        check!(ledger.peak.load(Ordering::SeqCst) == staged);
        check!(sink.reserve.load(Ordering::SeqCst) == 0);
        check!(sink.confirm.load(Ordering::SeqCst) == 0);
        let snapshot = observer.snapshot()?;
        check!(snapshot.write_calls == 0 && snapshot.reserve_calls == 0);
        check!(std::fs::read_dir(directory.path().join("store/objects"))?.count() == 0);
    }
    Ok(())
}

#[test]
fn retained_registration_and_output_slot_denial_precede_custody()
-> Result<(), Box<dyn std::error::Error>> {
    struct DenyRetained(Arc<Ledger>);
    impl MemoryScopePort for DenyRetained {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime == MemoryLifetime::Retained {
                return Err(IndexError::ResourceLimited);
            }
            Scope(self.0.clone()).reserve(bytes, lifetime)
        }
    }
    for batch in [false, true] {
        let directory = tempfile::tempdir()?;
        let observer = Arc::new(ObjectIoObservation::default());
        let store = FileObjectStore::open(directory.path().join("store"))?
            .with_io_observation(observer.clone());
        let (mut work, ledger) = budget(1000)?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(DenyRetained(
            ledger.clone(),
        )))));
        let sink = sink(&ledger, false);
        let result = if batch {
            store
                .install_batch(
                    &[ObjectBatchInput {
                        payload: b"first".to_vec(),
                        children: Vec::new(),
                    }],
                    &sink,
                    &mut work,
                )
                .map(|_objects| ())
        } else {
            store
                .install(b"first", &[], &sink, &mut work)
                .map(|_object| ())
        };
        check!(matches!(result, Err(IndexError::ResourceLimited)));
        check!(
            sink.reserve.load(Ordering::SeqCst) == 0 && sink.confirm.load(Ordering::SeqCst) == 0
        );
        let io_counts = observer.snapshot()?;
        check!(io_counts.reserve_calls == 0 && io_counts.write_calls == 0);
        check!(ledger.live.load(Ordering::SeqCst) == 0);
        check!(std::fs::read_dir(directory.path().join("store/objects"))?.count() == 0);
    }
    Ok(())
}

#[test]
fn duplicate_batch_return_shares_metadata_and_keeps_slots_through_consumption()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store = FileObjectStore::open(directory.path().join("store"))?;
    let (mut work, ledger) = budget(1000)?;
    let sink = sink(&ledger, false);
    let input = ObjectBatchInput {
        payload: b"first".to_vec(),
        children: Vec::new(),
    };
    let objects = store.install_batch(&[input.clone(), input], &sink, &mut work)?;
    let first = objects.first().ok_or("missing first ordinal")?;
    let second = objects.get(1).ok_or("missing second ordinal")?;
    check!(std::ptr::eq(&raw const **first, &raw const **second));
    let metadata = RegisteredObject::allocation_bytes(0)?;
    let slots = quanta_index_core::index::RegisteredObjectBatch::allocation_bytes(2)?;
    check!(ledger.live.load(Ordering::SeqCst) == metadata + slots);
    let mut items = objects.into_iter();
    let registration = items.next().ok_or("missing returned registration")?;
    check!(ledger.live.load(Ordering::SeqCst) == metadata + slots);
    drop(items);
    check!(ledger.live.load(Ordering::SeqCst) == metadata);
    drop(registration);
    check!(ledger.live.load(Ordering::SeqCst) == 0);
    Ok(())
}

#[test]
fn wire_charge_survives_actual_install_confirmation_and_unwind()
-> Result<(), Box<dyn std::error::Error>> {
    for (batch, unwind) in [(false, false), (true, false), (false, true), (true, true)] {
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let (mut work, ledger) = budget(1000)?;
        let sink = sink(&ledger, unwind);
        let result = catch_unwind(AssertUnwindSafe(|| {
            if batch {
                store
                    .install_batch(
                        &[
                            ObjectBatchInput {
                                payload: b"first".to_vec(),
                                children: Vec::new(),
                            },
                            ObjectBatchInput {
                                payload: b"other".to_vec(),
                                children: Vec::new(),
                            },
                        ],
                        &sink,
                        &mut work,
                    )
                    .map(|_result| ())
            } else {
                store
                    .install(b"first", &[], &sink, &mut work)
                    .map(|_result| ())
            }
        }));
        check!(ledger.live.load(Ordering::SeqCst) == 0);
        let metadata = if batch {
            2 * u64::try_from(std::mem::size_of::<RegisteredObject>())?
        } else {
            0
        };
        let preparation = if batch {
            2 * u64::try_from(
                std::mem::size_of::<super::PreparedObject>()
                    + std::mem::size_of::<&super::PreparedObject>(),
            )?
        } else {
            0
        };
        let registrations = RegisteredObject::allocation_bytes(0)? * if batch { 2 } else { 1 };
        let returned_slots = if batch { metadata } else { 0 };
        check!(
            ledger.peak.load(Ordering::SeqCst)
                == if batch { 42 } else { 21 }
                    + metadata
                    + preparation
                    + registrations
                    + returned_slots
        );
        check!(ledger.refunds.load(Ordering::SeqCst) == if batch { 7 } else { 2 });
        check!(sink.reserve.load(Ordering::SeqCst) == 1);
        if unwind {
            check!(result.is_err());
            check!(sink.confirm.load(Ordering::SeqCst) == 0);
        } else {
            result.map_err(|_panic| "unexpected writer panic")??;
            check!(sink.confirm.load(Ordering::SeqCst) == 1);
        }
    }
    Ok(())
}

#[test]
fn commit_metadata_denial_precedes_custody_and_missing_child_lookup()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let observer = Arc::new(ObjectIoObservation::default());
    let store = FileObjectStore::open(directory.path().join("store"))?
        .with_io_observation(observer.clone());
    let (mut preparing, _) = budget(1000)?;
    // Prepared caller input is outside this commit-only reservation scope.
    preparing.bind_memory_scope(None);
    let missing = ObjectRef(Digest([71; 32]));
    let parent = store.prepare_object(b"parent", &[missing, missing], &mut preparing)?;
    let objects = [&parent];
    let expected = u64::try_from(std::mem::size_of::<RegisteredObject>())?;
    check!(RegistrationBatch::allocation_bytes(&objects)? == expected);
    let (mut work, ledger) = budget(expected.checked_sub(1).ok_or(IndexError::ResourceLimited)?)?;
    let sink = sink(&ledger, false);
    let result = store.install_prepared_batch(&objects, &sink, &mut work);
    check!(matches!(result, Err(IndexError::ResourceLimited)));
    check!(ledger.live.load(Ordering::SeqCst) == 0 && ledger.peak.load(Ordering::SeqCst) == 0);
    let snapshot = observer.snapshot()?;
    check!(snapshot.reserve_calls == 0 && snapshot.write_calls == 0);
    check!(work.usage().reads == 0);
    check!(std::fs::read_dir(directory.path().join("store/objects"))?.count() == 0);
    Ok(())
}

#[test]
fn registration_sharing_keeps_ordered_repeated_children_and_slots_until_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let (mut preparing, _) = budget(1000)?;
    preparing.bind_memory_scope(None);
    let child = super::prepare(b"child", &[], &mut preparing)?;
    let parent = super::prepare(
        b"parent",
        &[child.registration.reference, child.registration.reference],
        &mut preparing,
    )?;
    let objects = [&child, &parent];
    let expected = u64::try_from(2 * std::mem::size_of::<RegisteredObject>())?;
    let (work, ledger) = budget(expected)?;
    let copied = RegistrationBatch::new(&objects, &work)?;
    check!(copied.as_slice() == [child.registration.clone(), parent.registration.clone()]);
    check!(ledger.live.load(Ordering::SeqCst) == expected);
    drop(copied);
    check!(ledger.live.load(Ordering::SeqCst) == 0);
    check!(ledger.refunds.load(Ordering::SeqCst) == 1);
    let (empty_work, empty_ledger) = budget(0)?;
    let empty = RegistrationBatch::new(&[], &empty_work)?;
    check!(empty.as_slice().is_empty());
    check!(empty_ledger.peak.load(Ordering::SeqCst) == 0);
    Ok(())
}

#[test]
fn fixed_reference_table_preserves_child_first_duplicate_and_256_boundary_laws()
-> Result<(), Box<dyn std::error::Error>> {
    let (work, ledger) = budget(0)?;
    let mut preparing = WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let child = super::identify(b"child", &[], &mut preparing)?;
    let leaf = super::prepare(b"child", &[], &mut preparing)?;
    let parent = super::prepare(
        b"parent",
        &[child.reference, child.reference],
        &mut preparing,
    )?;
    let valid = BatchReferences::new(&[&leaf, &parent], &work)?;
    check!(valid.contains(child.reference)?);
    check!(!valid.contains(ObjectRef(Digest([0; 32])))?);
    check!(matches!(
        BatchReferences::new(&[&parent, &leaf], &work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        BatchReferences::new(&[&leaf, &leaf], &work),
        Err(IndexError::BackendViolation(_))
    ));
    let mut full = Vec::new();
    for ordinal in 0..256u16 {
        full.push(super::prepare(&ordinal.to_be_bytes(), &[], &mut preparing)?);
    }
    let references: Vec<_> = full.iter().collect();
    let first_255 = references.get(..255).ok_or("missing 255 references")?;
    let almost_full = BatchReferences::new(first_255, &work)?;
    check!(
        almost_full.contains(
            full.get(254)
                .ok_or("missing penultimate registration")?
                .registration
                .reference
        )?
    );
    check!(
        !almost_full.contains(
            full.get(255)
                .ok_or("missing last registration")?
                .registration
                .reference
        )?
    );
    let table = BatchReferences::new(&references, &work)?;
    for object in &full {
        check!(table.contains(object.registration.reference)?);
    }
    let mut over = references;
    over.push(&parent);
    check!(matches!(
        BatchReferences::new(&over, &work),
        Err(IndexError::ResourceLimited)
    ));
    check!(ledger.peak.load(Ordering::SeqCst) == 0);
    Ok(())
}

#[test]
fn private_256_object_chain_survives_reopen_across_physical_batch_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store_path = directory.path().join("store");
    let store = FileObjectStore::open(&store_path)?;
    let (mut work, ledger) = budget(1_000_000)?;
    let sink = sink(&ledger, false);
    let mut prepared = Vec::new();
    let mut expected: Vec<(ObjectRef, Vec<u8>, Vec<ObjectRef>)> = Vec::new();
    for ordinal in 0..256u16 {
        let payload = ordinal.to_be_bytes().to_vec();
        let mut children = expected
            .last()
            .map(|(reference, _, _)| vec![*reference])
            .unwrap_or_default();
        if ordinal >= 2 {
            children.push(expected.first().ok_or("missing first chain object")?.0);
        }
        let object = store.prepare_object(&payload, &children, &mut work)?;
        expected.push((object.registration.reference, payload, children));
        prepared.push(object);
    }
    let references: Vec<_> = prepared.iter().collect();
    store.install_prepared_batch(&references, &sink, &mut work)?;
    check!(sink.reserve.load(Ordering::SeqCst) == 1);
    check!(sink.confirm.load(Ordering::SeqCst) == 1);
    drop(references);
    drop(prepared);
    check!(ledger.live.load(Ordering::SeqCst) == 0);
    drop(store);

    let reopened = FileObjectStore::open(&store_path)?;
    let (mut read_work, _) = budget(1_000_000)?;
    for (reference, payload, children) in expected {
        let stored = reopened.get(reference, &mut read_work)?;
        check!(stored.payload == payload);
        check!(stored.registration.children == children);
    }
    Ok(())
}

#[test]
fn private_257_and_public_65_refuse_before_custody_or_physical_write()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store_path = directory.path().join("store");
    let observer = Arc::new(ObjectIoObservation::default());
    let store = FileObjectStore::open(&store_path)?.with_io_observation(observer.clone());
    let (mut work, ledger) = budget(1_000_000)?;
    let sink = sink(&ledger, false);
    let prepared = store.prepare_object(b"one", &[], &mut work)?;
    let over_private = vec![&prepared; 257];
    check!(matches!(
        store.install_prepared_batch(&over_private, &sink, &mut work),
        Err(IndexError::ResourceLimited)
    ));
    let over_public = vec![
        ObjectBatchInput {
            payload: b"one".to_vec(),
            children: Vec::new(),
        };
        65
    ];
    check!(matches!(
        store.install_batch(&over_public, &sink, &mut work),
        Err(IndexError::ResourceLimited)
    ));
    check!(sink.reserve.load(Ordering::SeqCst) == 0);
    check!(sink.confirm.load(Ordering::SeqCst) == 0);
    let sample = observer.snapshot()?;
    check!(sample.reserve_calls == 0 && sample.write_calls == 0);
    check!(std::fs::read_dir(store_path.join("objects"))?.count() == 0);
    drop(prepared);
    check!(ledger.live.load(Ordering::SeqCst) == 0);
    Ok(())
}

#[test]
fn bounded_digest_index_keeps_original_ordinals_at_zero_and_full_boundaries()
-> Result<(), Box<dyn std::error::Error>> {
    let mut table = BatchIndex::new();
    check!(table.lookup(ObjectRef(Digest([0; 32])))?.is_none());
    table.insert(ObjectRef(Digest([0; 32])), 63)?;
    check!(matches!(
        table.insert(ObjectRef(Digest([0; 32])), 0),
        Err(IndexError::BackendViolation(_))
    ));
    for byte in (1..64_u8).rev() {
        table.insert(ObjectRef(Digest([byte; 32])), usize::from(63 - byte))?;
    }
    for byte in 0..64_u8 {
        check!(table.lookup(ObjectRef(Digest([byte; 32])))? == Some(usize::from(63 - byte)));
    }
    check!(table.lookup(ObjectRef(Digest([64; 32])))?.is_none());
    check!(matches!(
        table.insert(ObjectRef(Digest([64; 32])), 0),
        Err(IndexError::ResourceLimited)
    ));
    Ok(())
}

#[test]
fn fixed_gate_array_keeps_every_gate_through_real_reserve_and_confirmation()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store = Arc::new(FileObjectStore::open(directory.path().join("store"))?);
    let (mut work, ledger) = budget(1000)?;
    let mut sink = sink(&ledger, false);
    sink.gate_store = Some(store.clone());
    let output = store.install_batch(
        &[
            ObjectBatchInput {
                payload: vec![6],
                children: Vec::new(),
            },
            ObjectBatchInput {
                payload: vec![7],
                children: Vec::new(),
            },
            ObjectBatchInput {
                payload: vec![0],
                children: Vec::new(),
            },
        ],
        &sink,
        &mut work,
    )?;
    let first = output.first().ok_or("missing first gate object")?;
    let second = output.get(1).ok_or("missing second gate object")?;
    let third = output.get(2).ok_or("missing third gate object")?;
    check!(
        FileObjectStore::gate_index(first.reference)
            == FileObjectStore::gate_index(second.reference)
    );
    check!(
        FileObjectStore::gate_index(first.reference)
            != FileObjectStore::gate_index(third.reference)
    );
    check!(sink.reserve.load(Ordering::SeqCst) == 1 && sink.confirm.load(Ordering::SeqCst) == 1);
    for object in output {
        let gate = store
            .gates
            .get(FileObjectStore::gate_index(object.reference))
            .ok_or("invalid released gate index")?;
        check!(gate.try_lock().is_ok());
    }
    check!(ledger.live.load(Ordering::SeqCst) == 0);
    Ok(())
}
