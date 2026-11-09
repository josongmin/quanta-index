//! Byte-level Block oracle. Expected bytes are fixed literals, independent of
//! the Block writer and of any decoder round trip.

use super::super::super::entry_map::BlockDraft;
use super::*;
use quanta_index_core::index::StagedObjectBatch;

// Nine one-byte keys make Block canonical: Small can hold at most eight.
const NINE_KEYS: &[u8] = b"QITXEM02\x00\x09\x00\x08\x00\x01a\x00\x00\x00\x01b\x00\x01\x00\x01c\x00\x01\x00\x01d\x00\x02\x00\x01e\x00\x03\x00\x01f\x00\x04\x00\x01g\x00\x05\x00\x01h\x00\x06\x00\x01i\x00\x07";
const EMPTY_FIRST: &[u8] = b"QITXEM02\x00\x09\x00\x08\x00\x00\x00\x00\x00\x01a\x00\x01\x00\x01b\x00\x01\x00\x01c\x00\x02\x00\x01d\x00\x03\x00\x01e\x00\x04\x00\x01f\x00\x05\x00\x01g\x00\x06\x00\x01h\x00\x07";

fn nine_values() -> Vec<ObjectRef> {
    vec![
        reference(1),
        reference(2),
        reference(2), // The third row aliases the second child's slot.
        reference(4),
        reference(5),
        reference(6),
        reference(7),
        reference(8),
        reference(9),
    ]
}

fn nine_children() -> Vec<ObjectRef> {
    vec![
        reference(1),
        reference(2),
        reference(4),
        reference(5),
        reference(6),
        reference(7),
        reference(8),
        reference(9),
    ]
}

fn gold_rows() -> BTreeMap<Vec<u8>, ObjectRef> {
    (b'a'..=b'i')
        .zip(nine_values())
        .map(|(key, value)| (vec![key], value))
        .collect()
}

#[test]
fn staged_writer_matches_handwritten_wire_and_unique_children() -> Result<(), IndexError> {
    let mut work = budget()?;
    let mut draft = BlockDraft::new(&work)?;
    for (key, value) in (b'a'..=b'i').zip(nine_values()) {
        check!(draft.push(&[key], value, &mut work)?);
    }
    let mut batch = StagedObjectBatch::new(1, &work)?;
    let ordinal = draft.stage(&mut batch, &work)?;
    check_eq!(ordinal, 0);
    let input = batch
        .inputs()
        .get(ordinal)
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(input.payload.as_slice(), NINE_KEYS);
    check_eq!(input.children.as_slice(), nine_children().as_slice());

    let mut noncanonical = BlockDraft::new(&work)?;
    check!(noncanonical.push(b"a", reference(1), &mut work)?);
    check!(matches!(
        noncanonical.stage(&mut batch, &work),
        Err(IndexError::CorruptObject(_))
    ));
    let mut too_long = BlockDraft::new(&work)?;
    check!(!too_long.push(&vec![b'x'; 1025], reference(1), &mut work)?);
    Ok(())
}

#[test]
fn draft_row_and_payload_limits_return_overflow_without_extra_input() -> Result<(), IndexError> {
    let mut work = entry_budget()?;
    let mut rows = BlockDraft::new(&work)?;
    for ordinal in 0_u16..512 {
        check!(rows.push(&ordinal.to_be_bytes(), reference(1), &mut work)?);
    }
    check!(!rows.push(&512_u16.to_be_bytes(), reference(1), &mut work)?);
    let mut batch = StagedObjectBatch::new(1, &work)?;
    let slot = rows.stage(&mut batch, &work)?;
    check_eq!(
        batch
            .inputs()
            .get(slot)
            .ok_or(IndexError::ResourceLimited)?
            .children
            .len(),
        1
    );

    let mut bytes = BlockDraft::new(&work)?;
    for prefix in 0_u8..15 {
        let mut key = vec![0; 1024];
        *key.get_mut(0).ok_or(IndexError::ResourceLimited)? = prefix;
        check!(bytes.push(&key, reference(1), &mut work)?);
    }
    let mut overflow = vec![0; 1024];
    *overflow.get_mut(0).ok_or(IndexError::ResourceLimited)? = 15;
    check!(!bytes.push(&overflow, reference(1), &mut work)?);
    Ok(())
}

