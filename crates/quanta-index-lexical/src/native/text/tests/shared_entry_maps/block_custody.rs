//! Producer-side Block transitions and complete-input/batch custody.

use super::super::super::entry_map::BlockDraft;
use super::support::BatchFault;
use super::*;

#[test]
fn block_draft_denial_drop_and_cancellation_preserve_memory_custody() -> Result<(), IndexError> {
    let (work, memory) = funded_batch_budget(20_000)?;
    check!(matches!(
        BlockDraft::new(&work),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));

    let (work, memory) = funded_batch_budget(40_000)?;
    let draft = BlockDraft::new(&work)?;
    check!(memory.live.load(Ordering::Relaxed) >= 32_768);
    drop(draft);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));

    let (work, memory) = funded_batch_budget(40_000)?;
    work.request().cancel_handle().cancel();
    check!(matches!(BlockDraft::new(&work), Err(IndexError::Cancelled)));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn eight_short_rows_fit_without_reserving_a_block_draft() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let (mut work, memory) = funded_batch_budget(12_000)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let edits = (b'a'..=b'h')
        .map(|key| row(&[key], None, Some(reference(key))))
        .collect::<Vec<_>>();
    let roots = entry_patch::apply(
        EntryMapUse::Inventory,
        &[EntryMap::default()],
        &[8],
        &mut Rows::new(edits.clone()),
        |_slot| Ok(Rows::new(edits.clone())),
        &mut context,
    )?;
    check!(matches!(
        roots
            .as_slice()
            .first()
            .ok_or(IndexError::ResourceLimited)?,
        EntryMap::Small(_)
    ));
    check_eq!(objects.batches.load(Ordering::Relaxed), 0);
    drop(roots);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn block_capacity_crosses_to_tree_and_returns_without_losing_old_snapshots()
-> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let mut model = BTreeMap::new();
    let edits = (0_u16..512)
        .map(|ordinal| -> Result<OrdinalMapEdit, IndexError> {
            let key = ordinal.to_be_bytes();
            let byte = u8::try_from(ordinal % 251).map_err(|_error| IndexError::ResourceLimited)?;
            let value = reference(byte);
            let _prior = model.insert(key.to_vec(), value);
            Ok(row(&key, None, Some(value)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let block = execute(&objects, &maps, &EntryMap::default(), 512, &edits)?;
    check!(matches!(block, EntryMap::Block(_)));
    check_eq!(maps.applies.load(Ordering::Relaxed), 0);
    assert_rows(&objects, &maps, &block, &model)?;

    let extra = 512_u16.to_be_bytes();
    let value = reference(252);
    let tree = execute(
        &objects,
        &maps,
        &block,
        513,
        &[row(&extra, None, Some(value))],
    )?;
    check!(matches!(tree, EntryMap::Tree(_)));
    check_eq!(maps.applies.load(Ordering::Relaxed), 1);
    let _prior = model.insert(extra.to_vec(), value);
    assert_rows(&objects, &maps, &tree, &model)?;

    let restored = execute(
        &objects,
        &maps,
        &tree,
        512,
        &[row(&extra, Some(value), None)],
    )?;
    check!(matches!(restored, EntryMap::Block(_)));
    let _prior = model.remove(extra.as_slice());
    assert_rows(&objects, &maps, &restored, &model)?;
    assert_rows(&objects, &maps, &block, &model)?;
    Ok(())
}

#[test]
fn short_or_wrong_last_batch_result_cannot_publish_a_block() -> Result<(), IndexError> {
    let maps = ModelMaps::default();
    let local = (b'a'..=b'i')
        .map(|key| row(&[key], None, Some(reference(key))))
        .collect::<Vec<_>>();
    let mut original = local.clone();
    original.extend((b'a'..=b'i').map(|key| {
        OrdinalMapEdit::unscoped(
            1,
            MapEdit {
                key: vec![key],
                expected: None,
                next: Some(reference(key).into()),
            },
        )
    }));
    for fault in [BatchFault::Short, BatchFault::WrongLast] {
        let objects = CanonicalObjects::default();
        objects.fault_next_batch(fault)?;
        let fixture = visitor_fixture()?;
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = entry_budget()?;
        let mut context = AttemptContext {
            attempt: test_attempt(fixture.ticket.view.identity.workspace),
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        check!(matches!(
            entry_patch::apply(
                EntryMapUse::Inventory,
                &[EntryMap::default(); 2],
                &[9, 9],
                &mut Rows::new(original.clone()),
                |_slot| Ok(Rows::new(local.clone())),
                &mut context,
            ),
            Err(IndexError::BackendViolation(_) | IndexError::CorruptObject(_))
        ));
        check_eq!(objects.batches.load(Ordering::Relaxed), 1);
    }
    Ok(())
}

#[test]
fn block_plan_requires_exact_original_digest_eof_and_cas() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = entry_budget()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let planned = (b'a'..=b'i')
        .map(|key| row(&[key], None, Some(reference(key))))
        .collect::<Vec<_>>();
    let mut changed = planned.clone();
    *changed.get_mut(8).ok_or(IndexError::ResourceLimited)? = row(b"i", None, Some(reference(250)));
    check!(matches!(
        entry_patch::apply(
            EntryMapUse::Inventory,
            &[EntryMap::default()],
            &[9],
            &mut Rows::new(changed),
            |_slot| Ok(Rows::new(planned.clone())),
            &mut context,
        ),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(objects.batches.load(Ordering::Relaxed), 0);

    let mut interrupted = Rows::new(planned.clone());
    interrupted.fail_at_end = true;
    check!(matches!(
        entry_patch::apply(
            EntryMapUse::Inventory,
            &[EntryMap::default()],
            &[9],
            &mut interrupted,
            |_slot| Ok(Rows::new(planned.clone())),
            &mut context,
        ),
        Err(IndexError::Cancelled)
    ));
    check_eq!(objects.batches.load(Ordering::Relaxed), 0);

    let block = execute(&objects, &maps, &EntryMap::default(), 9, &planned)?;
    check!(matches!(
        execute(
            &objects,
            &maps,
            &block,
            9,
            &[row(b"a", Some(reference(251)), Some(reference(1)))],
        ),
        Err(IndexError::Conflict)
    ));
    Ok(())
}

#[test]
fn sixty_four_mixed_ordinals_keep_empty_small_and_block_roots_distinct() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = entry_budget()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let small = OrdinalMapEdit::unscoped(
        2,
        MapEdit {
            key: b"s".to_vec(),
            expected: None,
            next: Some(reference(17).into()),
        },
    );
    let block = (b'a'..=b'i')
        .map(|key| {
            OrdinalMapEdit::unscoped(
                31,
                MapEdit {
                    key: vec![key],
                    expected: None,
                    next: Some(reference(key).into()),
                },
            )
        })
        .collect::<Vec<_>>();
    let mut original = vec![small];
    original.extend(block);
    let mut counts = [0_u64; 64];
    counts[2] = 1;
    counts[31] = 9;
    let priors = vec![EntryMap::default(); 64];
    let roots = entry_patch::apply(
        EntryMapUse::Inventory,
        &priors,
        &counts,
        &mut Rows::new(original),
        |slot| {
            Ok(Rows::new(match slot {
                2 => vec![row(b"s", None, Some(reference(17)))],
                31 => (b'a'..=b'i')
                    .map(|key| row(&[key], None, Some(reference(key))))
                    .collect(),
                _ => Vec::new(),
            }))
        },
        &mut context,
    )?;
    check_eq!(roots.len(), 64);
    for (ordinal, root) in roots.as_slice().iter().enumerate() {
        match ordinal {
            2 => check!(matches!(root, EntryMap::Small(_))),
            31 => check!(matches!(root, EntryMap::Block(_))),
            _ => check_eq!(*root, EntryMap::default()),
        }
    }
    assert_rows(
        &objects,
        &maps,
        roots.as_slice().get(2).ok_or(IndexError::ResourceLimited)?,
        &BTreeMap::from([(b"s".to_vec(), reference(17))]),
    )?;
    assert_rows(
        &objects,
        &maps,
        roots
            .as_slice()
            .get(31)
            .ok_or(IndexError::ResourceLimited)?,
        &(b'a'..=b'i')
            .map(|key| (vec![key], reference(key)))
            .collect::<BTreeMap<_, _>>(),
    )?;
    check_eq!(objects.batches.load(Ordering::Relaxed), 1);
    check_eq!(maps.applies.load(Ordering::Relaxed), 0);
    Ok(())
}
