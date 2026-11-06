//! Independent byte oracle for the owner-local occurrence dictionary.
//!
//! Goldens below are literal wire bytes. They are not made by the writer or by
//! round-tripping the reader, so both sides cannot agree on the same bad wire.

use super::*;
use crate::native::{text::PreparedPositions, text_memory::PreparedKey};
use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
    MemoryScopePort, ObjectRef, RegisteredObject, StoredObject, WorkBudget, WorkLimits,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

macro_rules! check {
    ($condition:expr $(,)?) => {{
        if !$condition {
            return Err(IndexError::BackendViolation(format!(
                "test assertion failed at {}:{}: {}",
                file!(), line!(), stringify!($condition)
            ).into()));
        }
    }};
    ($condition:expr, $($message:tt)+) => {{
        if !$condition {
            return Err(IndexError::BackendViolation(format!($($message)+).into()));
        }
    }};
}

macro_rules! check_eq {
    ($left:expr, $right:expr $(,)?) => {{
        match (&$left, &$right) {
            (left, right) if *left == *right => {}
            (left, right) => {
                return Err(IndexError::BackendViolation(
                    format!(
                        "test equality failed at {}:{}: {left:?} != {right:?}",
                        file!(),
                        line!()
                    )
                    .into(),
                ));
            }
        }
    }};
}

// alpha has two positions and beta has one. The count and selector fields are
// distinct: the reader must select the right vector, not just the block ref.
const TWO_TERMS: &[u8] = b"QITXOC01\x00\x02\x00\x02\x00\x00\x00\x00\x00\x06\x00alpha\x00\x00\x00\x05\x00beta\x00\x01\x00\x02\x00\x00\x00\x03\x00\x00\x00\x0b\x00\x01\x00\x00\x00\x07";
// Both terms select vector 0. An encoder that repeats the vector is invalid.
const ALIASED: &[u8] = b"QITXOC01\x00\x02\x00\x01\x00\x00\x00\x00\x00\x06\x00alpha\x00\x00\x00\x05\x00beta\x00\x00\x00\x02\x00\x00\x00\x03\x00\x00\x00\x0b";

fn boundary_key(ordinal: u16) -> Vec<u8> {
    let suffix = if ordinal < 490 { "xxxxxx" } else { "xxxxx" };
    format!("\0k{ordinal:04}{suffix}").into_bytes()
}

/// Independent byte construction: 490 rows of 16 bytes, 22 rows of 15
/// bytes, a 16-byte header, and a 6-byte one-position vector total 8192.
fn literal_boundary_wire() -> Result<Vec<u8>, IndexError> {
    let mut wire = b"QITXOC01\x02\x00\x00\x01\x00\x00\x00\x00".to_vec();
    for ordinal in 0_u16..512 {
        let key = boundary_key(ordinal);
        wire.extend_from_slice(&[
            0,
            u8::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ]);
        wire.extend_from_slice(&key);
        wire.extend_from_slice(&[0, 0]);
    }
    wire.extend_from_slice(b"\x00\x01\x00\x00\x00\x03");
    check_eq!(wire.len(), 8_192);
    Ok(wire)
}

fn budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 100_000_000,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn input(
    key: &[u8],
    positions: &[u32],
    budget: &WorkBudget,
) -> Result<(PreparedKey, PreparedPositions), IndexError> {
    let key = PreparedKey::copy(key, budget)?;
    let mut prepared = PreparedPositions::new(8192)?;
    for &position in positions {
        prepared.push(position, budget)?;
    }
    Ok((key, prepared))
}

fn parsed_as(
    wire: &[u8],
    reference_byte: u8,
    budget: &WorkBudget,
) -> Result<CheckedBlock, IndexError> {
    // Fixture ownership is external input. The parser must fund its own layout.
    let object = StoredObject::new(
        RegisteredObject::unscoped(
            ObjectRef(Digest([reference_byte; 32])),
            u64::try_from(
                wire.len()
                    .checked_add(16)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            vec![],
        ),
        wire.to_vec(),
        None,
    );
    CheckedBlock::parse(object, budget)
}

fn parsed(wire: &[u8], budget: &WorkBudget) -> Result<CheckedBlock, IndexError> {
    parsed_as(wire, 1, budget)
}

fn selected<'a>(
    checked: &'a CheckedBlock,
    key: &[u8],
    budget: &WorkBudget,
) -> Result<&'a [u8], IndexError> {
    let range = checked.selection(key, budget)?;
    checked.object().payload.get(range).ok_or_else(|| {
        IndexError::BackendViolation("selection left its authenticated payload".into())
    })
}