#[test]
fn five_hundred_twelve_rows_can_use_five_hundred_twelve_unique_children() -> Result<(), IndexError>
{
    let mut work = entry_budget()?;
    let mut draft = BlockDraft::new(&work)?;
    let mut expected = b"QITXEM02\x02\x00\x02\x00".to_vec();
    let mut children = Vec::new();
    for ordinal in 0_u16..512 {
        let key = ordinal.to_be_bytes();
        let mut digest = [0_u8; 32];
        digest
            .get_mut(..2)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(
                &ordinal
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
        let child = ObjectRef(Digest(digest));
        check!(draft.push(&key, child, &mut work)?);
        children.push(child);
        expected.extend_from_slice(b"\x00\x02");
        expected.extend_from_slice(&key);
        expected.extend_from_slice(&ordinal.to_be_bytes());
    }
    check_eq!(expected.len(), 3_084);
    let mut batch = StagedObjectBatch::new(1, &work)?;
    let slot = draft.stage(&mut batch, &work)?;
    let input = batch
        .inputs()
        .get(slot)
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(input.payload.as_slice(), expected.as_slice());
    check_eq!(input.children.as_slice(), children.as_slice());
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let root = objects.insert_wire(&expected, &children, &mut work)?;
    for ordinal in [0_u16, 255, 256, 511] {
        check_eq!(
            EntryMap::Block(root).lookup(&objects, &maps, &ordinal.to_be_bytes(), &mut work)?,
            children.get(usize::from(ordinal)).copied()
        );
    }
    Ok(())
}

#[test]
fn handwritten_wire_has_exact_child_ordinals_and_full_read_behavior() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let mut work = budget()?;
    let children = nine_children();
    let root = objects.insert_wire(NINE_KEYS, &children, &mut work)?;
    // Fixed SHA-256 of the physical object domain, header, ordered children,
    // and handwritten payload; this also pins the test port's identity rule.
    check_eq!(
        root,
        ObjectRef(Digest([
            0x38, 0x44, 0x60, 0xf4, 0x8c, 0xde, 0x0f, 0x58, 0xe3, 0x1e, 0x76, 0x3c, 0x21, 0x44,
            0xe9, 0x11, 0x9f, 0x91, 0x52, 0x59, 0x46, 0xef, 0x9d, 0xfc, 0x15, 0x80, 0x26, 0xfe,
            0xd5, 0x65, 0x71, 0x71,
        ]))
    );
    let map = EntryMap::Block(root);
    let stored = objects.get(root, &mut work)?;
    check_eq!(stored.payload.as_slice(), NINE_KEYS);
    check_eq!(stored.registration.children.as_slice(), children.as_slice());
    assert_rows(&objects, &maps, &map, &gold_rows())?;
    check_eq!(map.lookup(&objects, &maps, b"j", &mut work)?, None);
    let page = map.scan(&objects, &maps, Some(b"d"), 2, &mut work)?;
    check_eq!(page.entries.len(), 2);
    check_eq!(
        page.entries
            .first()
            .ok_or(IndexError::ResourceLimited)?
            .0
            .as_slice(),
        b"e"
    );
    check_eq!(
        page.entries
            .get(1)
            .ok_or(IndexError::ResourceLimited)?
            .0
            .as_slice(),
        b"f"
    );
    check!(page.has_more);
    check_eq!(maps.scans.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn empty_binary_key_is_a_real_first_row_and_continuation_key() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let mut work = budget()?;
    let root = objects.insert_wire(EMPTY_FIRST, &nine_children(), &mut work)?;
    let map = EntryMap::Block(root);
    check_eq!(
        map.lookup(&objects, &maps, b"", &mut work)?,
        Some(reference(1))
    );
    let first = map.scan(&objects, &maps, None, 1, &mut work)?;
    check_eq!(first.entries.len(), 1);
    check_eq!(
        first
            .entries
            .first()
            .ok_or(IndexError::ResourceLimited)?
            .0
            .as_slice(),
        b""
    );
    check!(first.has_more);
    let second = map.scan(&objects, &maps, Some(b""), 1, &mut work)?;
    check_eq!(second.entries.len(), 1);
    check_eq!(
        second
            .entries
            .first()
            .ok_or(IndexError::ResourceLimited)?
            .0
            .as_slice(),
        b"a"
    );
    Ok(())
}

#[test]
fn malformed_or_noncanonical_block_is_rejected_before_first_hit_or_page() -> Result<(), IndexError>
{
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let children = nine_children();
    let mut failures: Vec<(Vec<u8>, Vec<ObjectRef>)> = Vec::new();
    let mut wrong_magic = NINE_KEYS.to_vec();
    *wrong_magic.get_mut(0).ok_or(IndexError::ResourceLimited)? = b'X';
    failures.push((wrong_magic, children.clone()));
    let mut zero_rows = NINE_KEYS.to_vec();
    zero_rows
        .get_mut(8..10)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&0_u16.to_be_bytes());
    failures.push((zero_rows, children.clone()));
    let mut zero_children = NINE_KEYS.to_vec();
    *zero_children
        .get_mut(11)
        .ok_or(IndexError::ResourceLimited)? = 0;
    failures.push((zero_children, children.clone()));
    let mut too_many_children = NINE_KEYS.to_vec();
    *too_many_children
        .get_mut(11)
        .ok_or(IndexError::ResourceLimited)? = 9;
    failures.push((too_many_children, children.clone()));
    let mut declared_ten = NINE_KEYS.to_vec();
    declared_ten
        .get_mut(8..10)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&10_u16.to_be_bytes());
    failures.push((declared_ten, children.clone()));
    let mut late_order = NINE_KEYS.to_vec();
    let last_key = late_order
        .len()
        .checked_sub(3)
        .ok_or(IndexError::ResourceLimited)?;
    *late_order
        .get_mut(last_key)
        .ok_or(IndexError::ResourceLimited)? = b'a';
    failures.push((late_order, children.clone()));
    let mut out_of_range = NINE_KEYS.to_vec();
    *out_of_range.last_mut().ok_or(IndexError::ResourceLimited)? = 8;
    failures.push((out_of_range, children.clone()));
    let mut noncanonical_first_use = NINE_KEYS.to_vec();
    // Row b is the first use after slot zero. Slot two skips slot one.
    *noncanonical_first_use
        .get_mut(21)
        .ok_or(IndexError::ResourceLimited)? = 2;
    failures.push((noncanonical_first_use, children.clone()));
    let mut unused_child = NINE_KEYS.to_vec();
    *unused_child
        .get_mut(11)
        .ok_or(IndexError::ResourceLimited)? = 9;
    let mut extra_child = children.clone();
    extra_child.push(reference(10));
    failures.push((unused_child, extra_child));
    let mut duplicate_children = children.clone();
    *duplicate_children
        .get_mut(7)
        .ok_or(IndexError::ResourceLimited)? = reference(8);
    failures.push((NINE_KEYS.to_vec(), duplicate_children));
    let mut trailing = NINE_KEYS.to_vec();
    trailing.push(0);
    failures.push((trailing, children.clone()));
    let truncated = NINE_KEYS
        .len()
        .checked_sub(1)
        .ok_or(IndexError::ResourceLimited)?;
    failures.push((
        NINE_KEYS
            .get(..truncated)
            .ok_or(IndexError::ResourceLimited)?
            .to_vec(),
        children.clone(),
    ));
    failures.push((
        NINE_KEYS.to_vec(),
        children
            .get(..7)
            .ok_or(IndexError::ResourceLimited)?
            .to_vec(),
    ));
    let mut swapped_children = children.clone();
    swapped_children.swap(6, 7);
    // A valid permutation changes content identity and selected values; it
    // cannot silently decode as the gold map or borrow its old root identity.
    let mut work = budget()?;
    let gold = objects.insert_wire(NINE_KEYS, &children, &mut work)?;
    let swapped = objects.insert_wire(NINE_KEYS, &swapped_children, &mut work)?;
    check!(swapped != gold);
    check_eq!(
        EntryMap::Block(swapped).lookup(&objects, &maps, b"h", &mut work)?,
        Some(reference(9))
    );
    check_eq!(
        EntryMap::Block(swapped).lookup(&objects, &maps, b"i", &mut work)?,
        Some(reference(8))
    );
    // One short key must use Small, even if the wire is otherwise well formed.
    failures.push((
        b"QITXEM02\x00\x01\x00\x01\x00\x01a\x00\x00".to_vec(),
        vec![reference(1)],
    ));
    failures.push((
        b"QITXEM02\x00\x01\x00\x01\x00\x00\x00\x00".to_vec(),
        vec![reference(1)],
    ));
    for (payload, children) in failures {
        let mut work = budget()?;
        let root = objects.insert_wire(&payload, &children, &mut work)?;
        let map = EntryMap::Block(root);
        check!(matches!(
            map.lookup(&objects, &maps, b"a", &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        check!(matches!(
            map.scan(&objects, &maps, None, 1, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    let mut work = budget()?;
    let missing = EntryMap::Block(reference(251));
    check!(matches!(
        missing.lookup(&objects, &maps, b"a", &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}
