//! Independent ordered-map oracle, physical read bounds and refusal/lifetime laws.

use super::*;
use quanta_index_core::index::ports::{MapDiffRange, MapDiffVisitor};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Change = (Vec<u8>, Option<MapValue>, Option<MapValue>);

#[derive(Default)]
struct Collector(Vec<Change>);
impl MapDiffVisitor for Collector {
    fn changed(
        &mut self,
        key: &[u8],
        expected: Option<MapValue>,
        next: Option<MapValue>,
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        // Test-owned output; production visitors must admit their retained copy.
        self.0.push((key.to_vec(), expected, next));
        Ok(())
    }
}

fn root_from_model(
    store: &FileObjectStore,
    model: &BTreeMap<Vec<u8>, MapValue>,
    work: &mut WorkBudget,
) -> Result<MapRoot, IndexError> {
    let edits = model
        .iter()
        .map(|(key, value)| MapEdit {
            key: key.clone(),
            expected: None,
            next: Some(*value),
        })
        .collect::<Vec<_>>();
    Ok(store
        .apply(MapRoot(None), &edits, &RecordingSink::default(), work)?
        .root)
}

fn oracle(
    before: &BTreeMap<Vec<u8>, MapValue>,
    after: &BTreeMap<Vec<u8>, MapValue>,
    prefix: &[u8],
    start: Option<&[u8]>,
) -> Vec<Change> {
    before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| key.starts_with(prefix) && start.is_none_or(|start| key.as_slice() > start))
        .filter_map(|key| {
            let expected = before.get(key).copied();
            let next = after.get(key).copied();
            (expected != next).then(|| (key.clone(), expected, next))
        })
        .collect()
}

#[test]
fn compression_changes_match_ordered_map_oracle_and_exact_paging() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let child = value(&store, b"real-object-value", &mut work)?;
    let first = MapValue::inline(b"first", [child])?;
    let second = MapValue::inline(b"second", [])?;
    let mut before = BTreeMap::from([
        (vec![], first),
        (vec![0], child.into()),
        (vec![0, 0], first),
        (vec![0x10], first),
        (vec![0x11], first),
        (vec![0x11, 0], first),
        (vec![0xfe, 0xff], first),
        (vec![0xff], first),
    ]);
    let _prior = before.insert(vec![0x20; 129], first);
    let mut after = before.clone();
    let _prior = after.remove(&vec![]);
    let _prior = after.remove(&vec![0]);
    let _prior = after.remove(&vec![0x11, 0]);
    let _prior = after.insert(vec![0x10], second);
    let _prior = after.insert(vec![0x10, 0], child.into());
    let _prior = after.insert(vec![0x21; 129], second);
    let _prior = after.insert(vec![0xff, 0], second);
    let old = root_from_model(&store, &before, &mut work)?;
    let new = root_from_model(&store, &after, &mut work)?;
    // Reverse and empty roots exercise terminal/indirect splits and collapses.
    for (old, new, before, after) in [
        (old, new, &before, &after),
        (new, old, &after, &before),
        (MapRoot(None), old, &BTreeMap::new(), &before),
        (new, MapRoot(None), &after, &BTreeMap::new()),
    ] {
        for prefix in [b"".as_slice(), &[0], &[0x10], &[0x11], &[0xff], &[0x80]] {
            for start in [
                None,
                Some(b"".as_slice()),
                Some(&[0x10]),
                Some(&[0xff, 0xff]),
            ] {
                let expected = oracle(before, after, prefix, start);
                for limit in [1_u32, 2, 7, 4096] {
                    let mut position = start.map(<[u8]>::to_vec);
                    let mut actual = Vec::new();
                    loop {
                        let mut page = Collector::default();
                        let more = store.visit_diff(
                            old,
                            new,
                            MapDiffRange {
                                prefix,
                                after: position.as_deref(),
                                limit,
                            },
                            &mut page,
                            &mut work,
                        )?;
                        check!(page.0.len() <= usize::try_from(limit)?);
                        let count = actual
                            .len()
                            .checked_add(page.0.len())
                            .ok_or("count overflow")?;
                        check_eq!(more, count < expected.len());
                        if more {
                            check!(!page.0.is_empty());
                        }
                        position = page.0.last().map(|change| change.0.clone());
                        actual.extend(page.0);
                        if !more {
                            break;
                        }
                    }
                    check_eq!(actual, expected);
                }
            }
        }
    }
    Ok(())
}

