//! Independent ordered-map laws for the shared bounded Text representation.

use super::super::{
    entry_map::{EntryMap, SmallMap},
    entry_patch,
};
use super::*;
use quanta_index_core::index::{OrdinalMapEdit, StagedMapRoots};
use std::sync::Mutex;

#[derive(Default)]
struct ModelMaps {
    trees: Mutex<BTreeMap<ObjectRef, BTreeMap<Vec<u8>, ObjectRef>>>,
    scans: AtomicUsize,
    applies: AtomicUsize,
    stop_early: bool,
}
impl ModelMaps {
    fn keep(&self, rows: BTreeMap<Vec<u8>, ObjectRef>) -> Result<MapRoot, IndexError> {
        if rows.is_empty() {
            return Ok(MapRoot(None));
        }
        let mut hash = Sha256::new();
        hash.update(b"independent-test-ordered-map\0");
        for (key, value) in &rows {
            hash.update(
                u64::try_from(key.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
            hash.update(key);
            hash.update(value.0.0);
        }
        let reference = ObjectRef(Digest(hash.finalize().into()));
        let _prior = self
            .trees
            .lock()
            .map_err(|_error| IndexError::Storage("model lock".into()))?
            .insert(reference, rows);
        Ok(MapRoot(Some(reference)))
    }
}
impl PersistentMapPort for ModelMaps {
    fn lookup_object(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        budget.checkpoint()?;
        let trees = self
            .trees
            .lock()
            .map_err(|_error| IndexError::Storage("model lock".into()))?;
        let Some(reference) = root.0 else {
            return Ok(None);
        };
        let rows = trees
            .get(&reference)
            .ok_or_else(|| IndexError::CorruptObject("model root absent".into()))?;
        let found = rows.get(key).copied();
        drop(trees);
        Ok(found)
    }
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
        self.lookup_object(root, key, budget)
            .map(|value| value.map(Into::into))
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        budget.checkpoint()?;
        let _prior = self.scans.fetch_add(1, Ordering::Relaxed);
        let trees = self
            .trees
            .lock()
            .map_err(|_error| IndexError::Storage("model lock".into()))?;
        let rows = root
            .0
            .map(|root| {
                trees
                    .get(&root)
                    .ok_or_else(|| IndexError::CorruptObject("model root absent".into()))
            })
            .transpose()?;
        let mut entries = Vec::new();
        let mut more = false;
        if let Some(rows) = rows {
            for (key, value) in rows {
                if after.is_some_and(|after| key.as_slice() <= after) {
                    continue;
                }
                if entries.len()
                    == usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?
                {
                    more = true;
                    break;
                }
                entries.push((key.clone(), (*value).into()));
            }
        }
        Ok(MapPage::new(entries, more, Vec::new(), None))
    }
    fn apply(
        &self,
        _: MapRoot,
        _: &[MapEdit],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        Err(IndexError::BackendViolation(
            "model expects one complete stream".into(),
        ))
    }
    fn apply_batch(
        &self,
        _: &[MapBatchRequest],
        _: &dyn ObjectWriteSink,
        _: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        Err(IndexError::BackendViolation(
            "model expects one complete stream".into(),
        ))
    }
    fn apply_stream(
        &self,
        bases: &[MapRoot],
        input: &mut dyn MapEditCursor,
        _: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        let _prior = self.applies.fetch_add(1, Ordering::Relaxed);
        let mut roots = StagedMapRoots::prepare(bases.len(), budget)?;
        if self.stop_early {
            for _base in bases {
                roots.push(MapRoot(None))?;
            }
            return roots.finish();
        }
        let mut next = {
            let trees = self
                .trees
                .lock()
                .map_err(|_error| IndexError::Storage("model lock".into()))?;
            bases
                .iter()
                .map(|root| {
                    root.0.map_or_else(
                        || Ok(BTreeMap::new()),
                        |reference| {
                            trees.get(&reference).cloned().ok_or_else(|| {
                                IndexError::CorruptObject("model root absent".into())
                            })
                        },
                    )
                })
                .collect::<Result<Vec<_>, IndexError>>()?
        };
        let mut previous: Option<OrdinalMapEdit> = None;
        while let Some(row) = input.next_edit(budget)? {
            if previous.as_ref().is_some_and(|old| {
                old.root > row.root || (old.root == row.root && old.edit.key >= row.edit.key)
            }) {
                return Err(IndexError::Conflict);
            }
            let map = next
                .get_mut(usize::try_from(row.root).map_err(|_error| IndexError::ResourceLimited)?)
                .ok_or(IndexError::Conflict)?;
            if map.get(&row.edit.key).copied().map(Into::into) != row.edit.expected {
                return Err(IndexError::Conflict);
            }
            if let Some(value) = row.edit.next {
                let _old = map.insert(row.edit.key.clone(), value.object_ref()?);
            } else {
                let _old = map.remove(&row.edit.key);
            }
            previous = Some(row);
        }
        // Store no new root until the entire reference input and CAS have passed.
        for rows in next {
            roots.push(self.keep(rows)?)?;
        }
        roots.finish()
    }
}

struct Rows {
    rows: std::vec::IntoIter<OrdinalMapEdit>,
    fail_at_end: bool,
}
impl Rows {
    fn new(rows: Vec<OrdinalMapEdit>) -> Self {
        Self {
            rows: rows.into_iter(),
            fail_at_end: false,
        }
    }
}
impl MapEditCursor for Rows {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if let Some(row) = self.rows.next() {
            return Ok(Some(row));
        }
        if self.fail_at_end {
            return Err(IndexError::Cancelled);
        }
        Ok(None)
    }
}
fn row(key: &[u8], expected: Option<ObjectRef>, next: Option<ObjectRef>) -> OrdinalMapEdit {
    OrdinalMapEdit::unscoped(
        0,
        MapEdit {
            key: key.to_vec(),
            expected: expected.map(Into::into),
            next: next.map(Into::into),
        },
    )
}
fn execute(
    maps: &ModelMaps,
    prior: &EntryMap,
    count: u64,
    edits: &[OrdinalMapEdit],
) -> Result<EntryMap, IndexError> {
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let result = entry_patch::apply(
        &[*prior],
        &[count],
        &mut Rows::new(edits.to_vec()),
        |_slot| Ok(Rows::new(edits.to_vec())),
        &mut context,
    )?;
    result
        .as_slice()
        .first()
        .copied()
        .ok_or_else(|| IndexError::BackendViolation("missing model result".into()))
}
fn assert_rows(
    maps: &ModelMaps,
    value: &EntryMap,
    rows: &BTreeMap<Vec<u8>, ObjectRef>,
) -> Result<(), IndexError> {
    let mut work = budget()?;
    let mut cursor = MapEntries::new(maps, value);
    let mut actual = BTreeMap::new();
    while let Some(entry) = cursor.next(&mut work)? {
        let (key, value) = entry.object_parts()?;
        let _old = actual.insert(key.clone(), *value);
    }
    if actual != *rows {
        return Err(IndexError::BackendViolation(
            "entry map differs from independent ordered model".into(),
        ));
    }
    for (key, expected) in rows {
        if value.lookup(maps, key, &mut work)? != Some(*expected) {
            return Err(IndexError::BackendViolation(
                "entry point lookup differs".into(),
            ));
        }
    }
    Ok(())
}

