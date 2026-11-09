//! Independent posting membership model for the Small/Tree witness boundary.

use super::*;

fn posting_key(id: u8) -> Vec<u8> {
    vec![id; 32]
}

fn original_rows() -> BTreeMap<Vec<u8>, ObjectRef> {
    (1_u8..=9)
        .map(|id| (posting_key(id), reference(id)))
        .collect()
}

fn tree_values(
    maps: &ModelMaps,
    entry: &EntryMap,
) -> Result<BTreeMap<Vec<u8>, MapValue>, IndexError> {
    let EntryMap::Tree(reference) = entry else {
        return Err(IndexError::BackendViolation(
            "posting Tree was expected".into(),
        ));
    };
    maps.trees
        .lock()
        .map_err(|_error| IndexError::Storage("model lock".into()))?
        .get(reference)
        .cloned()
        .ok_or_else(|| IndexError::CorruptObject("model root absent".into()))
}

fn posting_apply(
    objects: &CanonicalObjects,
    maps: &ModelMaps,
    prior: &EntryMap,
    count: u64,
    original: Vec<OrdinalMapEdit>,
    planned: &[OrdinalMapEdit],
    refs: &mut entry_patch::PostingRefResolver<'_>,
) -> Result<EntryMap, IndexError> {
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = entry_budget()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects,
        maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let roots = entry_patch::apply(
        EntryMapUse::Posting,
        std::slice::from_ref(prior),
        &[count],
        &mut Rows::new(original),
        |_slot| Ok(Rows::new(planned.to_vec())),
        &mut context,
        Some(refs),
    )?;
    roots
        .as_slice()
        .first()
        .copied()
        .ok_or_else(|| IndexError::BackendViolation("posting root absent".into()))
}

fn create_tree(objects: &CanonicalObjects, maps: &ModelMaps) -> Result<EntryMap, IndexError> {
    let initial = original_rows();
    let edits = initial
        .iter()
        .map(|(key, value)| row(key, None, Some(*value)))
        .collect::<Vec<_>>();
    posting_apply(
        objects,
        maps,
        &EntryMap::default(),
        9,
        edits.clone(),
        &edits,
        &mut |_slot, _key, _witness, _budget| {
            Err(IndexError::BackendViolation(
                "empty posting Tree cannot need a resolver".into(),
            ))
        },
    )
}

fn assert_exact_witnesses(
    maps: &ModelMaps,
    entry: &EntryMap,
    expected: &BTreeMap<Vec<u8>, ObjectRef>,
) -> Result<(), IndexError> {
    let actual = tree_values(maps, entry)?;
    check_eq!(actual.len(), expected.len());
    for (key, reference) in expected {
        let value = *actual
            .get(key)
            .ok_or_else(|| IndexError::BackendViolation("posting witness key absent".into()))?;
        check_eq!(
            value,
            OccurrenceDigestWitness::from_reference(*reference).into_value()?
        );
        check_eq!(value.children()?.len(), 0);
        check!(value.object_ref().is_err());
    }
    Ok(())
}

#[test]
fn posting_nine_eight_nine_keeps_exact_childless_witness_and_real_small_refs()
-> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let original = original_rows();
    let tree = create_tree(&objects, &maps)?;
    assert_exact_witnesses(&maps, &tree, &original)?;
    check_eq!(maps.applies.load(Ordering::Relaxed), 1);

    let deleted = posting_key(5);
    let delete = row(&deleted, original.get(&deleted).copied(), None);
    let mut calls = Vec::new();
    let small = posting_apply(
        &objects,
        &maps,
        &tree,
        8,
        vec![delete.clone()],
        &[delete],
        &mut |slot, key, _witness, _budget| {
            check_eq!(slot, 0);
            calls.push(key.to_vec());
            original
                .get(key)
                .copied()
                .ok_or_else(|| IndexError::CorruptObject("owner inventory key absent".into()))
        },
    )?;
    let expected_calls = original
        .keys()
        .filter(|key| key.as_slice() != deleted.as_slice())
        .cloned()
        .collect::<Vec<_>>();
    check_eq!(calls, expected_calls);
    check_eq!(maps.applies.load(Ordering::Relaxed), 1);
    let inline = small
        .inline_map()
        .ok_or_else(|| IndexError::BackendViolation("posting Small was expected".into()))?;
    check_eq!(inline.len(), 8);
    for (slot, (expected_key, expected_ref)) in original
        .iter()
        .filter(|(key, _)| key.as_slice() != deleted.as_slice())
        .enumerate()
    {
        let (actual_key, actual_ref) = inline.entry(slot)?.ok_or(IndexError::Conflict)?;
        check_eq!(actual_key, expected_key.as_slice());
        check_eq!(actual_ref, *expected_ref);
    }

    let add = row(&deleted, None, original.get(&deleted).copied());
    let again = posting_apply(
        &objects,
        &maps,
        &small,
        9,
        vec![add.clone()],
        &[add],
        &mut |_slot, _key, _witness, _budget| {
            Err(IndexError::BackendViolation(
                "Small refs must not need a resolver".into(),
            ))
        },
    )?;
    assert_exact_witnesses(&maps, &again, &original)?;
    check_eq!(maps.applies.load(Ordering::Relaxed), 2);
    assert_exact_witnesses(&maps, &tree, &original)?;
    Ok(())
}