fn byte_mut(bytes: &mut [u8], index: usize) -> Result<&mut u8, IndexError> {
    bytes.get_mut(index).ok_or_else(|| {
        IndexError::BackendViolation("fixed malformed-wire fixture offset changed".into())
    })
}

#[test]
fn writer_matches_literal_wire_and_reader_selects_distinct_tf() -> Result<(), IndexError> {
    let work = budget()?;
    let mut draft = BlockDraft::new()?;
    let (key, positions) = input(b"\0alpha", &[3, 11], &work)?;
    check!(matches!(
        draft.try_push(key, positions, &work)?,
        BlockPush::Added
    ));
    let (key, positions) = input(b"\0beta", &[7], &work)?;
    check!(matches!(
        draft.try_push(key, positions, &work)?,
        BlockPush::Added
    ));
    check_eq!(draft.len(), 2);
    let SealedBlock::Packed { payload, terms } = draft.finish(&work)? else {
        return Err(IndexError::BackendViolation(
            "two terms did not pack".into(),
        ));
    };
    check_eq!(payload.bytes.as_slice(), TWO_TERMS);
    check_eq!(terms.as_slice().len(), 2);
    let alpha = terms
        .as_slice()
        .first()
        .ok_or(IndexError::ResourceLimited)?;
    let beta = terms.as_slice().get(1).ok_or(IndexError::ResourceLimited)?;
    check_eq!(alpha.key.as_slice(), b"\0alpha");
    check_eq!(alpha.vector, 0);
    check_eq!(beta.key.as_slice(), b"\0beta");
    check_eq!(beta.vector, 1);
    let checked = parsed(TWO_TERMS, &work)?;
    check_eq!(
        selected(&checked, b"\0alpha", &work)?,
        b"\x00\x00\x00\x03\x00\x00\x00\x0b"
    );
    check_eq!(selected(&checked, b"\0beta", &work)?, b"\x00\x00\x00\x07");
    check!(checked.selection(b"\0gamma", &work).is_err());
    Ok(())
}

#[test]
fn equal_positions_share_one_canonical_vector() -> Result<(), IndexError> {
    let work = budget()?;
    let mut draft = BlockDraft::new()?;
    for key in [b"\0alpha".as_slice(), b"\0beta".as_slice()] {
        let (key, positions) = input(key, &[3, 11], &work)?;
        check!(matches!(
            draft.try_push(key, positions, &work)?,
            BlockPush::Added
        ));
    }
    let SealedBlock::Packed { payload, terms } = draft.finish(&work)? else {
        return Err(IndexError::BackendViolation(
            "alias pair did not pack".into(),
        ));
    };
    check_eq!(payload.bytes.as_slice(), ALIASED);
    let alpha = terms
        .as_slice()
        .first()
        .ok_or(IndexError::ResourceLimited)?;
    let beta = terms.as_slice().get(1).ok_or(IndexError::ResourceLimited)?;
    check_eq!(alpha.vector, 0);
    check_eq!(beta.vector, 0);
    let checked = parsed(ALIASED, &work)?;
    check_eq!(
        selected(&checked, b"\0alpha", &work)?,
        selected(&checked, b"\0beta", &work)?
    );
    check_eq!(checked.object().payload.as_slice(), ALIASED);
    let independent = parsed_as(TWO_TERMS, 2, &work)?;
    check_eq!(
        selected(&checked, b"\0beta", &work)?,
        b"\x00\x00\x00\x03\x00\x00\x00\x0b"
    );
    check_eq!(
        selected(&independent, b"\0beta", &work)?,
        b"\x00\x00\x00\x07"
    );
    check!(checked.object().registration.reference != independent.object().registration.reference);
    Ok(())
}

