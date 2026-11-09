//! Producer-selected packed subtrees must preserve ordinary map semantics.

use super::*;
use quanta_index_core::index::ports::{MapDiffRange, MapDiffVisitor};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct HintCursor {
    edits: std::vec::IntoIter<OrdinalMapEdit>,
    selected: Option<u32>,
}

impl MapEditCursor for HintCursor {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        Ok(self.edits.next())
    }

    fn pack_terminal_subtrees_for_root(&self) -> Option<u32> {
        self.selected
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

    fn pack_terminal_subtrees_for_root(&self) -> Option<u32> {
        Some(2)
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
            ));
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
        ));
    }
    let before = physical_file_count(directory)?;
    let mut cursor = HintCursor {
        edits: edits.into_iter(),
        selected,
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
        selected: Some(0),
        edits: vec![OrdinalMapEdit::unscoped(
            0,
            MapEdit {
                key: removed_key.clone(),
                expected: None,
                next: Some(replacement),
            },
        )]
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
        selected: Some(0),
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
        .collect::<Vec<_>>()
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