#[test]
fn posting_tree_cas_is_exact_and_rejects_malformed_prior_and_late_stream() -> Result<(), IndexError>
{
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let original = original_rows();
    let tree = create_tree(&objects, &maps)?;
    let changed_key = posting_key(4);
    let replacement = reference(44);
    let replace = row(
        &changed_key,
        original.get(&changed_key).copied(),
        Some(replacement),
    );
    let next = posting_apply(
        &objects,
        &maps,
        &tree,
        9,
        vec![replace.clone()],
        &[replace],
        &mut |_slot, _key, _witness, _budget| Err(IndexError::Conflict),
    )?;
    let mut expected = original.clone();
    let _old = expected.insert(changed_key.clone(), replacement);
    assert_exact_witnesses(&maps, &next, &expected)?;
    assert_exact_witnesses(&maps, &tree, &original)?;
    let before_applies = maps.applies.load(Ordering::Relaxed);
    let before_roots = maps
        .trees
        .lock()
        .map_err(|_error| IndexError::Storage("model lock".into()))?
        .len();
    let wrong = row(&changed_key, Some(reference(99)), Some(reference(45)));
    check!(matches!(
        posting_apply(
            &objects,
            &maps,
            &next,
            9,
            vec![wrong.clone()],
            &[wrong],
            &mut |_slot, _key, _witness, _budget| Err(IndexError::Conflict),
        ),
        Err(IndexError::Conflict)
    ));
    check_eq!(
        maps.applies.load(Ordering::Relaxed),
        before_applies
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?
    );
    let roots_after_bad_cas = maps
        .trees
        .lock()
        .map_err(|_error| IndexError::Storage("model lock".into()))?
        .len();
    check_eq!(roots_after_bad_cas, before_roots);

    // A malformed prior cannot be interpreted as a real ObjectRef or published.
    for corrupt in [
        MapValue::Object(reference(3)),
        MapValue::inline(&reference(3).0.0, [reference(90)])?,
    ] {
        let mut values = tree_values(&maps, &tree)?;
        let _old = values.insert(posting_key(3), corrupt);
        let malformed = EntryMap::from(maps.keep_values(values)?);
        let delete = row(&posting_key(5), Some(reference(5)), None);
        let prior_applies = maps.applies.load(Ordering::Relaxed);
        check!(matches!(
            posting_apply(
                &objects,
                &maps,
                &malformed,
                8,
                vec![delete.clone()],
                &[delete],
                &mut |_slot, key, _witness, _budget| original
                    .get(key)
                    .copied()
                    .ok_or(IndexError::Conflict),
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(maps.applies.load(Ordering::Relaxed), prior_applies);
    }

    // The resolver consults the independent owner inventory by key; it never
    // converts witness bytes into a reference. A mismatched inventory fails.
    let delete = row(&posting_key(5), Some(reference(5)), None);
    let prior_applies = maps.applies.load(Ordering::Relaxed);
    check!(matches!(
        posting_apply(
            &objects,
            &maps,
            &tree,
            8,
            vec![delete.clone()],
            std::slice::from_ref(&delete),
            &mut |_slot, key, _witness, _budget| {
                if key == posting_key(2).as_slice() {
                    Ok(reference(99))
                } else {
                    original.get(key).copied().ok_or(IndexError::Conflict)
                }
            },
        ),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(maps.applies.load(Ordering::Relaxed), prior_applies);
    check!(matches!(
        posting_apply(
            &objects,
            &maps,
            &tree,
            8,
            vec![row(&posting_key(6), Some(reference(6)), None)],
            &[delete],
            &mut |_slot, key, _witness, _budget| original
                .get(key)
                .copied()
                .ok_or(IndexError::Conflict),
        ),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(maps.applies.load(Ordering::Relaxed), prior_applies);
    let roots_after_refusals = maps
        .trees
        .lock()
        .map_err(|_error| IndexError::Storage("model lock".into()))?
        .len();
    check_eq!(
        roots_after_refusals,
        before_roots
            .checked_add(2)
            .ok_or(IndexError::ResourceLimited)?
    );
    Ok(())
}