#[test]
fn one_delta_skips_shared_subtrees_and_never_writes() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut build = budget()?;
    let first = MapValue::inline(b"before", [])?;
    let second = MapValue::inline(b"after", [])?;
    let model = (0_u16..4096)
        .map(|key| (key.to_be_bytes().to_vec(), first))
        .collect();
    let before = root_from_model(&store, &model, &mut build)?;
    let key = 2815_u16.to_be_bytes();
    let after = store
        .apply(
            before,
            &[MapEdit {
                key: key.to_vec(),
                expected: Some(first),
                next: Some(second),
            }],
            &RecordingSink::default(),
            &mut build,
        )?
        .root;
    let files = physical_file_count(&directory)?;
    drop(store);
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let mut output = Collector::default();
    let more = store.visit_diff(
        before,
        after,
        MapDiffRange {
            prefix: &[],
            after: None,
            limit: 1,
        },
        &mut output,
        &mut work,
    )?;
    check!(!more);
    check_eq!(output.0, vec![(key.to_vec(), Some(first), Some(second))]);
    // Fixed read-count invariant, not a wall-time benchmark or ANN acceptance.
    check!(
        work.usage().reads > 0 && work.usage().reads <= 32,
        "one COW delta read {:?}",
        work.usage()
    );
    check_eq!(work.usage().writes, 0);
    check_eq!(work.usage().mutations, 0);
    check_eq!(physical_file_count(&directory)?, files);
    let prior = work.usage();
    check!(!store.visit_diff(
        before,
        before,
        MapDiffRange {
            prefix: &[],
            after: None,
            limit: 1,
        },
        &mut output,
        &mut work
    )?);
    check_eq!(work.usage(), prior);
    Ok(())
}

#[test]
fn maximum_key_and_prefix_continuation_are_iterative() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut build = budget()?;
    let key = vec![0x42; MAX_KEY_BYTES];
    let first = MapValue::inline(b"old", [])?;
    let second = MapValue::inline(b"new", [])?;
    let before = root_from_model(&store, &BTreeMap::from([(key.clone(), first)]), &mut build)?;
    let after = root_from_model(&store, &BTreeMap::from([(key.clone(), second)]), &mut build)?;
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    let mut output = Collector::default();
    check!(!store.visit_diff(
        before,
        after,
        MapDiffRange {
            prefix: &key,
            after: None,
            limit: 1,
        },
        &mut output,
        &mut work
    )?);
    check_eq!(output.0, vec![(key.clone(), Some(first), Some(second))]);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    check_eq!(tracker.refused.load(Ordering::Relaxed), 0);
    let mut exhausted = Collector::default();
    check!(!store.visit_diff(
        before,
        after,
        MapDiffRange {
            prefix: &key,
            after: Some(&key),
            limit: 1,
        },
        &mut exhausted,
        &mut work
    )?);
    check!(exhausted.0.is_empty());
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

struct RefusingVisitor;
impl MapDiffVisitor for RefusingVisitor {
    fn changed(
        &mut self,
        _: &[u8],
        _: Option<MapValue>,
        _: Option<MapValue>,
        _: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        Err(IndexError::FenceLost)
    }
}

#[test]
fn refusals_cancel_and_callback_failure_never_seal_a_prefix_or_leak() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut build = budget()?;
    let after = root_from_model(
        &store,
        &BTreeMap::from([
            (vec![], MapValue::inline(b"empty-key", [])?),
            (vec![1], MapValue::inline(b"other-key", [])?),
        ]),
        &mut build,
    )?;
    let range = MapDiffRange {
        prefix: &[],
        after: None,
        limit: 1,
    };
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    check_eq!(
        store.visit_diff(MapRoot(None), after, range, &mut RefusingVisitor, &mut work),
        Err(IndexError::FenceLost)
    );
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);

    let mut refused = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, None, &mut refused);
    check_eq!(
        store.visit_diff(
            MapRoot(None),
            after,
            range,
            &mut RefusingVisitor,
            &mut refused
        ),
        Err(IndexError::ResourceLimited)
    );
    check_eq!(refused.usage().reads, 0);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);

    let mut denied = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 1_000_000,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("deadline overflow")?,
    );
    check_eq!(
        store.visit_diff(
            MapRoot(None),
            after,
            range,
            &mut RefusingVisitor,
            &mut denied
        ),
        Err(IndexError::ResourceLimited)
    );
    let mut cancelled = budget()?;
    cancelled.request().cancel_handle().cancel();
    check_eq!(
        store.visit_diff(after, after, range, &mut RefusingVisitor, &mut cancelled),
        Err(IndexError::Cancelled)
    );
    check_eq!(cancelled.usage(), WorkUsage::default());
    Ok(())
}

