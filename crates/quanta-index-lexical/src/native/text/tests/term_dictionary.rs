//! Independent fixed-wire and lookup oracles for bounded term pages.

use super::super::{term_dictionary, term_dictionary::PageDraft};
use super::*;
use quanta_index_core::index::MemoryScope;

fn funded_page_budget() -> Result<(WorkBudget, Arc<TokenMemory>), IndexError> {
    let memory = Arc::new(TokenMemory {
        ceiling: 64 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = packet_budget()?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(TokenMemoryScope(
        Arc::clone(&memory),
    )))));
    Ok((work, memory))
}

fn fixed_two_row_wire() -> Vec<u8> {
    let mut wire = b"QITXTD01".to_vec();
    wire.extend_from_slice(&[0, 2, 2, 0, 0, 0, 0, 0]);
    wire.extend_from_slice(&[0, 2, 0, b'a', 0, 0, 0]);
    wire.extend_from_slice(&[0, 2, 1, b'a', 1, 8]);
    wire.extend_from_slice(&2_u64.to_be_bytes());
    wire.push(1);
    wire
}

#[test]
fn page_draft_matches_fixed_object_inline_wire_and_full_registration() -> Result<(), IndexError> {
    let (mut work, memory) = funded_page_budget()?;
    let object = MapValue::Object(reference(7));
    let inline = MapValue::inline(&2_u64.to_be_bytes(), Some(reference(8)))?;
    let mut draft = PageDraft::new(&mut work)?;
    check!(draft.is_empty());
    check!(draft.try_push(b"\0a", object, &mut work)?);
    check!(!draft.is_empty());
    check!(draft.try_push(b"\x01a", inline, &mut work)?);
    check_eq!(draft.len(), 2);
    let sealed = draft.finish(&mut work)?;
    check_eq!(sealed.fence.as_slice(), b"\x01a");
    check_eq!(sealed.input.payload, fixed_two_row_wire());
    check_eq!(sealed.input.children, vec![reference(7), reference(8)]);
    let registration = super::occurrence_blocks::CanonicalObjects::registration(
        &sealed.input.payload,
        sealed.input.children.clone(),
    )?;
    check_eq!(registration.bytes, 16 + 64 + 38);
    let expected = registration.reference;
    let stored = StoredObject::new(registration, sealed.input.payload.clone(), None);
    let page = term_dictionary::CheckedPage::parse(stored, expected, &mut work)?;
    check_eq!(page.reference(), expected);
    check_eq!(page.len(), 2);
    check_eq!(page.first_key()?, b"\0a");
    check_eq!(page.last_key()?, b"\x01a");
    page.validate_fence(b"\x01a")?;
    check!(matches!(
        page.validate_fence(b"\x01b"),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(page.entry(0)?, (&b"\0a"[..], object));
    check_eq!(page.entry(1)?, (&b"\x01a"[..], inline));
    check_eq!(page.lookup(b"\0b", &mut work)?, None);
    drop(page);
    drop(sealed);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn fixed_128_keys_fill_one_page_and_lookup_fences_and_gaps() -> Result<(), IndexError> {
    let (mut work, memory) = funded_page_budget()?;
    let mut objects = TestObjects::default();
    let mut maps = TestMaps::default();
    let root = MapRoot(Some(reference(91)));
    let mut keys = Vec::new();
    for case in [0_u8, 1_u8] {
        for ordinal in 0..64 {
            let mut key = vec![case];
            key.extend_from_slice(format!("k{ordinal:03}").as_bytes());
            keys.push(key);
        }
    }
    let value = MapValue::Object(reference(7));
    let mut page = PageDraft::new(&mut work)?;
    for key in &keys {
        check!(page.try_push(key, value, &mut work)?);
    }
    check_eq!(page.len(), 128);
    let mut sealed = page.finish(&mut work)?;
    check_eq!(sealed.fence.as_slice(), &keys[127]);
    let fence = sealed.fence.as_slice().to_vec();
    let payload = std::mem::take(&mut sealed.input.payload);
    let children = std::mem::take(&mut sealed.input.children);
    let registered = super::occurrence_blocks::CanonicalObjects::registration(&payload, children)?;
    let child = registered.reference;
    check!(
        objects
            .stored
            .insert(child, StoredObject::new(registered, payload, None))
            .is_none()
    );
    maps.add(root, fence, child)?;
    drop(sealed);
    objects.counted_body = Some(child);
    let absent_inside = b"\0k000x".as_slice();
    let absent_gap = b"\0z".as_slice();
    let absent_after = b"\x01z".as_slice();
    let queries: [&[u8]; 8] = [
        &keys[0],
        absent_inside,
        &keys[32],
        &keys[63],
        absent_gap,
        &keys[64],
        &keys[127],
        absent_after,
    ];
    let mut observed = [None; 8];
    term_dictionary::lookup_batch_sorted(
        &objects,
        &maps,
        root,
        &queries,
        &mut observed,
        &mut work,
    )?;
    check_eq!(
        observed,
        [
            Some(value),
            None,
            Some(value),
            Some(value),
            None,
            Some(value),
            Some(value),
            None,
        ]
    );
    check_eq!(objects.body_reads.load(Ordering::Relaxed), 1);
    check_eq!(objects.occurrence_reads.load(Ordering::Relaxed), 0);
    check_eq!(
        term_dictionary::lookup(&objects, &maps, root, absent_gap, &mut work)?,
        None
    );
    check_eq!(
        term_dictionary::lookup(&objects, &maps, root, &keys[64], &mut work)?,
        Some(value)
    );
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn unicode_and_maximum_key_lengths_split_on_actual_page_bytes() -> Result<(), IndexError> {
    let (mut work, memory) = funded_page_budget()?;
    let value = MapValue::Object(reference(7));
    let mut draft = PageDraft::new(&mut work)?;
    check!(draft.try_push("\0café".as_bytes(), value, &mut work)?);
    let mut last = Vec::new();
    for ordinal in 0..15 {
        let mut key = vec![1];
        key.extend_from_slice(format!("l{ordinal:03}").as_bytes());
        key.resize(1024, b'x');
        check!(draft.try_push(&key, value, &mut work)?);
        last = key;
    }
    let mut overflow = vec![1];
    overflow.extend_from_slice(b"l015");
    overflow.resize(1024, b'x');
    check!(!draft.try_push(&overflow, value, &mut work)?);
    check_eq!(draft.len(), 16);
    let sealed = draft.finish(&mut work)?;
    check_eq!(sealed.fence.as_slice(), last);
    check_eq!(sealed.input.children, vec![reference(7)]);
    let registration = super::occurrence_blocks::CanonicalObjects::registration(
        &sealed.input.payload,
        sealed.input.children.clone(),
    )?;
    let expected = registration.reference;
    let stored = StoredObject::new(registration, sealed.input.payload.clone(), None);
    let checked = term_dictionary::CheckedPage::parse(stored, expected, &mut work)?;
    check_eq!(checked.len(), 16);
    check_eq!(checked.first_key()?, "\0café".as_bytes());
    check_eq!(checked.last_key()?, last);
    drop(checked);
    drop(sealed);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn page_fills_exact_payload_bound_and_rejects_one_more_row() -> Result<(), IndexError> {
    let (mut work, memory) = funded_page_budget()?;
    let value = MapValue::Object(reference(7));
    let mut draft = PageDraft::new(&mut work)?;
    for ordinal in 0..15 {
        let mut key = vec![0];
        key.extend_from_slice(format!("a{ordinal:03}").as_bytes());
        key.resize(1024, b'x');
        check!(draft.try_push(&key, value, &mut work)?);
    }
    let mut last = vec![1, b'z'];
    last.resize(928, b'x');
    check!(draft.try_push(&last, value, &mut work)?);
    check!(!draft.try_push(b"\x01zz", value, &mut work)?);
    let sealed = draft.finish(&mut work)?;
    check_eq!(sealed.input.payload.len(), term_dictionary::MAX_PAGE_BYTES);
    check_eq!(sealed.fence.as_slice(), last);
    let registration = super::occurrence_blocks::CanonicalObjects::registration(
        &sealed.input.payload,
        sealed.input.children.clone(),
    )?;
    let expected = registration.reference;
    let stored = StoredObject::new(registration, sealed.input.payload.clone(), None);
    let checked = term_dictionary::CheckedPage::parse(stored, expected, &mut work)?;
    check_eq!(checked.len(), 16);
    drop(checked);
    drop(sealed);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn child_index_accepts_255_first_uses_and_reuse_after_full() -> Result<(), IndexError> {
    let (mut work, memory) = funded_page_budget()?;
    let mut draft = PageDraft::new(&mut work)?;
    for ordinal in 0..255_u16 {
        let mut key = vec![0];
        key.extend_from_slice(format!("{ordinal:03}").as_bytes());
        check!(draft.try_push(
            &key,
            MapValue::Object(reference(
                u8::try_from(ordinal + 1).map_err(|_error| IndexError::ResourceLimited)?
            )),
            &mut work,
        )?);
    }
    check_eq!(draft.len(), 255);
    check!(!draft.try_push(b"\x00255", MapValue::Object(reference(0)), &mut work)?);
    // Reference 1 and 5 collide in the deterministic 512-slot index. Full
    // reference equality must distinguish them and preserve first-use slots.
    check!(draft.try_push(b"\x01255", MapValue::Object(reference(1)), &mut work)?);
    let sealed = draft.finish(&mut work)?;
    check_eq!(sealed.input.children.len(), 255);
    let registration = super::occurrence_blocks::CanonicalObjects::registration(
        &sealed.input.payload,
        sealed.input.children.clone(),
    )?;
    let expected = registration.reference;
    let stored = StoredObject::new(registration, sealed.input.payload.clone(), None);
    let checked = term_dictionary::CheckedPage::parse(stored, expected, &mut work)?;
    check_eq!(checked.len(), 256);
    check_eq!(checked.entry(0)?.1, MapValue::Object(reference(1)));
    check_eq!(checked.entry(4)?.1, MapValue::Object(reference(5)));
    check_eq!(checked.entry(255)?.1, MapValue::Object(reference(1)));
    drop(checked);
    let mut duplicated = sealed.input.children.clone();
    duplicated[254] = duplicated[0];
    let registration = super::occurrence_blocks::CanonicalObjects::registration(
        &sealed.input.payload,
        duplicated,
    )?;
    let expected = registration.reference;
    let stored = StoredObject::new(registration, sealed.input.payload.clone(), None);
    check!(matches!(
        term_dictionary::CheckedPage::parse(stored, expected, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    drop(sealed);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn fixed_page_rejects_malformed_order_slot_children_and_identity() -> Result<(), IndexError> {
    let (mut work, memory) = funded_page_budget()?;
    let gold = fixed_two_row_wire();
    let children = vec![reference(7), reference(8)];
    for fault in 0..6_u8 {
        let mut wire = gold.clone();
        let mut refs = children.clone();
        match fault {
            0 => wire[0] = b'X',
            1 => wire[25..27].copy_from_slice(b"\0a"),
            2 => *wire.last_mut().ok_or(IndexError::ResourceLimited)? = 2,
            3 => refs[1] = refs[0],
            4 => wire.push(0),
            5 => wire[8..10].copy_from_slice(
                &u16::try_from(term_dictionary::MAX_PAGE_ROWS)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            ),
            _ => return Err(IndexError::ResourceLimited),
        }
        let registration = super::occurrence_blocks::CanonicalObjects::registration(&wire, refs)?;
        let expected = registration.reference;
        let stored = StoredObject::new(registration, wire, None);
        check!(matches!(
            term_dictionary::CheckedPage::parse(stored, expected, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    let registration = super::occurrence_blocks::CanonicalObjects::registration(&gold, children)?;
    let stored = StoredObject::new(registration, gold, None);
    check!(matches!(
        term_dictionary::CheckedPage::parse(stored, reference(200), &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}
