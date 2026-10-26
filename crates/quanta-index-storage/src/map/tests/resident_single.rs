//! Resident single-edit results against the durable streamed map contract.

use std::{
    os::unix::fs::DirBuilderExt as _,
    sync::{Arc, atomic::AtomicU64},
};

use quanta_index_core::index::{ScratchAllocation, ScratchRequest, ScratchSpacePort};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

mod pair;

struct CountScratchSpace {
    inner: crate::FileScratchSpace,
    allocations: AtomicU64,
}

impl CountScratchSpace {
    fn allocated(&self) -> u64 {
        self.allocations.load(Ordering::Relaxed)
    }
}

impl ScratchSpacePort for CountScratchSpace {
    fn allocate(
        &self,
        request: ScratchRequest,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
        let allocation = self.inner.allocate(request, budget)?;
        let _prior = self
            .allocations
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                count.checked_add(1)
            })
            .map_err(|_overflow| IndexError::ResourceLimited)?;
        Ok(allocation)
    }
}

fn assert_model(
    store: &FileObjectStore,
    root: MapRoot,
    expected: &BTreeMap<Vec<u8>, ObjectRef>,
    work: &mut WorkBudget,
) -> TestResult {
    for (key, value) in expected {
        check_eq!(store.lookup_object(root, key, work)?, Some(*value));
    }
    let page = store.scan(root, None, 32, work)?;
    check!(!page.has_more);
    check_eq!(
        page.entries,
        expected
            .iter()
            .map(|(key, value)| (key.clone(), (*value).into()))
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn resident_single_edit_matches_stream_and_avoids_scratch_allocation() -> TestResult {
    let directory = TestDir::new();
    fs::DirBuilder::new().mode(0o700).create(&directory.0)?;
    let scratch = Arc::new(CountScratchSpace {
        inner: crate::FileScratchSpace::open_standalone(
            directory.0.join("scratch"),
            16 * 1024 * 1024,
        )?,
        allocations: AtomicU64::new(0),
    });
    let scratch_port: Arc<dyn ScratchSpacePort> = scratch.clone();
    let store = FileObjectStore::open_with_scratch(directory.0.join("objects"), scratch_port)?;
    let mut work = budget()?;
    let old = value(&store, b"resident-old", &mut work)?;
    let new = value(&store, b"resident-new", &mut work)?;
    let sink = BoundedCustodySink::new(old, None);
    {
        let mut state = sink
            .state
            .lock()
            .map_err(|_poisoned| "test sink poisoned")?;
        let _inserted = state.registered.insert(new);
    }

    let baseline = [
        Vec::new(),
        b"a".to_vec(),
        b"ab".to_vec(),
        b"b".to_vec(),
        vec![b'q'; 64],
        vec![b'l'; 4096],
    ]
    .into_iter()
    .map(|key| (key, old))
    .collect::<BTreeMap<_, _>>();
    let mut setup = Vec::new();
    for key in baseline.keys() {
        setup.push(OrdinalMapEdit::unscoped(
            0,
            MapEdit {
                key: key.clone(),
                expected: None,
                next: Some(old.into()),
            },
        ));
    }
    let mut setup_cursor = TestEditCursor(setup.into_iter());
    let base_roots = store.apply_stream(&[MapRoot(None)], &mut setup_cursor, &sink, &mut work)?;
    let base = *base_roots.first().ok_or("base root missing")?;
    assert_model(&store, base, &baseline, &mut work)?;
    let memory = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);

    let cases = [
        ("prefix split", b"aa".to_vec(), None, Some(new)),
        ("update", b"a".to_vec(), Some(old), Some(new)),
        ("delete", b"ab".to_vec(), Some(old), None),
        ("noop", b"b".to_vec(), Some(old), Some(old)),
        ("absent delete", b"missing".to_vec(), None, None),
        ("empty key", Vec::new(), Some(old), Some(new)),
        ("38 byte key", vec![b'z'; 38], None, Some(new)),
        (
            "split long existing prefix",
            vec![b'l'; 38],
            None,
            Some(new),
        ),
        ("64 byte key", vec![b'y'; 64], None, Some(new)),
    ];
    for (label, key, expected, next) in cases {
        let edit = MapEdit {
            key: key.clone(),
            expected: expected.map(Into::into),
            next: next.map(Into::into),
        };
        let before_resident = scratch.allocated();
        let resident = store
            .apply(base, std::slice::from_ref(&edit), &sink, &mut work)?
            .root;
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check_eq!(
            scratch.allocated(),
            before_resident,
            "{label} allocated scratch"
        );

        let mut cursor = TestEditCursor(vec![OrdinalMapEdit::unscoped(0, edit)].into_iter());
        let before_stream = scratch.allocated();
        let streamed = store.apply_stream(&[base], &mut cursor, &sink, &mut work)?;
        check_eq!(
            scratch.allocated(),
            before_stream
                .checked_add(1)
                .ok_or("scratch count overflow")?,
            "{label} stream skipped scratch"
        );
        check_eq!(streamed.first(), Some(&resident), "{label} root differs");
        let mut model = baseline.clone();
        if let Some(next) = next {
            let _prior = model.insert(key, next);
        } else {
            let _prior = model.remove(&key);
        }
        assert_model(&store, resident, &model, &mut work)?;
        assert_model(&store, base, &baseline, &mut work)?;
    }

    for key_len in [65, 4096] {
        let edit = MapEdit {
            key: vec![b'x'; key_len],
            expected: None,
            next: Some(new.into()),
        };
        let before = scratch.allocated();
        let root = store.apply(base, &[edit], &sink, &mut work)?.root;
        check_eq!(
            scratch.allocated(),
            before.checked_add(1).ok_or("scratch count overflow")?,
            "{key_len}-byte key did not use scratch"
        );
        check_eq!(
            store.lookup_object(root, &vec![b'x'; key_len], &mut work)?,
            Some(new)
        );
        assert_model(&store, base, &baseline, &mut work)?;
    }
    let two_edits = [
        MapEdit {
            key: b"m1".to_vec(),
            expected: None,
            next: Some(new.into()),
        },
        MapEdit {
            key: b"m2".to_vec(),
            expected: None,
            next: Some(new.into()),
        },
    ];
    let before = scratch.allocated();
    let pair = store.apply(base, &two_edits, &sink, &mut work)?.root;
    check_eq!(
        scratch.allocated(),
        before.checked_add(1).ok_or("scratch count overflow")?,
        "two edits did not use scratch"
    );
    let mut pair_model = baseline.clone();
    for edit in &two_edits {
        let _prior = pair_model.insert(edit.key.clone(), new);
    }
    assert_model(&store, pair, &pair_model, &mut work)?;
    assert_model(&store, base, &baseline, &mut work)?;
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    assert_clean_scratch(&directory)?;
    Ok(())
}

#[test]
fn resident_prepared_transfers_the_original_wire_and_allocation_custody() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    let object = store.prepare_object(b"original resident wire", &[], &mut work)?;
    let reference = object.registration.reference;
    let pointer = object.wire.as_ptr();
    let original_wire = object.wire.clone();
    let staged = ResidentStage::new();
    check_eq!(staged.insert(object, &mut work)?, reference);
    let live = tracker.live.load(Ordering::Relaxed);
    let before = work.usage();
    let taken = staged
        .take_prepared(reference, &mut work)?
        .ok_or("resident wire missing")?;
    check_eq!(taken.wire.as_ptr(), pointer);
    check_eq!(taken.wire, original_wire);
    check_eq!(work.usage().bytes, before.bytes);
    check_eq!(work.usage().reads, before.reads + 1);
    check_eq!(tracker.live.load(Ordering::Relaxed), live);
    staged.set_mark(reference, 1, &mut work)?;
    check!(matches!(
        staged.take_prepared(reference, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    staged.set_mark(reference, 2, &mut work)?;
    check!(staged.take_prepared(reference, &mut work)?.is_none());
    drop(staged);
    check!(tracker.live.load(Ordering::Relaxed) > 0);
    drop(taken);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn resident_prepared_keeps_cancellation_and_invalid_state_refusals() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let object = store.prepare_object(b"state refusal", &[], &mut work)?;
    let reference = object.registration.reference;
    let staged = ResidentStage::new();
    let _reference = staged.insert(object, &mut work)?;
    staged.set_mark(reference, 3, &mut work)?;
    check!(matches!(
        staged.take_prepared(reference, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    staged.set_mark(reference, 0, &mut work)?;
    work.request().cancel_handle().cancel();
    check!(matches!(
        staged.take_prepared(reference, &mut work),
        Err(IndexError::Cancelled)
    ));
    let mut fresh = budget()?;
    check!(staged.take_prepared(reference, &mut fresh)?.is_some());
    check!(matches!(
        staged.take_prepared(reference, &mut fresh),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}
