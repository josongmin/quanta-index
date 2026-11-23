//! Producer-selected packed subtrees must preserve ordinary map semantics.

use super::*;
use quanta_index_core::index::ports::{MapDiffRange, MapDiffVisitor, RootPackingMode};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct HintCursor {
    edits: std::vec::IntoIter<OrdinalMapEdit>,
    selected: Vec<u32>,
    mode: RootPackingMode,
}

impl MapEditCursor for HintCursor {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        Ok(self.edits.next())
    }

    fn subtree_packing(
        &self,
    ) -> Result<quanta_index_core::index::ports::RootPackingPlan, IndexError> {
        let mut packing = quanta_index_core::index::ports::RootPackingPlan::default();
        for ordinal in &self.selected {
            packing.select(*ordinal, self.mode)?;
        }
        Ok(packing)
    }
}

struct InvalidHintCursor;

impl MapEditCursor for InvalidHintCursor {
    fn next_edit(
        &mut self,
        _budget: &mut WorkBudget,
    ) -> Result<Option<OrdinalMapEdit>, IndexError> {
        Err(IndexError::BackendViolation(
            "invalid selection consumed the cursor".into(),
        ))
    }

    fn subtree_packing(
        &self,
    ) -> Result<quanta_index_core::index::ports::RootPackingPlan, IndexError> {
        let mut packing = quanta_index_core::index::ports::RootPackingPlan::default();
        packing.select(
            2,
            quanta_index_core::index::ports::RootPackingMode::Terminal,
        )?;
        Ok(packing)
    }
}

#[derive(Default)]
struct Changes(Vec<(Vec<u8>, Option<MapValue>, Option<MapValue>)>);

impl MapDiffVisitor for Changes {
    fn changed(
        &mut self,
        key: &[u8],
        expected: Option<MapValue>,
        next: Option<MapValue>,
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.0.push((key.to_vec(), expected, next));
        Ok(())
    }
}

fn scan_all(
    store: &FileObjectStore,
    root: MapRoot,
    work: &mut WorkBudget,
) -> Result<Vec<(Vec<u8>, MapValue)>, IndexError> {
    let mut entries = Vec::new();
    let mut after = None;
    loop {
        let page = store.scan(root, after.as_deref(), 7, work)?;
        after = page.entries.last().map(|(key, _)| key.clone());
        entries.extend(page.entries.iter().cloned());
        if !page.has_more {
            return Ok(entries);
        }
    }
}

fn make_roots(
    store: &FileObjectStore,
    directory: &TestDir,
    selected: Option<u32>,
    branch_count: u8,
    work: &mut WorkBudget,
) -> Result<([MapRoot; 2], BTreeMap<Vec<u8>, MapValue>, usize), Box<dyn std::error::Error>> {
    let owner_value = MapValue::from(value(store, b"packed subtree owner value", work)?);
    let mut first = BTreeMap::new();
    let mut edits = Vec::new();
    for branch in 0_u8..branch_count {
        // Distinguish child subtrees before the content-addressed install.
        let scalar = MapValue::from(value(store, &[b'p', branch], work)?);
        for leaf in 0_u8..8 {
            let key = vec![branch << 4, leaf << 4];
            let _previous = first.insert(key.clone(), scalar);
            edits.push(OrdinalMapEdit::unscoped(
                0,
                MapEdit {
                    key,
                    expected: None,
                    next: Some(scalar),
                },
            )?);
        }
    }
    for key in [vec![0xf0], vec![0xf1], vec![0xf2]] {
        edits.push(OrdinalMapEdit::unscoped(
            1,
            MapEdit {
                key,
                expected: None,
                next: Some(owner_value),
            },
        )?);
    }
    let before = physical_file_count(directory)?;
    let mut cursor = HintCursor {
        edits: edits.into_iter(),
        selected: selected.into_iter().collect(),
        mode: RootPackingMode::Terminal,
    };
    let roots = store.apply_stream(
        &[MapRoot(None), MapRoot(None)],
        &mut cursor,
        &RecordingSink::default(),
        work,
    )?;
    check_eq!(roots.len(), 2);
    let first_root = *roots.first().ok_or("first root missing")?;
    let second_root = *roots.get(1).ok_or("second root missing")?;
    let installs = physical_file_count(directory)? - before;
    Ok(([first_root, second_root], first, installs))
}