#[test]
fn invalid_ranges_refuse_even_equal_roots_before_io() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let oversized = vec![0; MAX_KEY_BYTES.checked_add(1).ok_or("key overflow")?];
    for range in [
        MapDiffRange {
            prefix: &[],
            after: None,
            limit: 0,
        },
        MapDiffRange {
            prefix: &[],
            after: None,
            limit: 4097,
        },
        MapDiffRange {
            prefix: &oversized,
            after: None,
            limit: 1,
        },
        MapDiffRange {
            prefix: &[],
            after: Some(&oversized),
            limit: 1,
        },
    ] {
        let mut work = budget()?;
        check!(matches!(
            store.visit_diff(
                MapRoot(None),
                MapRoot(None),
                range,
                &mut RefusingVisitor,
                &mut work
            ),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(work.usage(), WorkUsage::default());
    }
    Ok(())
}

#[test]
fn authenticated_malformed_node_is_not_accepted_as_a_diff_page() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut build = budget()?;
    let child = value(&store, b"real-child", &mut build)?;
    // Valid physical object and map encoding, invalid value at an odd nibble.
    let mut wire = NODE_MAGIC.to_vec();
    wire.extend_from_slice(&1_u16.to_be_bytes());
    wire.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0]);
    let malformed = store.put(&wire, &[child], &mut build)?.reference;
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    let mut output = Collector::default();
    check!(matches!(
        store.visit_diff(
            MapRoot(None),
            MapRoot(Some(malformed)),
            MapDiffRange {
                prefix: &[],
                after: None,
                limit: 1
            },
            &mut output,
            &mut work
        ),
        Err(IndexError::CorruptObject(_))
    ));
    check!(output.0.is_empty());
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn cancellation_and_unwind_inside_visitor_release_all_temporary_backing() -> TestResult {
    struct CancelOnVisit;
    impl MapDiffVisitor for CancelOnVisit {
        fn changed(
            &mut self,
            _: &[u8],
            _: Option<MapValue>,
            _: Option<MapValue>,
            budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            budget.request().cancel_handle().cancel();
            Ok(())
        }
    }
    struct UnwindOnVisit;
    impl MapDiffVisitor for UnwindOnVisit {
        fn changed(
            &mut self,
            _: &[u8],
            _: Option<MapValue>,
            _: Option<MapValue>,
            _: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            std::panic::resume_unwind(Box::new(()))
        }
    }
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut build = budget()?;
    let after = root_from_model(
        &store,
        &BTreeMap::from([(vec![0x12], MapValue::inline(b"value", [])?)]),
        &mut build,
    )?;
    let range = MapDiffRange {
        prefix: &[],
        after: None,
        limit: 1,
    };
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    check_eq!(
        store.visit_diff(MapRoot(None), after, range, &mut CancelOnVisit, &mut work),
        Err(IndexError::Cancelled)
    );
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store.visit_diff(MapRoot(None), after, range, &mut UnwindOnVisit, &mut work)
    }));
    check!(result.is_err());
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn corruption_after_a_full_page_is_failure_instead_of_has_more() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut build = budget()?;
    let value = value(&store, b"real-value", &mut build)?;
    let bad_child = store.put(b"not-a-map", &[], &mut build)?.reference;
    // Canonical physical root with an empty-key own value and one indirect
    // branch. The branch is physically authenticated but not a map node.
    let mut wire = NODE_MAGIC.to_vec();
    wire.extend_from_slice(&0_u16.to_be_bytes());
    wire.extend_from_slice(&[1, 0, 1, 0, 0, 0]);
    let root = store.put(&wire, &[value, bad_child], &mut build)?.reference;
    let mut work = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    let mut output = Collector::default();
    check!(matches!(
        store.visit_diff(
            MapRoot(None),
            MapRoot(Some(root)),
            MapDiffRange {
                prefix: &[],
                after: None,
                limit: 1
            },
            &mut output,
            &mut work,
        ),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(output.0, vec![(vec![], None, Some(value.into()))]);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}
