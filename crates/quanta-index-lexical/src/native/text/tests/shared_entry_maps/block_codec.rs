//! Byte-level Block oracle. Expected bytes are fixed literals, independent of
//! the Block writer and of any decoder round trip.

use super::super::super::entry_map::BlockDraft;
use super::*;
use quanta_index_core::index::StagedObjectBatch;

// Nine one-byte keys make Block canonical: Small can hold at most eight.
const NINE_KEYS: &[u8] = b"QITXEM01\x00\x09\x00\x00\x00\x01a\x00\x01b\x00\x01c\x00\x01d\x00\x01e\x00\x01f\x00\x01g\x00\x01h\x00\x01i";
const EMPTY_FIRST: &[u8] = b"QITXEM01\x00\x09\x00\x00\x00\x00\x00\x01a\x00\x01b\x00\x01c\x00\x01d\x00\x01e\x00\x01f\x00\x01g\x00\x01h";

fn nine_children() -> Vec<ObjectRef> {
    vec![
        reference(1),
        reference(2),
        reference(2), // Repeated child edges retain both ordinals.
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
        .zip(nine_children())
        .map(|(key, value)| (vec![key], value))
        .collect()
}

#[test]
fn staged_writer_matches_handwritten_wire_and_repeated_edges() -> Result<(), IndexError> {
    let mut work = budget()?;
    let mut draft = BlockDraft::new(&work)?;
    for (key, value) in (b'a'..=b'i').zip(nine_children()) {
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
        512
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
            0xb2, 0xd1, 0xb1, 0x6d, 0xc0, 0xa5, 0x1e, 0x2a, 0x60, 0x2d, 0x88, 0xe9, 0xe3, 0x5b,
            0x6c, 0x7c, 0x7b, 0x63, 0x4c, 0x18, 0x05, 0x65, 0x7a, 0xe2, 0xc0, 0x9a, 0xeb, 0x3f,
            0x58, 0xed, 0x4b, 0x2e,
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
    let mut bad_reserved = NINE_KEYS.to_vec();
    *bad_reserved
        .get_mut(11)
        .ok_or(IndexError::ResourceLimited)? = 1;
    failures.push((bad_reserved, children.clone()));
    let mut declared_ten = NINE_KEYS.to_vec();
    declared_ten
        .get_mut(8..10)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&10_u16.to_be_bytes());
    failures.push((declared_ten, children.clone()));
    let mut late_order = NINE_KEYS.to_vec();
    *late_order.last_mut().ok_or(IndexError::ResourceLimited)? = b'a';
    failures.push((late_order, children.clone()));
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
            .get(..8)
            .ok_or(IndexError::ResourceLimited)?
            .to_vec(),
    ));
    let mut swapped_children = children.clone();
    swapped_children.swap(7, 8);
    // A valid permutation changes content identity and positional values; it
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
        b"QITXEM01\x00\x01\x00\x00\x00\x01a".to_vec(),
        vec![reference(1)],
    ));
    failures.push((
        b"QITXEM01\x00\x01\x00\x00\x00\x00".to_vec(),
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