#[test]
fn inline_eight_expand_nine_and_shrink_eight_preserve_snapshots_and_model() -> Result<(), IndexError>
{
    let maps = ModelMaps::default();
    let mut expected = BTreeMap::new();
    let initial = (b'a'..=b'h')
        .map(|key| {
            let value = reference(key);
            let _old = expected.insert(vec![key], value);
            row(&[key], None, Some(value))
        })
        .collect::<Vec<_>>();
    let small = execute(&maps, &EntryMap::default(), 8, &initial)?;
    check!(small.inline_map().is_some());
    check_eq!(maps.applies.load(Ordering::Relaxed), 0);
    assert_rows(&maps, &small, &expected)?;
    let ninth = reference(b'i');
    let large = execute(&maps, &small, 9, &[row(b"i", None, Some(ninth))])?;
    let old = expected.clone();
    let _prior = expected.insert(b"i".to_vec(), ninth);
    check!(large.inline_map().is_none());
    check_eq!(maps.applies.load(Ordering::Relaxed), 1);
    assert_rows(&maps, &large, &expected)?;
    assert_rows(&maps, &small, &old)?;
    let expanded = expected.clone();
    let _prior = expected.remove(b"a".as_slice());
    let shrunk = execute(&maps, &large, 8, &[row(b"a", Some(reference(b'a')), None)])?;
    check!(shrunk.inline_map().is_some());
    check_eq!(maps.applies.load(Ordering::Relaxed), 1);
    assert_rows(&maps, &shrunk, &expected)?;
    assert_rows(&maps, &large, &expanded)?;
    Ok(())
}

