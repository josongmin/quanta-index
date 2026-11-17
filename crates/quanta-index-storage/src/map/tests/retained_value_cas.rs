//! A failed CAS must be decided before admission for its replacement value.

use super::*;
use quanta_index_core::index::{
    Digest, MemoryLease, MemoryLeaseOwner, MemoryScope, MemoryScopePort,
};
use std::sync::Arc;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct GrantState {
    denied_bytes: u64,
    attempts: AtomicU64,
    refusals: AtomicU64,
    live: AtomicU64,
}

impl MemoryLeaseOwner for GrantState {
    fn release(&self, bytes: u64) {
        let _previous = self.live.fetch_sub(bytes, Ordering::Relaxed);
    }
}

struct DenyMany(Arc<GrantState>);

impl MemoryScopePort for DenyMany {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self.0.attempts.fetch_add(1, Ordering::Relaxed);
        if lifetime == MemoryLifetime::Temporary && bytes == self.0.denied_bytes {
            let _previous = self.0.refusals.fetch_add(1, Ordering::Relaxed);
            return Err(IndexError::ResourceLimited);
        }
        let _previous = self.0.live.fetch_add(bytes, Ordering::Relaxed);
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

fn deny_many(work: &mut WorkBudget) -> Result<Arc<GrantState>, IndexError> {
    let denied_bytes = u64::try_from(
        MemoryLease::shared_allocation_backing_layout(std::alloc::Layout::new::<
            super::super::retained_value::ManyChildren,
        >())?
        .size(),
    )
    .map_err(|_error| IndexError::ResourceLimited)?;
    let state = Arc::new(GrantState {
        denied_bytes,
        attempts: AtomicU64::new(0),
        refusals: AtomicU64::new(0),
        live: AtomicU64::new(0),
    });
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(DenyMany(Arc::clone(
        &state,
    ))))));
    Ok(state)
}

fn assert_no_effects(directory: &TestDir, sink: &RecordingSink) -> TestResult {
    check_eq!(physical_file_count(directory)?, 0);
    let registrations = sink
        .0
        .lock()
        .map_err(|_error| IndexError::Storage("map test sink poisoned".into()))?;
    check!(registrations.is_empty());
    drop(registrations);
    check_eq!(sink.1.load(Ordering::Relaxed), 0);
    check_eq!(sink.2.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn wrong_cas_precedes_denied_many_child_value_grant() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let sink = RecordingSink::default();
    let children = [
        ObjectRef(Digest([0x31; 32])),
        ObjectRef(Digest([0x32; 32])),
        ObjectRef(Digest([0x33; 32])),
        ObjectRef(Digest([0x34; 32])),
        ObjectRef(Digest([0x35; 32])),
    ];
    let wrong_expected = MapValue::Object(ObjectRef(Digest([0x41; 32])));
    assert_no_effects(&directory, &sink)?;

    for count in [2, 5] {
        let next = MapValue::inline(b"v", children[..count].iter().copied())?;
        check_eq!(next.children()?, &children[..count]);

        let mut wrong_work = budget()?;
        let wrong_grant = deny_many(&mut wrong_work)?;
        let wrong = store.apply(
            MapRoot(None),
            &[MapEdit {
                key: Vec::new(),
                expected: Some(wrong_expected),
                next: Some(next),
            }],
            &sink,
            &mut wrong_work,
        );
        check!(matches!(wrong, Err(IndexError::Conflict)));
        check_eq!(wrong_grant.attempts.load(Ordering::Relaxed), 0);
        check_eq!(wrong_grant.refusals.load(Ordering::Relaxed), 0);
        check_eq!(wrong_grant.live.load(Ordering::Relaxed), 0);
        assert_no_effects(&directory, &sink)?;

        let mut matching_work = budget()?;
        let matching_grant = deny_many(&mut matching_work)?;
        let matching = store.apply(
            MapRoot(None),
            &[MapEdit {
                key: Vec::new(),
                expected: None,
                next: Some(next),
            }],
            &sink,
            &mut matching_work,
        );
        check!(matches!(matching, Err(IndexError::ResourceLimited)));
        check_eq!(matching_grant.attempts.load(Ordering::Relaxed), 1);
        check_eq!(matching_grant.refusals.load(Ordering::Relaxed), 1);
        check_eq!(matching_grant.live.load(Ordering::Relaxed), 0);
        assert_no_effects(&directory, &sink)?;
    }
    Ok(())
}