#[test]
fn writer_matches_literal_512_row_8192_byte_boundary() -> Result<(), IndexError> {
    let work = budget()?;
    let expected = literal_boundary_wire()?;
    let mut draft = BlockDraft::new()?;
    for ordinal in 0_u16..512 {
        let key = boundary_key(ordinal);
        let (key, positions) = input(&key, &[3], &work)?;
        check!(matches!(
            draft.try_push(key, positions, &work)?,
            BlockPush::Added
        ));
    }
    check_eq!(draft.len(), MAX_BLOCK_TERMS);
    let (key, positions) = input(b"\0k0512", &[5], &work)?;
    let BlockPush::Full { term, positions } = draft.try_push(key, positions, &work)? else {
        return Err(IndexError::BackendViolation(
            "513th row was not returned with its owned input".into(),
        ));
    };
    check_eq!(term.as_slice(), b"\0k0512");
    check_eq!(positions.as_slice(), &[5]);
    let SealedBlock::Packed { payload, terms } = draft.finish(&work)? else {
        return Err(IndexError::BackendViolation(
            "512-row boundary was not packed".into(),
        ));
    };
    check_eq!(terms.len(), 512);
    check_eq!(payload.bytes.as_slice(), expected.as_slice());
    let checked = parsed(&expected, &work)?;
    for ordinal in [0_u16, 489, 490, 511] {
        check_eq!(
            selected(&checked, &boundary_key(ordinal), &work)?,
            b"\x00\x00\x00\x03"
        );
    }
    let mut oversized = expected;
    oversized.push(0);
    check!(matches!(
        parsed(&oversized, &work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn reader_rejects_noncanonical_and_malformed_wire() -> Result<(), IndexError> {
    let work = budget()?;
    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    let mut wrong_magic = TWO_TERMS.to_vec();
    *byte_mut(&mut wrong_magic, 0)? = b'X';
    cases.push(("magic", wrong_magic));
    let mut reserved = TWO_TERMS.to_vec();
    *byte_mut(&mut reserved, 15)? = 1;
    cases.push(("reserved", reserved));
    let mut selector = TWO_TERMS.to_vec();
    *byte_mut(&mut selector, 34)? = 2;
    cases.push(("selector outside dictionary", selector));
    let mut first_use = TWO_TERMS.to_vec();
    *byte_mut(&mut first_use, 25)? = 1;
    *byte_mut(&mut first_use, 34)? = 0;
    cases.push(("out of order vector first use", first_use));
    let mut unused = TWO_TERMS.to_vec();
    *byte_mut(&mut unused, 34)? = 0;
    cases.push(("unused vector", unused));
    let mut duplicate_vector = TWO_TERMS.to_vec();
    check!(duplicate_vector.get(45..51).is_some());
    drop(duplicate_vector.splice(
        45..51,
        b"\x00\x02\x00\x00\x00\x03\x00\x00\x00\x0b".iter().copied(),
    ));
    cases.push(("duplicate vector", duplicate_vector));
    let mut duplicate_key = ALIASED.to_vec();
    check!(duplicate_key.get(28..33).is_some());
    drop(duplicate_key.splice(28..33, b"\0alpha".iter().copied()));
    *byte_mut(&mut duplicate_key, 27)? = 6;
    cases.push(("duplicate key", duplicate_key));
    let mut reverse_key = TWO_TERMS.to_vec();
    reverse_key
        .get_mut(29..33)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(b"aaaa");
    cases.push(("reverse key order", reverse_key));
    let mut bad_key = TWO_TERMS.to_vec();
    *byte_mut(&mut bad_key, 28)? = 2;
    cases.push(("invalid key mode", bad_key));
    let mut invalid_utf8 = TWO_TERMS.to_vec();
    *byte_mut(&mut invalid_utf8, 29)? = 0xff;
    cases.push(("invalid key utf8", invalid_utf8));
    let mut not_increasing = TWO_TERMS.to_vec();
    *byte_mut(&mut not_increasing, 44)? = 3;
    cases.push(("positions not increasing", not_increasing));
    let mut count_over_limit = TWO_TERMS.to_vec();
    *byte_mut(&mut count_over_limit, 35)? = 0x20;
    *byte_mut(&mut count_over_limit, 36)? = 0x01; // 8193, before any allocation from that count.
    cases.push(("position count 8193", count_over_limit));
    let mut trailing = TWO_TERMS.to_vec();
    trailing.push(0);
    cases.push(("trailing bytes", trailing));
    let shortened = TWO_TERMS
        .get(
            ..TWO_TERMS
                .len()
                .checked_sub(1)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    cases.push(("short input", shortened.to_vec()));
    // Otherwise canonical shape: eight distinct 1024-byte keys and one shared
    // vector. This isolates the payload bound from malformed magic/rows.
    let mut oversize = b"QITXOC01\x00\x08\x00\x01\x00\x00\x00\x00".to_vec();
    for prefix in [b'a', b'b', b'c', b'd', b'e', b'f', b'g', b'h'] {
        oversize.extend_from_slice(&1024_u16.to_be_bytes());
        oversize.push(0);
        oversize.push(prefix);
        oversize.extend(std::iter::repeat_n(b'x', 1022));
        oversize.extend_from_slice(&0_u16.to_be_bytes());
    }
    oversize.extend_from_slice(b"\x00\x01\x00\x00\x00\x01");
    check!(oversize.len() > MAX_BLOCK_BYTES);
    cases.push(("payload over 8192", oversize));
    let mut excessive_rows = TWO_TERMS.to_vec();
    *byte_mut(&mut excessive_rows, 8)? = 2;
    *byte_mut(&mut excessive_rows, 9)? = 1; // 513 rows; reject before row allocation.
    cases.push(("term count 513", excessive_rows));
    for (name, wire) in cases {
        check!(
            matches!(parsed(&wire, &work), Err(IndexError::CorruptObject(_))),
            "accepted {name}"
        );
    }
    Ok(())
}

#[test]
fn block_object_refuses_child_edges() -> Result<(), IndexError> {
    let work = budget()?;
    let object = StoredObject::new(
        RegisteredObject::unscoped(
            ObjectRef(Digest([1; 32])),
            67,
            vec![ObjectRef(Digest([2; 32]))],
        ),
        TWO_TERMS.to_vec(),
        None,
    );
    check!(matches!(
        CheckedBlock::parse(object, &work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn singleton_is_leaf_and_full_returns_original_input() -> Result<(), IndexError> {
    let work = budget()?;
    let mut one = BlockDraft::new()?;
    let (key, positions) = input(b"\0alpha", &[3], &work)?;
    check!(matches!(
        one.try_push(key, positions, &work)?,
        BlockPush::Added
    ));
    let SealedBlock::Leaf { term, positions } = one.finish(&work)? else {
        return Err(IndexError::BackendViolation(
            "singleton was not leaf".into(),
        ));
    };
    check_eq!(term.as_slice(), b"\0alpha");
    check_eq!(positions.as_slice(), &[3]);
    check!(BlockDraft::new()?.finish(&work).is_err());

    let mut full = BlockDraft::new()?;
    for ordinal in 0_u16..512 {
        let key = format!("\0k{ordinal:04}");
        let (key, positions) = input(key.as_bytes(), &[1], &work)?;
        check!(matches!(
            full.try_push(key, positions, &work)?,
            BlockPush::Added
        ));
    }
    check_eq!(full.len(), 512);
    let (key, positions) = input(b"\0k0512", &[5], &work)?;
    let BlockPush::Full { term, positions } = full.try_push(key, positions, &work)? else {
        return Err(IndexError::BackendViolation(
            "513th term was not returned".into(),
        ));
    };
    check_eq!(term.as_slice(), b"\0k0512");
    check_eq!(positions.as_slice(), &[5]);
    check_eq!(full.len(), 512);
    let SealedBlock::Packed { payload, .. } = full.finish(&work)? else {
        return Err(IndexError::BackendViolation(
            "full draft was not packed".into(),
        ));
    };
    check!(payload.bytes.len() <= MAX_BLOCK_BYTES);
    Ok(())
}

#[test]
fn byte_limit_returns_input_before_birth() -> Result<(), IndexError> {
    let work = budget()?;
    let mut draft = BlockDraft::new()?;
    for prefix in [b'a', b'b', b'c', b'd', b'e', b'f', b'g'] {
        let mut key = vec![b'x'; 1024];
        *byte_mut(&mut key, 0)? = 0;
        *byte_mut(&mut key, 1)? = prefix;
        let (key, positions) = input(&key, &[1], &work)?;
        check!(matches!(
            draft.try_push(key, positions, &work)?,
            BlockPush::Added
        ));
    }
    let mut key = vec![b'x'; 1024];
    *byte_mut(&mut key, 0)? = 0;
    *byte_mut(&mut key, 1)? = b'h';
    let (term_input, positions) = input(&key, &[1], &work)?;
    let BlockPush::Full { term, positions } = draft.try_push(term_input, positions, &work)? else {
        return Err(IndexError::BackendViolation(
            "overflow term was not returned".into(),
        ));
    };
    check_eq!(term.as_slice(), key.as_slice());
    check_eq!(positions.as_slice(), &[1]);
    check_eq!(draft.len(), 7);
    Ok(())
}

#[derive(Default)]
struct MemoryMeter {
    live: AtomicU64,
    ceiling: AtomicU64,
}

impl MemoryLeaseOwner for MemoryMeter {
    fn release(&self, bytes: u64) {
        let previous = self.live.fetch_sub(bytes, Ordering::Relaxed);
        assert!(
            previous >= bytes,
            "memory lease released more than admitted"
        );
    }
}

struct MeterScope(Arc<MemoryMeter>);

impl MemoryScopePort for MeterScope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self
            .0
            .live
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |prior| {
                prior
                    .checked_add(bytes)
                    .filter(|next| *next <= self.0.ceiling.load(Ordering::Relaxed))
            })
            .map_err(|_prior| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, self.0.clone()))
    }
}

#[test]
fn memory_denial_and_cancellation_are_not_capacity_full() -> Result<(), IndexError> {
    let mut work = budget()?;
    let meter = Arc::new(MemoryMeter::default());
    meter.ceiling.store(100_000, Ordering::Relaxed);
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MeterScope(meter.clone())))));
    let mut draft = BlockDraft::new()?;
    let (key, positions) = input(b"\0alpha", &[3, 11], &work)?;
    meter
        .ceiling
        .store(meter.live.load(Ordering::Relaxed), Ordering::Relaxed);
    check!(matches!(
        draft.try_push(key, positions, &work),
        Err(IndexError::ResourceLimited)
    ));
    drop(draft);
    check_eq!(meter.live.load(Ordering::Relaxed), 0);

    let mut work = budget()?;
    let meter = Arc::new(MemoryMeter::default());
    meter.ceiling.store(100_000, Ordering::Relaxed);
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MeterScope(meter.clone())))));
    let mut draft = BlockDraft::new()?;
    let (key, positions) = input(b"\0alpha", &[3], &work)?;
    work.request().cancel_handle().cancel();
    check!(matches!(
        draft.try_push(key, positions, &work),
        Err(IndexError::Cancelled)
    ));
    drop(draft);
    check_eq!(meter.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn output_grant_denial_and_success_both_refund_all_memory() -> Result<(), IndexError> {
    for deny_output in [true, false] {
        let mut work = budget()?;
        let meter = Arc::new(MemoryMeter::default());
        meter.ceiling.store(100_000, Ordering::Relaxed);
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MeterScope(meter.clone())))));
        let mut draft = BlockDraft::new()?;
        for key in [b"\0alpha".as_slice(), b"\0beta".as_slice()] {
            let (key, positions) = input(key, &[3, 11], &work)?;
            check!(matches!(
                draft.try_push(key, positions, &work)?,
                BlockPush::Added
            ));
        }
        check!(meter.live.load(Ordering::Relaxed) > 0);
        if deny_output {
            meter
                .ceiling
                .store(meter.live.load(Ordering::Relaxed), Ordering::Relaxed);
            check!(matches!(
                draft.finish(&work),
                Err(IndexError::ResourceLimited)
            ));
        } else {
            let SealedBlock::Packed { payload, terms } = draft.finish(&work)? else {
                return Err(IndexError::BackendViolation(
                    "two terms did not pack".into(),
                ));
            };
            check_eq!(payload.bytes.as_slice(), ALIASED);
            drop(payload);
            drop(terms);
        }
        check_eq!(meter.live.load(Ordering::Relaxed), 0);
    }
    Ok(())
}

#[test]
fn checked_layout_grant_lives_with_owned_object_and_refunds() -> Result<(), IndexError> {
    let mut work = budget()?;
    let meter = Arc::new(MemoryMeter::default());
    meter.ceiling.store(100_000, Ordering::Relaxed);
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MeterScope(meter.clone())))));
    let checked = parsed(TWO_TERMS, &work)?;
    check!(meter.live.load(Ordering::Relaxed) > 0);
    check_eq!(checked.object().payload.as_slice(), TWO_TERMS);
    check_eq!(selected(&checked, b"\0beta", &work)?, b"\x00\x00\x00\x07");
    drop(checked);
    check_eq!(meter.live.load(Ordering::Relaxed), 0);

    let mut work = budget()?;
    meter.ceiling.store(0, Ordering::Relaxed);
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(MeterScope(meter.clone())))));
    check!(matches!(
        parsed(TWO_TERMS, &work),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(meter.live.load(Ordering::Relaxed), 0);
    Ok(())
}