#[test]
fn long_key_fallback_bad_cas_and_final_cardinality_have_independent_oracles()
-> Result<(), IndexError> {
    let maps = ModelMaps::default();
    // An absent object is corruption, distinct from the legitimate empty root.
    let missing = EntryMap::from(MapRoot(Some(reference(99))));
    let mut original = budget()?;
    check!(matches!(
        missing.lookup(&maps, b"a", &mut original),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        missing.scan(&maps, None, 1, &mut original),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        execute(&maps, &missing, 9, &[row(b"a", None, Some(reference(1)))]),
        Err(IndexError::CorruptObject(_))
    ));
    let key = vec![b'x'; 33];
    let value = reference(91);
    let long = execute(
        &maps,
        &EntryMap::default(),
        1,
        &[row(&key, None, Some(value))],
    )?;
    check!(long.inline_map().is_none());
    assert_rows(&maps, &long, &BTreeMap::from([(key, value)]))?;
    let before = maps.applies.load(Ordering::Relaxed);
    check!(matches!(
        execute(
            &maps,
            &EntryMap::default(),
            1,
            &[row(b"a", Some(value), Some(value))]
        ),
        Err(IndexError::Conflict)
    ));
    check!(matches!(
        execute(
            &maps,
            &EntryMap::default(),
            2,
            &[row(b"a", None, Some(value))]
        ),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(maps.applies.load(Ordering::Relaxed), before);
    Ok(())
}

#[test]
fn compact_result_requires_the_same_complete_original_edit_stream() -> Result<(), IndexError> {
    let maps = ModelMaps::default();
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let first = row(b"a", None, Some(reference(1)));
    let changed = row(b"a", None, Some(reference(2)));
    check!(matches!(
        entry_patch::apply(
            &[EntryMap::default()],
            &[1],
            &mut Rows::new(vec![changed]),
            |_slot| Ok(Rows::new(vec![first.clone()])),
            &mut context
        ),
        Err(IndexError::CorruptObject(_))
    ));
    let mut interrupted = Rows::new(vec![first.clone()]);
    interrupted.fail_at_end = true;
    check!(matches!(
        entry_patch::apply(
            &[EntryMap::default()],
            &[1],
            &mut interrupted,
            |_slot| Ok(Rows::new(vec![first.clone()])),
            &mut context
        ),
        Err(IndexError::Cancelled)
    ));
    check_eq!(maps.applies.load(Ordering::Relaxed), 0);
    let partial = ModelMaps {
        stop_early: true,
        ..ModelMaps::default()
    };
    check!(matches!(
        execute(
            &partial,
            &EntryMap::default(),
            1,
            &[row(&[b'z'; 33], None, Some(reference(4)))]
        ),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}

#[test]
fn large_tree_delta_does_not_scan_the_unrelated_prior_population() -> Result<(), IndexError> {
    let maps = ModelMaps::default();
    let mut before = BTreeMap::new();
    for key in 0_u16..4096 {
        let _prior = before.insert(key.to_be_bytes().to_vec(), reference(5));
    }
    let prior = EntryMap::from(maps.keep(before.clone())?);
    let mut after = before.clone();
    let key = 512_u16.to_be_bytes();
    let _prior = after.insert(key.to_vec(), reference(6));
    let next = execute(
        &maps,
        &prior,
        4096,
        &[row(&key, Some(reference(5)), Some(reference(6)))],
    )?;
    check_eq!(maps.scans.load(Ordering::Relaxed), 0);
    assert_rows(&maps, &next, &after)?;
    assert_rows(&maps, &prior, &before)?;
    Ok(())
}

#[test]
fn inline_codec_bounds_padding_and_direct_child_edges_are_not_advisory() -> Result<(), IndexError> {
    let mut small = SmallMap::default();
    let mut children = vec![reference(200)];
    for key in b'a'..=b'h' {
        let child = reference(key);
        check!(small.push(&[0, key], child)?);
        children.push(child);
    }
    check!(!small.push(b"i", reference(100))?);
    let inventory = EntryMap::inline(&small);
    let contribution = Contribution {
        record: reference(200),
        inventory,
        indexed_tokens: 4,
    };
    check_eq!(
        contribution_children(&contribution)?.as_slice(),
        children.as_slice()
    );
    let mut value =
        serde_json::to_value(inventory).map_err(|error| IndexError::Storage(error.to_string()))?;
    for (path, replacement) in [
        ("/small/len", serde_json::json!(9)),
        ("/small/rows/0/key/31", serde_json::json!(7)),
        ("/small/rows/0/len", serde_json::json!(33)),
    ] {
        let mut corrupt = value.clone();
        *corrupt
            .pointer_mut(path)
            .ok_or_else(|| IndexError::BackendViolation("codec mutation path missing".into()))? =
            replacement;
        let decoded: EntryMap = serde_json::from_value(corrupt)
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        check!(matches!(
            decoded.validate(),
            Err(IndexError::CorruptObject(_))
        ));
    }
    let record = value
        .pointer_mut("/tree")
        .ok_or_else(|| IndexError::BackendViolation("codec tree path missing".into()))?;
    *record = serde_json::to_value(MapRoot(Some(reference(9))))
        .map_err(|error| IndexError::Storage(error.to_string()))?;
    let overlap: EntryMap =
        serde_json::from_value(value).map_err(|error| IndexError::Storage(error.to_string()))?;
    check!(matches!(
        overlap.validate(),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn mixed_root_ordinals_empty_roots_and_malformed_complete_streams_are_exact()
-> Result<(), IndexError> {
    let maps = ModelMaps::default();
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let long = [b'z'; 33];
    let edits = vec![
        row(b"a", None, Some(reference(1))),
        OrdinalMapEdit::unscoped(
            2,
            MapEdit {
                key: long.to_vec(),
                expected: None,
                next: Some((reference(2)).into()),
            },
        ),
    ];
    let results = entry_patch::apply(
        &[EntryMap::default(); 3],
        &[1, 0, 1],
        &mut Rows::new(edits),
        |slot| {
            Ok(Rows::new(match slot {
                0 => vec![row(b"a", None, Some(reference(1)))],
                1 => Vec::new(),
                2 => vec![row(&long, None, Some(reference(2)))],
                _ => return Err(IndexError::Conflict),
            }))
        },
        &mut context,
    )?;
    let mut result = results.as_slice().iter();
    assert_rows(
        &maps,
        result.next().ok_or(IndexError::Conflict)?,
        &BTreeMap::from([(b"a".to_vec(), reference(1))]),
    )?;
    assert_rows(
        &maps,
        result.next().ok_or(IndexError::Conflict)?,
        &BTreeMap::new(),
    )?;
    assert_rows(
        &maps,
        result.next().ok_or(IndexError::Conflict)?,
        &BTreeMap::from([(long.to_vec(), reference(2))]),
    )?;
    check!(result.next().is_none());
    drop(results);
    let first = row(b"a", None, Some(reference(1)));
    let foreign = OrdinalMapEdit::unscoped(
        1,
        MapEdit {
            key: b"a".to_vec(),
            expected: None,
            next: Some((reference(1)).into()),
        },
    );
    for malformed in [
        vec![],
        vec![foreign],
        vec![first.clone(), first.clone()],
        vec![row(b"b", None, Some(reference(1))), first.clone()],
    ] {
        check!(matches!(
            entry_patch::apply(
                &[EntryMap::default()],
                &[1],
                &mut Rows::new(malformed),
                |_slot| Ok(Rows::new(vec![first.clone()])),
                &mut context
            ),
            Err(IndexError::CorruptObject(_))
        ));
    }
    let mut empty = Rows::new(Vec::new());
    let empty_result = entry_patch::apply(
        &vec![EntryMap::default(); 64],
        &[0; 64],
        &mut empty,
        |_slot| Ok(Rows::new(Vec::new())),
        &mut context,
    )?;
    check_eq!(empty_result.len(), 64);
    check!(
        empty_result
            .as_slice()
            .iter()
            .all(|root| *root == EntryMap::default())
    );
    Ok(())
}