#[test]
fn invalid_selected_root_is_refused_before_cursor_consumption() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    check!(matches!(
        store.apply_stream(
            &[MapRoot(None), MapRoot(None)],
            &mut InvalidHintCursor,
            &RecordingSink::default(),
            &mut work,
        ),
        Err(IndexError::InvalidInput(_))
    ));
    Ok(())
}

#[test]
fn packed_wire_rejects_invalid_mask_body_and_nested_version() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let (roots, _model, _installs) = make_roots(&store, &directory, Some(0), 8, &mut work)?;
    let stored = store.get(roots[0].0.ok_or("packed root missing")?, &mut work)?;
    check_eq!(stored.payload.get(..8), Some(b"QIMAP005".as_slice()));
    let empty_prefix = [0_u8, 0];
    check_eq!(stored.payload.get(8..10), Some(empty_prefix.as_slice()));
    let mut empty_mask = stored.payload.clone();
    empty_mask[15..17].copy_from_slice(&0_u16.to_be_bytes());
    let mut overlapping_mask = stored.payload.clone();
    overlapping_mask[13..15].copy_from_slice(&stored.payload[15..17]);
    let mut empty_body = stored.payload.clone();
    empty_body[17..19].copy_from_slice(&0_u16.to_be_bytes());
    let mut nested_version = stored.payload.clone();
    nested_version[19..27].copy_from_slice(b"QIMAP005");
    let mut long_body = stored.payload.clone();
    let body_length = u16::from_be_bytes(long_body[17..19].try_into()?);
    long_body[17..19].copy_from_slice(
        &body_length
            .checked_add(1)
            .ok_or("body length")?
            .to_be_bytes(),
    );
    for payload in [
        empty_mask,
        overlapping_mask,
        empty_body,
        nested_version,
        long_body,
    ] {
        let malformed = store.put(&payload, &stored.registration.children, &mut work)?;
        check!(matches!(
            store.read_node(malformed.reference, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    let mut edges = stored.registration.children.clone();
    let last = edges.pop().ok_or("packed fixture has no edge")?;
    for changed_edges in [edges, {
        let mut trailing = stored.registration.children.clone();
        trailing.push(last);
        trailing
    }] {
        let malformed = store.put(&stored.payload, &changed_edges, &mut work)?;
        check!(matches!(
            store.read_node(malformed.reference, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    Ok(())
}

#[test]
fn selected_root_spills_after_physical_child_limit() -> TestResult {
    let plain_directory = TestDir::new();
    let plain_store = FileObjectStore::open(&plain_directory.0)?;
    let mut plain_work = budget()?;
    let (_plain, expected, plain_installs) =
        make_roots(&plain_store, &plain_directory, None, 16, &mut plain_work)?;

    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut work = budget()?;
    let (roots, model, packed_installs) = make_roots(&store, &directory, Some(0), 16, &mut work)?;
    check_eq!(model, expected);
    check!(packed_installs < plain_installs);
    let stored = store.get(roots[0].0.ok_or("packed root missing")?, &mut work)?;
    check_eq!(stored.payload.get(..8), Some(b"QIMAP005".as_slice()));
    // Sixteen physical child edges plus seven more for each embedded child:
    // nine embed, seven spill; embedding a tenth would exceed the 85-edge cap.
    check_eq!(stored.registration.children.len(), 79);
    let node = store.read_node(roots[0].0.ok_or("packed root missing")?, &mut work)?;
    let embedded = node
        .branches
        .iter()
        .filter_map(|(edge, branch)| {
            matches!(&branch.value, BranchValue::Packed(_)).then_some(edge)
        })
        .collect::<Vec<_>>();
    check_eq!(embedded, (0_usize..9).collect::<Vec<_>>());
    let spilled = node
        .branches
        .iter()
        .filter_map(|(edge, branch)| {
            matches!(&branch.value, BranchValue::Indirect(_)).then_some(edge)
        })
        .collect::<Vec<_>>();
    check_eq!(spilled, (9_usize..16).collect::<Vec<_>>());
    for (key, value) in &expected {
        check_eq!(store.lookup(roots[0], key, &mut work)?, Some(*value));
    }
    check_eq!(
        scan_all(&store, roots[0], &mut work)?,
        expected
            .iter()
            .map(|(key, value)| (key.clone(), *value))
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn multiple_selected_roots_reduce_physical_nodes_and_reopen_with_same_entries() -> TestResult {
    let mut counts = Vec::new();
    for selected in [Vec::new(), vec![0, 1]] {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let sink = RecordingSink::default();
        let mut work = budget()?;
        let values = [
            MapValue::from(value(&store, b"first root value", &mut work)?),
            MapValue::from(value(&store, b"second root value", &mut work)?),
        ];
        let mut edits = Vec::new();
        let mut expected = [BTreeMap::new(), BTreeMap::new()];
        for root in 0u32..2 {
            let value = *values.get(usize::try_from(root)?).ok_or("value missing")?;
            for branch in 0u8..8 {
                for leaf in 0u8..8 {
                    let key = vec![branch << 4, leaf << 4];
                    let _old = expected
                        .get_mut(usize::try_from(root)?)
                        .ok_or("model missing")?
                        .insert(key.clone(), value);
                    edits.push(OrdinalMapEdit::unscoped(
                        root,
                        MapEdit {
                            key,
                            expected: None,
                            next: Some(value),
                        },
                    )?);
                }
            }
        }
        let before = physical_file_count(&directory)?;
        let mut cursor = HintCursor {
            edits: edits.into_iter(),
            selected,
            mode: RootPackingMode::Terminal,
        };
        let output = store.apply_stream(
            &[MapRoot(None), MapRoot(None)],
            &mut cursor,
            &sink,
            &mut work,
        )?;
        let roots = [
            *output.first().ok_or("first root missing")?,
            *output.get(1).ok_or("second root missing")?,
        ];
        counts.push(physical_file_count(&directory)? - before);
        drop(store);
        let reopened = FileObjectStore::open(&directory.0)?;
        for (root, model) in roots.into_iter().zip(expected) {
            let entries = model.into_iter().collect::<Vec<_>>();
            check_eq!(scan_all(&reopened, root, &mut work)?, entries);
        }
    }
    check!(
        counts.get(1).ok_or("packed count missing")?
            < counts.first().ok_or("plain count missing")?
    );
    Ok(())
}

#[test]
fn selected_root_packs_terminal_subtrees_without_changing_map_laws() -> TestResult {
    let plain_directory = TestDir::new();
    let plain_store = FileObjectStore::open(&plain_directory.0)?;
    let mut plain_work = budget()?;
    let (plain, expected, plain_installs) =
        make_roots(&plain_store, &plain_directory, None, 8, &mut plain_work)?;

    let packed_directory = TestDir::new();
    let packed_store = FileObjectStore::open(&packed_directory.0)?;
    let mut packed_work = budget()?;
    let (packed, packed_expected, packed_installs) = make_roots(
        &packed_store,
        &packed_directory,
        Some(0),
        8,
        &mut packed_work,
    )?;
    check_eq!(packed_expected, expected);
    check!(packed_installs < plain_installs);
    let selected_object =
        packed_store.get(packed[0].0.ok_or("packed root missing")?, &mut packed_work)?;
    let ordinary_object =
        packed_store.get(packed[1].0.ok_or("second root missing")?, &mut packed_work)?;
    check_eq!(
        selected_object.payload.get(..8),
        Some(b"QIMAP005".as_slice())
    );
    check_eq!(
        ordinary_object.payload.get(..8),
        Some(b"QIMAP004".as_slice())
    );

    let expected_entries = expected
        .iter()
        .map(|(key, value)| (key.clone(), *value))
        .collect::<Vec<_>>();
    check_eq!(
        scan_all(&plain_store, plain[0], &mut plain_work)?,
        expected_entries
    );
    check_eq!(
        scan_all(&packed_store, packed[0], &mut packed_work)?,
        expected_entries
    );
    for (key, value) in &expected {
        check_eq!(
            packed_store.lookup(packed[0], key, &mut packed_work)?,
            Some(*value)
        );
    }
    let keys = [
        b"\x70\x70".as_slice(),
        b"\x00\x01".as_slice(),
        b"\x00\x00".as_slice(),
        b"\x70\x70".as_slice(),
    ];
    let mut batch = budget()?;
    let mut values = [None; 4];
    packed_store.lookup_batch(packed[0], &keys, &mut values, &mut batch)?;
    check_eq!(values, keys.map(|key| expected.get(key).copied()));
    let mut scalar = budget()?;
    for key in keys {
        check_eq!(
            packed_store.lookup(packed[0], key, &mut scalar)?,
            expected.get(key).copied()
        );
    }
    check!(batch.usage().reads < scalar.usage().reads);
    check_eq!(
        scan_all(&plain_store, plain[1], &mut plain_work)?,
        scan_all(&packed_store, packed[1], &mut packed_work)?
    );

    let replaced = value(&packed_store, b"replacement", &mut packed_work)?;
    let replacement = MapValue::from(replaced);
    let removed_key = vec![0x00, 0x00];
    let replaced_key = vec![0x10, 0x10];
    let inserted_key = vec![0x20, 0x08];
    let prior = *expected.get(&removed_key).ok_or("removed value missing")?;
    let replaced_prior = *expected
        .get(&replaced_key)
        .ok_or("replaced value missing")?;
    let before_refusal = physical_file_count(&packed_directory)?;
    let mut stale = HintCursor {
        selected: vec![0],
        mode: RootPackingMode::Terminal,
        edits: vec![OrdinalMapEdit::unscoped(
            0,
            MapEdit {
                key: removed_key.clone(),
                expected: None,
                next: Some(replacement),
            },
        )?]
        .into_iter(),
    };
    check!(matches!(
        packed_store.apply_stream(
            &packed[..1],
            &mut stale,
            &RecordingSink::default(),
            &mut packed_work,
        ),
        Err(IndexError::Conflict)
    ));
    check_eq!(physical_file_count(&packed_directory)?, before_refusal);
    let mut cursor = HintCursor {
        selected: vec![0],
        mode: RootPackingMode::Terminal,
        edits: [
            MapEdit {
                key: removed_key.clone(),
                expected: Some(prior),
                next: None,
            },
            MapEdit {
                key: replaced_key.clone(),
                expected: Some(replaced_prior),
                next: Some(replacement),
            },
            MapEdit {
                key: inserted_key.clone(),
                expected: None,
                next: Some(replacement),
            },
        ]
        .into_iter()
        .map(|edit| OrdinalMapEdit::unscoped(0, edit))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter(),
    };
    let changed = *packed_store
        .apply_stream(
            &packed[..1],
            &mut cursor,
            &RecordingSink::default(),
            &mut packed_work,
        )?
        .first()
        .ok_or("warm packed root missing")?;
    let mut changed_expected = expected.clone();
    let _previous = changed_expected.remove(&removed_key);
    let _previous = changed_expected.insert(replaced_key.clone(), replacement);
    let _previous = changed_expected.insert(inserted_key.clone(), replacement);
    let changed_entries = changed_expected
        .iter()
        .map(|(key, value)| (key.clone(), *value))
        .collect::<Vec<_>>();
    check_eq!(
        scan_all(&packed_store, changed, &mut packed_work)?,
        changed_entries
    );
    let mut changes = Changes::default();
    check!(!packed_store.visit_diff(
        packed[0],
        changed,
        MapDiffRange {
            prefix: b"",
            after: None,
            limit: 16
        },
        &mut changes,
        &mut packed_work,
    )?);
    check_eq!(
        changes.0,
        vec![
            (removed_key, Some(prior), None),
            (replaced_key, Some(replaced_prior), Some(replacement)),
            (inserted_key, None, Some(replacement)),
        ]
    );
    drop(packed_store);
    let reopened = FileObjectStore::open(&packed_directory.0)?;
    check_eq!(
        scan_all(&reopened, packed[0], &mut packed_work)?,
        expected_entries
    );
    check_eq!(
        scan_all(&reopened, changed, &mut packed_work)?,
        changed_entries
    );
    check_eq!(
        scan_all(&reopened, packed[1], &mut packed_work)?,
        scan_all(&plain_store, plain[1], &mut plain_work)?
    );
    Ok(())
}

fn hierarchical_root(
    store: &FileObjectStore,
    directory: &TestDir,
    mode: RootPackingMode,
    work: &mut WorkBudget,
) -> Result<(MapRoot, BTreeMap<Vec<u8>, MapValue>, usize), Box<dyn std::error::Error>> {
    let scalar = MapValue::from(value(store, b"nested map scalar", work)?);
    let mut model = BTreeMap::new();
    let mut edits = Vec::new();
    for first in 0u8..4 {
        for second in 0u8..4 {
            for third in 0u8..4 {
                let key = vec![first << 4, second << 4, third << 4];
                let _prior = model.insert(key.clone(), scalar);
                edits.push(OrdinalMapEdit::unscoped(
                    0,
                    MapEdit {
                        key,
                        expected: None,
                        next: Some(scalar),
                    },
                )?);
            }
        }
    }
    let before = physical_file_count(directory)?;
    let mut cursor = HintCursor {
        edits: edits.into_iter(),
        selected: vec![0],
        mode,
    };
    let roots = store.apply_stream(
        &[MapRoot(None)],
        &mut cursor,
        &RecordingSink::default(),
        work,
    )?;
    let root = *roots.first().ok_or("nested map root missing")?;
    Ok((root, model, physical_file_count(directory)? - before))
}

#[test]
fn nested_mode_reduces_physical_nodes_and_preserves_lookup_diff_and_reopen() -> TestResult {
    let plain_dir = TestDir::new();
    let plain_store = FileObjectStore::open(&plain_dir.0)?;
    let mut plain_work = budget()?;
    let (_plain, expected, terminal_count) = hierarchical_root(
        &plain_store,
        &plain_dir,
        RootPackingMode::Terminal,
        &mut plain_work,
    )?;

    let dir = TestDir::new();
    let store = FileObjectStore::open(&dir.0)?;
    let mut work = budget()?;
    let (root, model, nested_count) =
        hierarchical_root(&store, &dir, RootPackingMode::Nested, &mut work)?;
    check_eq!(model, expected);
    check!(nested_count < terminal_count);
    let stored = store.get(root.0.ok_or("nested root missing")?, &mut work)?;
    check_eq!(stored.payload.get(..8), Some(b"QIMAP006".as_slice()));
    let expected_rows = model
        .iter()
        .map(|(key, value)| (key.clone(), *value))
        .collect::<Vec<_>>();
    check_eq!(scan_all(&store, root, &mut work)?, expected_rows);
    for (key, value) in &model {
        check_eq!(store.lookup(root, key, &mut work)?, Some(*value));
    }
    let keys = [b"\x00\x00\x00".as_slice(), b"\x30\x30\x30", b"\x40\x00\x00"];
    let mut results = [None; 3];
    store.lookup_batch(root, &keys, &mut results, &mut work)?;
    check_eq!(results, keys.map(|key| model.get(key).copied()));

    let removed_key = vec![0x10, 0x10, 0x10];
    let prior = *model.get(&removed_key).ok_or("nested prior missing")?;
    let mut cursor = HintCursor {
        edits: vec![OrdinalMapEdit::unscoped(
            0,
            MapEdit {
                key: removed_key.clone(),
                expected: Some(prior),
                next: None,
            },
        )?]
        .into_iter(),
        selected: vec![0],
        mode: RootPackingMode::Nested,
    };
    let changed = *store
        .apply_stream(&[root], &mut cursor, &RecordingSink::default(), &mut work)?
        .first()
        .ok_or("changed nested root missing")?;
    let mut changes = Changes::default();
    check!(!store.visit_diff(
        root,
        changed,
        MapDiffRange {
            prefix: b"",
            after: None,
            limit: 16,
        },
        &mut changes,
        &mut work
    )?);
    check_eq!(changes.0, vec![(removed_key.clone(), Some(prior), None)]);
    let mut changed_model = model.clone();
    let _removed = changed_model.remove(&removed_key);
    let changed_rows: Vec<_> = changed_model.into_iter().collect();
    check_eq!(scan_all(&store, changed, &mut work)?, changed_rows);
    // Existing nested roots must also accept the ordinary scalar COW path
    // after cold publications stop selecting nested packing.
    let scalar = store.apply(
        root,
        &[MapEdit {
            key: removed_key.clone(),
            expected: Some(prior),
            next: None,
        }],
        &RecordingSink::default(),
        &mut work,
    )?;
    check_eq!(store.lookup(root, &removed_key, &mut work)?, Some(prior));
    check_eq!(store.lookup(scalar.root, &removed_key, &mut work)?, None);
    let mut scalar_changes = Changes::default();
    check!(!store.visit_diff(
        root,
        scalar.root,
        MapDiffRange {
            prefix: b"",
            after: None,
            limit: 16,
        },
        &mut scalar_changes,
        &mut work
    )?);
    check_eq!(
        scalar_changes.0,
        vec![(removed_key.clone(), Some(prior), None)]
    );
    check_eq!(scan_all(&store, scalar.root, &mut work)?, changed_rows);
    // This established fixture has eight small terminal subtrees, so its
    // selected root is guaranteed to fit the QIMAP005 physical child bound.
    // The larger hierarchy above can legitimately spill its selected root as
    // QIMAP004; that is not a legacy-reader oracle.
    let (mixed_roots, mixed_model, _) = make_roots(&store, &dir, Some(0), 8, &mut work)?;
    let legacy_root = mixed_roots[0];
    let ordinary_root = mixed_roots[1];
    let legacy_rows = mixed_model.into_iter().collect::<Vec<_>>();
    let ordinary_rows = scan_all(&store, ordinary_root, &mut work)?;
    for (other, magic) in [
        (legacy_root, b"QIMAP005".as_slice()),
        (ordinary_root, b"QIMAP004".as_slice()),
    ] {
        let object = store.get(other.0.ok_or("mixed map root missing")?, &mut work)?;
        check_eq!(object.payload.get(..8), Some(magic));
        let actual = scan_all(&store, other, &mut work)?;
        let expected = if other == legacy_root {
            legacy_rows.as_slice()
        } else {
            ordinary_rows.as_slice()
        };
        check_eq!(actual.as_slice(), expected);
    }
    drop(store);
    let reopened = FileObjectStore::open(&dir.0)?;
    check_eq!(scan_all(&reopened, root, &mut work)?, expected_rows);
    check_eq!(reopened.lookup(changed, &removed_key, &mut work)?, None);
    check_eq!(reopened.lookup(scalar.root, &removed_key, &mut work)?, None);
    check_eq!(scan_all(&reopened, scalar.root, &mut work)?, changed_rows);
    check_eq!(scan_all(&reopened, legacy_root, &mut work)?, legacy_rows);
    check_eq!(
        scan_all(&reopened, ordinary_root, &mut work)?,
        ordinary_rows
    );
    Ok(())
}

#[test]
fn cold_nested_path_postings_preserve_repeated_object_children_after_reopen() -> TestResult {
    let build = |mode: RootPackingMode| {
        let directory = TestDir::new();
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        let mut rows = Vec::new();
        for owner in 0u8..8 {
            let source = value(&store, &[b's', owner], &mut work)?;
            for term in b'a'..=b'h' {
                // [term length][case term][owner], as in TreeV1 path postings.
                let mut key = vec![0, 3, 0, term, b'x'];
                let mut owner_key = [0u8; 32];
                owner_key[31] = owner;
                key.extend_from_slice(&owner_key);
                rows.push((key, MapValue::Object(source)));
            }
        }
        rows.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        let edits = rows
            .iter()
            .map(|(key, value)| {
                OrdinalMapEdit::unscoped(
                    0,
                    MapEdit {
                        key: key.clone(),
                        expected: None,
                        next: Some(*value),
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let before = physical_file_count(&directory)?;
        let mut cursor = HintCursor {
            edits: edits.into_iter(),
            selected: vec![0],
            mode,
        };
        let roots = store.apply_stream(
            &[MapRoot(None)],
            &mut cursor,
            &RecordingSink::default(),
            &mut work,
        )?;
        let root = *roots.first().ok_or("path posting root missing")?;
        let physical_nodes = physical_file_count(&directory)?
            .checked_sub(before)
            .ok_or("path map object count shrank")?;
        check_eq!(scan_all(&store, root, &mut work)?, rows);
        Ok::<_, Box<dyn std::error::Error>>((directory, root, rows, physical_nodes))
    };

    let (plain_dir, plain_root, expected, plain_nodes) = build(RootPackingMode::None)?;
    let (packed_dir, packed_root, packed_rows, packed_nodes) = build(RootPackingMode::Nested)?;
    check_eq!(packed_rows, expected);
    check!(packed_nodes < plain_nodes);
    for (directory, root) in [(plain_dir, plain_root), (packed_dir, packed_root)] {
        let store = FileObjectStore::open(&directory.0)?;
        let mut work = budget()?;
        check_eq!(scan_all(&store, root, &mut work)?, expected);
        for (key, value) in &expected {
            let source = *value.as_object_ref()?;
            check_eq!(store.lookup_object(root, key, &mut work)?, Some(source));
            let object = store.get(source, &mut work)?;
            check_eq!(
                object.payload.as_slice(),
                &[b's', *key.last().ok_or("owner byte missing")?]
            );
        }
        let ((key, value), remaining) = expected.split_first().ok_or("path postings missing")?;
        let changed = store.apply(
            root,
            &[MapEdit {
                key: key.clone(),
                expected: Some(*value),
                next: None,
            }],
            &RecordingSink::default(),
            &mut work,
        )?;
        let remaining_rows = scan_all(&store, changed.root, &mut work)?;
        check_eq!(remaining_rows.as_slice(), remaining);
        check_eq!(scan_all(&store, root, &mut work)?, expected);
    }
    Ok(())
}

#[test]
fn nested_wire_refuses_bad_child_version_length_and_edges() -> TestResult {
    let dir = TestDir::new();
    let store = FileObjectStore::open(&dir.0)?;
    let mut work = budget()?;
    let (root, _, _) = hierarchical_root(&store, &dir, RootPackingMode::Nested, &mut work)?;
    let stored = store.get(root.0.ok_or("nested root missing")?, &mut work)?;
    let child_at = stored
        .payload
        .windows(8)
        .enumerate()
        .skip(1)
        .find_map(|(at, bytes)| (bytes == b"QIMAP006").then_some(at))
        .ok_or("nested child body missing")?;
    let mut wrong_version = stored.payload.clone();
    wrong_version[child_at..child_at + 8].copy_from_slice(b"QIMAP005");
    let mut zero_length = stored.payload.clone();
    zero_length[child_at - 2..child_at].copy_from_slice(&0u16.to_be_bytes());
    let mut overlong = stored.payload.clone();
    let declared = u16::from_be_bytes(overlong[child_at - 2..child_at].try_into()?);
    overlong[child_at - 2..child_at].copy_from_slice(
        &declared
            .checked_add(1)
            .ok_or("nested body length overflow")?
            .to_be_bytes(),
    );
    for payload in [wrong_version, zero_length, overlong] {
        let bad = store.put(&payload, &stored.registration.children, &mut work)?;
        check!(matches!(
            store.read_node(bad.reference, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    let mut short_edges = stored.registration.children.clone();
    let edge = short_edges.pop().ok_or("nested edge missing")?;
    let bad = store.put(&stored.payload, &short_edges, &mut work)?;
    check!(matches!(
        store.read_node(bad.reference, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let mut long_edges = stored.registration.children.clone();
    long_edges.push(edge);
    let bad = store.put(&stored.payload, &long_edges, &mut work)?;
    check!(matches!(
        store.read_node(bad.reference, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

fn nested_inline_wire(wrappers: usize, invalid_nibble: bool) -> Result<Vec<u8>, IndexError> {
    // One canonical inline value at the node and its terminal branch means
    // every wrapper is non-unary while the whole fixture has zero child refs.
    let inline_empty = [1u8, 0, 0];
    let mut wire = Vec::new();
    wire.extend_from_slice(b"QIMAP004");
    wire.extend_from_slice(&(if invalid_nibble { 1u16 } else { 0u16 }).to_be_bytes());
    if invalid_nibble {
        wire.push(16);
    }
    wire.push(1);
    wire.extend_from_slice(&1u16.to_be_bytes());
    wire.extend_from_slice(&1u16.to_be_bytes());
    wire.extend_from_slice(&inline_empty);
    wire.extend_from_slice(&0u16.to_be_bytes());
    wire.extend_from_slice(&inline_empty);
    for _ in 0..wrappers {
        let mut outer = Vec::new();
        outer.extend_from_slice(b"QIMAP006");
        outer.extend_from_slice(&0u16.to_be_bytes());
        outer.push(1);
        outer.extend_from_slice(&1u16.to_be_bytes());
        outer.extend_from_slice(&0u16.to_be_bytes());
        outer.extend_from_slice(&1u16.to_be_bytes());
        outer.extend_from_slice(&inline_empty);
        outer.extend_from_slice(
            &u16::try_from(wire.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        outer.extend_from_slice(&wire);
        wire = outer;
    }
    Ok(wire)
}

#[test]
fn nested_wire_admits_depth_eight_and_refuses_depth_nine_bad_prefix_and_cancel() -> TestResult {
    let dir = TestDir::new();
    let store = FileObjectStore::open(&dir.0)?;
    let mut work = budget()?;
    let valid = nested_inline_wire(8, false)?;
    let registered = store.put(&valid, &[], &mut work)?;
    let _decoded = store.read_node(registered.reference, &mut work)?;
    for wire in [nested_inline_wire(9, false)?, nested_inline_wire(8, true)?] {
        let registered = store.put(&wire, &[], &mut work)?;
        check!(matches!(
            store.read_node(registered.reference, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    let mut cancelled = budget()?;
    cancelled.request().cancel_handle().cancel();
    check!(matches!(
        store.read_node(registered.reference, &mut cancelled),
        Err(IndexError::Cancelled)
    ));
    Ok(())
}
