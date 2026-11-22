//! One bounded, term-bound object for short posting memberships.
//!
//! Inline map data is only a slot/DF locator. The complete stored block binds
//! that slot to its canonical term and authenticates every child slot.

use super::entry_map::SmallMap;
use quanta_index_core::index::{
    IndexError, MapValue, MemoryLease, MemoryLifetime, ObjectBatchInput, ObjectRef, StoredObject,
    WorkBudget, WorkUsage,
};

pub(super) const LAYOUT_POLICY: &[u8] = b"small-posting:QITXSP01;be16-rows-terms-children-reserved0;full-term1024;DF2-8;owner32-child16;sorted-unique;first-use-children;inline-tag3-slot16-df64-child1;max16384-512;packing-per-case-stream;merge-fitting-tails;recipe-fixed-v2\0";
pub(super) const INLINE_TAG: u8 = 3;
pub(super) const MAGIC: &[u8; 8] = b"QITXSP01";
const HEADER_BYTES: usize = 16;
const MAX_BYTES: usize = 16_384;
const MAX_ROWS: usize = 512;
pub(super) const MAX_TERMS: usize = MAX_ROWS.div_euclid(2);
const MAX_TERM_BYTES: usize = 1_024;
const ROW_BYTES: usize = 34;

fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(format!("small posting block: {reason}"))
}

fn check_term(term: &[u8]) -> Result<(), IndexError> {
    if !(2..=MAX_TERM_BYTES).contains(&term.len())
        || term.first().copied().is_none_or(|tag| tag > 1)
        || term
            .get(1..)
            .is_none_or(|key| std::str::from_utf8(key).is_err())
    {
        return Err(corrupt("canonical term shape differs"));
    }
    Ok(())
}

fn width(value: usize) -> Result<u64, IndexError> {
    u64::try_from(value).map_err(|_error| IndexError::ResourceLimited)
}

/// Payload and child backing remain admitted until the caller adopts them.
pub(super) struct EncodedBlock {
    pub(super) input: ObjectBatchInput,
    // Payload and children drop before the Original grant on every path.
    _memory: Option<MemoryLease>,
}

pub(super) struct Draft {
    payload: Vec<u8>,
    children: Vec<ObjectRef>,
    rows: usize,
    terms: u16,
    previous: Option<(usize, usize)>,
    memory: Option<MemoryLease>,
}

impl Draft {
    pub(super) fn new(budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let child_bytes = MAX_ROWS
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let remap_bytes = std::mem::size_of::<ObjectRef>()
            .checked_add(2)
            .and_then(|bytes| bytes.checked_mul(8))
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = MAX_BYTES
            .checked_add(child_bytes)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .and_then(|bytes| bytes.checked_add(remap_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(width(bytes)?, MemoryLifetime::Temporary)?;
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(MAX_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(MAX_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if payload.capacity() > MAX_BYTES || children.capacity() > MAX_ROWS {
            return Err(IndexError::ResourceLimited);
        }
        budget.charge(WorkUsage {
            bytes: width(HEADER_BYTES)?,
            ..WorkUsage::default()
        })?;
        payload.extend_from_slice(MAGIC);
        payload.resize(HEADER_BYTES, 0);
        Ok(Self {
            payload,
            children,
            rows: 0,
            terms: 0,
            previous: None,
            memory,
        })
    }

    /// A full block declines before mutation. Format boundaries never depend
    /// on the available memory grant or backend credit.
    pub(super) fn push(
        &mut self,
        term: &[u8],
        map: &SmallMap,
        df: u64,
        budget: &mut WorkBudget,
    ) -> Result<Option<u16>, IndexError> {
        budget.checkpoint()?;
        check_term(term)?;
        map.validate()?;
        if !(2..=8).contains(&map.len()) || df != width(map.len())? {
            return Err(corrupt("DF differs from short membership"));
        }
        if let Some((start, end)) = self.previous {
            let prior = self
                .payload
                .get(start..end)
                .ok_or_else(|| corrupt("terms are not strictly ordered"))?;
            if prior.cmp(term).is_ge() {
                return Err(corrupt("terms are not strictly ordered"));
            }
        }
        let row_bytes = map
            .len()
            .checked_mul(ROW_BYTES)
            .ok_or(IndexError::ResourceLimited)?;
        let added = term
            .len()
            .checked_add(3)
            .and_then(|bytes| bytes.checked_add(row_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        if self
            .rows
            .checked_add(map.len())
            .ok_or(IndexError::ResourceLimited)?
            > MAX_ROWS
            || self
                .payload
                .len()
                .checked_add(added)
                .ok_or(IndexError::ResourceLimited)?
                > MAX_BYTES
        {
            return Ok(None);
        }
        let mut new_children = [None; 8];
        let mut new_count = 0;
        let mut ids = [0_u16; 8];
        for (slot, id) in ids.iter_mut().enumerate().take(map.len()) {
            let (owner, child) = map
                .entry(slot)?
                .ok_or_else(|| corrupt("owner entry absent"))?;
            let _owner = super::decode_posting_owner(owner)?;
            let index = if let Some(index) = self.children.iter().position(|known| *known == child)
            {
                index
            } else if let Some(index) = new_children
                .get(..new_count)
                .ok_or(IndexError::ResourceLimited)?
                .iter()
                .position(|known| *known == Some(child))
            {
                self.children
                    .len()
                    .checked_add(index)
                    .ok_or(IndexError::ResourceLimited)?
            } else {
                *new_children
                    .get_mut(new_count)
                    .ok_or(IndexError::ResourceLimited)? = Some(child);
                new_count = new_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                self.children
                    .len()
                    .checked_add(new_count)
                    .and_then(|count| count.checked_sub(1))
                    .ok_or(IndexError::ResourceLimited)?
            };
            *id = u16::try_from(index).map_err(|_error| IndexError::ResourceLimited)?;
        }
        let copied = added
            .checked_add(
                new_count
                    .checked_mul(std::mem::size_of::<ObjectRef>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: width(copied)?,
            ..WorkUsage::default()
        })?;
        for child in new_children.into_iter().take(new_count) {
            self.children
                .push(child.ok_or_else(|| corrupt("planned child absent"))?);
        }
        self.payload.extend_from_slice(
            &u16::try_from(term.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        let start = self.payload.len();
        self.payload.extend_from_slice(term);
        self.previous = Some((start, self.payload.len()));
        self.payload
            .push(u8::try_from(df).map_err(|_error| IndexError::ResourceLimited)?);
        for (slot, id) in ids.into_iter().enumerate().take(map.len()) {
            let (owner, _) = map
                .entry(slot)?
                .ok_or_else(|| corrupt("owner entry absent"))?;
            self.payload.extend_from_slice(owner);
            self.payload.extend_from_slice(&id.to_be_bytes());
        }
        let slot = self.terms;
        self.terms = self
            .terms
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.rows = self
            .rows
            .checked_add(map.len())
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Some(slot))
    }

    /// Join ordered final tails only when the same fixed wire bounds permit it.
    /// A declined join leaves both drafts unchanged.
    pub(super) fn try_append(
        &mut self,
        other: &Self,
        budget: &mut WorkBudget,
    ) -> Result<Option<u16>, IndexError> {
        budget.checkpoint()?;
        let added = other
            .payload
            .len()
            .checked_sub(HEADER_BYTES)
            .ok_or_else(|| corrupt("short draft header"))?;
        let rows = self
            .rows
            .checked_add(other.rows)
            .ok_or(IndexError::ResourceLimited)?;
        let end = self
            .payload
            .len()
            .checked_add(added)
            .ok_or(IndexError::ResourceLimited)?;
        if rows > MAX_ROWS || end > MAX_BYTES {
            return Ok(None);
        }
        let (left_start, left_end) = self.previous.ok_or_else(|| corrupt("empty left tail"))?;
        let mut at = HEADER_BYTES;
        let length = usize::from(next_u16(&other.payload, &mut at)?);
        let first = take(&other.payload, &mut at, length)?;
        if self
            .payload
            .get(left_start..left_end)
            .ok_or_else(|| corrupt("tail terms are not strictly ordered"))?
            .cmp(first)
            .is_ge()
        {
            return Err(corrupt("tail terms are not strictly ordered"));
        }
        let (right_start, right_end) = other.previous.ok_or_else(|| corrupt("empty right tail"))?;
        let terms = self
            .terms
            .checked_add(other.terms)
            .ok_or(IndexError::ResourceLimited)?;
        // The fixed remapping table is admitted before its backing is born.
        let _mapping_memory = budget.reserve_memory(
            width(
                MAX_ROWS
                    .checked_mul(std::mem::size_of::<u16>())
                    .ok_or(IndexError::ResourceLimited)?,
            )?,
            MemoryLifetime::Temporary,
        )?;
        let mut mapping = [0_u16; MAX_ROWS];
        let mut new_count = 0;
        for (slot, child) in other.children.iter().enumerate() {
            budget.checkpoint()?;
            let index = if let Some(index) = self.children.iter().position(|known| known == child) {
                index
            } else {
                let index = self
                    .children
                    .len()
                    .checked_add(new_count)
                    .ok_or(IndexError::ResourceLimited)?;
                new_count = new_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                index
            };
            *mapping
                .get_mut(slot)
                .ok_or_else(|| corrupt("child dictionary bound"))? =
                u16::try_from(index).map_err(|_error| IndexError::ResourceLimited)?;
        }
        let child_bytes = new_count
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let remap_bytes = other
            .rows
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        let copied = added
            .checked_add(child_bytes)
            .and_then(|bytes| bytes.checked_add(remap_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: width(copied)?,
            ..WorkUsage::default()
        })?;
        let base_slot = self.terms;
        let base = self
            .payload
            .len()
            .checked_sub(HEADER_BYTES)
            .ok_or_else(|| corrupt("short draft header"))?;
        self.payload.extend_from_slice(
            other
                .payload
                .get(HEADER_BYTES..)
                .ok_or_else(|| corrupt("short draft header"))?,
        );
        for (slot, child) in other.children.iter().enumerate() {
            if usize::from(
                *mapping
                    .get(slot)
                    .ok_or_else(|| corrupt("child dictionary bound"))?,
            ) == self.children.len()
            {
                self.children.push(*child);
            }
        }
        at = HEADER_BYTES;
        for _ in 0..other.terms {
            budget.checkpoint()?;
            let length = usize::from(next_u16(&other.payload, &mut at)?);
            let _term = take(&other.payload, &mut at, length)?;
            let df = usize::from(
                *take(&other.payload, &mut at, 1)?
                    .first()
                    .ok_or_else(|| corrupt("DF absent"))?,
            );
            for _ in 0..df {
                let _owner = take(&other.payload, &mut at, 32)?;
                let destination = base
                    .checked_add(at)
                    .ok_or_else(|| corrupt("joined child offset"))?;
                let slot = usize::from(next_u16(&other.payload, &mut at)?);
                let id = mapping
                    .get(slot)
                    .filter(|_| slot < other.children.len())
                    .ok_or_else(|| corrupt("child slot absent"))?;
                self.payload
                    .get_mut(
                        destination
                            ..destination
                                .checked_add(2)
                                .ok_or_else(|| corrupt("joined child offset"))?,
                    )
                    .ok_or_else(|| corrupt("joined child offset"))?
                    .copy_from_slice(&id.to_be_bytes());
            }
        }
        if at != other.payload.len() {
            return Err(corrupt("tail cardinality differs"));
        }
        self.previous = Some((
            base.checked_add(right_start)
                .ok_or_else(|| corrupt("joined child offset"))?,
            base.checked_add(right_end)
                .ok_or_else(|| corrupt("joined child offset"))?,
        ));
        self.terms = terms;
        self.rows = rows;
        Ok(Some(base_slot))
    }

    pub(super) fn finish(mut self, budget: &mut WorkBudget) -> Result<EncodedBlock, IndexError> {
        if self.terms == 0 || self.rows == 0 || self.children.is_empty() {
            return Err(corrupt("empty block"));
        }
        budget.charge(WorkUsage {
            bytes: 6,
            ..WorkUsage::default()
        })?;
        self.payload
            .get_mut(8..10)
            .ok_or_else(|| corrupt("short draft header"))?
            .copy_from_slice(
                &u16::try_from(self.rows)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
        self.payload
            .get_mut(10..12)
            .ok_or_else(|| corrupt("short draft header"))?
            .copy_from_slice(&self.terms.to_be_bytes());
        self.payload
            .get_mut(12..14)
            .ok_or_else(|| corrupt("short draft header"))?
            .copy_from_slice(
                &u16::try_from(self.children.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_be_bytes(),
            );
        Ok(EncodedBlock {
            input: ObjectBatchInput {
                payload: self.payload,
                children: self.children,
            },
            _memory: self.memory,
        })
    }
}

pub(super) fn locator(reference: ObjectRef, slot: u16, df: u64) -> Result<MapValue, IndexError> {
    if !(2..=8).contains(&df) {
        return Err(corrupt("locator DF differs"));
    }
    let mut data = [0_u8; 11];
    data[0] = INLINE_TAG;
    data[1..3].copy_from_slice(&slot.to_be_bytes());
    data[3..].copy_from_slice(&df.to_be_bytes());
    MapValue::inline(&data, Some(reference))
}

fn take<'a>(payload: &'a [u8], at: &mut usize, count: usize) -> Result<&'a [u8], IndexError> {
    let end = at
        .checked_add(count)
        .ok_or_else(|| corrupt("offset overflow"))?;
    let bytes = payload
        .get(*at..end)
        .ok_or_else(|| corrupt("truncated body"))?;
    *at = end;
    Ok(bytes)
}

fn next_u16(payload: &[u8], at: &mut usize) -> Result<u16, IndexError> {
    Ok(u16::from_be_bytes(
        take(payload, at, 2)?
            .try_into()
            .map_err(|_error| corrupt("integer width"))?,
    ))
}

/// Validate the complete term/owner/dictionary stream before any hit escapes.
pub(super) fn read(
    object: &StoredObject,
    reference: ObjectRef,
    expected_term: &[u8],
    slot: u16,
    expected_df: u64,
    budget: &mut WorkBudget,
) -> Result<SmallMap, IndexError> {
    budget.checkpoint()?;
    check_term(expected_term)?;
    let payload = object.payload.as_slice();
    if !(HEADER_BYTES..=MAX_BYTES).contains(&payload.len())
        || payload.get(..8) != Some(MAGIC.as_slice())
    {
        return Err(corrupt("magic or payload bound differs"));
    }
    let _memory = budget.reserve_memory(width(MAX_ROWS)?, MemoryLifetime::Temporary)?;
    let mut at = 8;
    let rows = usize::from(next_u16(payload, &mut at)?);
    let terms = next_u16(payload, &mut at)?;
    let children = usize::from(next_u16(payload, &mut at)?);
    let reserved = next_u16(payload, &mut at)?;
    if rows == 0
        || rows > MAX_ROWS
        || terms == 0
        || usize::from(terms) > rows.div_euclid(2)
        || children == 0
        || children > rows
        || reserved != 0
        || slot >= terms
        || !(2..=8).contains(&expected_df)
        || object.registration.reference != reference
        || object.registration.children.len() != children
    {
        return Err(corrupt("header, locator or registration differs"));
    }
    let child_bytes = children
        .checked_mul(32)
        .ok_or(IndexError::ResourceLimited)?;
    let wire_bytes = 16_usize
        .checked_add(payload.len())
        .and_then(|bytes| bytes.checked_add(child_bytes))
        .ok_or(IndexError::ResourceLimited)?;
    if object.registration.bytes != width(wire_bytes)? {
        return Err(corrupt("registered wire length differs"));
    }
    budget.charge(WorkUsage {
        bytes: width(
            payload
                .len()
                .checked_add(child_bytes)
                .ok_or(IndexError::ResourceLimited)?,
        )?,
        ..WorkUsage::default()
    })?;
    for (index, child) in object.registration.children.iter().enumerate() {
        budget.checkpoint()?;
        if object
            .registration
            .children
            .iter()
            .take(index)
            .any(|known| known == child)
        {
            return Err(corrupt("child dictionary repeats identity"));
        }
    }
    let mut seen = [false; MAX_ROWS];
    let mut used_children = 0;
    let mut previous_term: Option<&[u8]> = None;
    let mut found = SmallMap::default();
    let mut counted_rows = 0_usize;
    for current in 0..terms {
        budget.checkpoint()?;
        let length = usize::from(next_u16(payload, &mut at)?);
        let term = take(payload, &mut at, length)?;
        check_term(term)?;
        if previous_term.is_some_and(|previous| previous >= term) {
            return Err(corrupt("term order differs"));
        }
        previous_term = Some(term);
        let df = usize::from(
            *take(payload, &mut at, 1)?
                .first()
                .ok_or_else(|| corrupt("DF absent"))?,
        );
        if !(2..=8).contains(&df) {
            return Err(corrupt("entry DF exceeds short bound"));
        }
        if current == slot && (term != expected_term || width(df)? != expected_df) {
            return Err(corrupt("slot term or DF binding differs"));
        }
        let mut previous_owner: Option<&[u8]> = None;
        for _ in 0..df {
            let owner = take(payload, &mut at, 32)?;
            let _owner = super::decode_posting_owner(owner)?;
            if previous_owner.is_some_and(|previous| previous >= owner) {
                return Err(corrupt("owner order differs"));
            }
            previous_owner = Some(owner);
            let child = usize::from(next_u16(payload, &mut at)?);
            let reference = object
                .registration
                .children
                .get(child)
                .copied()
                .ok_or_else(|| corrupt("child slot exceeds dictionary"))?;
            let visited = seen
                .get_mut(child)
                .ok_or_else(|| corrupt("child slot exceeds bound"))?;
            if !*visited {
                if child != used_children {
                    return Err(corrupt("child first-use order differs"));
                }
                *visited = true;
                used_children = used_children
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            if current == slot && !found.push(owner, reference)? {
                return Err(corrupt("selected membership exceeds bound"));
            }
        }
        counted_rows = counted_rows
            .checked_add(df)
            .ok_or(IndexError::ResourceLimited)?;
    }
    if at != payload.len()
        || counted_rows != rows
        || used_children != children
        || width(found.len())? != expected_df
    {
        return Err(corrupt("row count, unused child or trailing bytes differ"));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{Digest, RegisteredObject, WorkLimits};
    use std::time::{Duration, Instant};

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 64 * 1024 * 1024,
                mutations: 1_000_000,
            },
            Instant::now() + Duration::from_secs(60),
        )
    }

    fn reference(id: u8) -> ObjectRef {
        ObjectRef(Digest([id; 32]))
    }

    fn owner(id: u8) -> [u8; 32] {
        let mut key = [7; 32];
        key[16..].fill(id);
        key
    }

    fn membership() -> SmallMap {
        let mut map = SmallMap::default();
        assert!(map.push(&owner(1), reference(11)).unwrap());
        assert!(map.push(&owner(2), reference(12)).unwrap());
        map
    }

    // Independently assembled wire: 4 owner rows, 2 distinct full case keys,
    // 2 children in first-use order. Both keys select the same membership.
    fn golden() -> Vec<u8> {
        let mut bytes = b"QITXSP01\0\x04\0\x02\0\x02\0\0".to_vec();
        for term in [b"\0alpha", b"\x01alpha"] {
            bytes.extend_from_slice(&[0, 6]);
            bytes.extend_from_slice(term);
            bytes.push(2);
            bytes.extend_from_slice(&[7; 16]);
            bytes.extend_from_slice(&[1; 16]);
            bytes.extend_from_slice(&[0, 0]);
            bytes.extend_from_slice(&[7; 16]);
            bytes.extend_from_slice(&[2; 16]);
            bytes.extend_from_slice(&[0, 1]);
        }
        assert_eq!(bytes.len(), 170);
        bytes
    }

    fn object(payload: Vec<u8>, children: Vec<ObjectRef>) -> StoredObject {
        let bytes = u64::try_from(16 + payload.len() + 32 * children.len()).unwrap();
        StoredObject::new(
            RegisteredObject::from_parts(reference(9), bytes, children, None).unwrap(),
            payload,
            None,
        )
    }

    #[test]
    fn encoder_matches_independent_wire_and_both_case_slots() -> Result<(), IndexError> {
        let mut work = budget();
        let mut draft = Draft::new(&mut work)?;
        assert_eq!(
            draft.push(b"\0alpha", &membership(), 2, &mut work)?,
            Some(0)
        );
        assert_eq!(
            draft.push(b"\x01alpha", &membership(), 2, &mut work)?,
            Some(1)
        );
        let encoded = draft.finish(&mut work)?;
        assert_eq!(encoded.input.payload, golden());
        assert_eq!(encoded.input.children, vec![reference(11), reference(12)]);
        let stored = object(encoded.input.payload, encoded.input.children);
        for (slot, key) in [(0, b"\0alpha".as_slice()), (1, b"\x01alpha".as_slice())] {
            assert_eq!(
                read(&stored, reference(9), key, slot, 2, &mut work)?,
                membership()
            );
        }
        let value = locator(reference(9), 1, 2)?;
        let MapValue::Inline(inline) = value else {
            panic!("locator must stay inline")
        };
        assert_eq!(inline.data()?, &[3, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2]);
        assert_eq!(inline.children()?, &[reference(9)]);
        Ok(())
    }

    #[test]
    fn foreign_term_slot_df_and_registration_are_refused() {
        let stored = object(golden(), vec![reference(11), reference(12)]);
        for (key, slot, df, target) in [
            (b"\0alpha".as_slice(), 1, 2, reference(9)),
            (b"\0omega".as_slice(), 0, 2, reference(9)),
            (b"\0alpha".as_slice(), 0, 3, reference(9)),
            (b"\0alpha".as_slice(), 2, 2, reference(9)),
            (b"\0alpha".as_slice(), 0, 2, reference(10)),
        ] {
            assert!(matches!(
                read(&stored, target, key, slot, df, &mut budget()),
                Err(IndexError::CorruptObject(_))
            ));
        }
        for df in [0, 1, 9, u64::MAX] {
            assert!(locator(reference(9), 0, df).is_err());
        }
    }

    #[test]
    fn full_stream_truncation_order_and_child_dictionary_are_checked() {
        let expected = golden();
        for end in 0..expected.len() {
            let stored = object(expected[..end].to_vec(), vec![reference(11), reference(12)]);
            assert!(
                read(&stored, reference(9), b"\0alpha", 0, 2, &mut budget()).is_err(),
                "truncation {end}"
            );
        }
        for mode in 0..9 {
            let mut bytes = golden();
            match mode {
                0 => bytes[14] = 1,
                1 => bytes[9] = 5,
                2 => bytes[24] = 1,
                3 => bytes[58] = 1,
                4 => bytes[58] = 2,
                5 => {
                    let first = bytes[25..57].to_vec();
                    bytes[59..91].copy_from_slice(&first);
                }
                6 => {
                    bytes[92] = 0;
                    bytes[169] = 0;
                }
                7 => bytes[95] = 0,
                _ => bytes.push(0),
            }
            let stored = object(bytes, vec![reference(11), reference(12)]);
            assert!(
                matches!(
                    read(&stored, reference(9), b"\0alpha", 0, 2, &mut budget()),
                    Err(IndexError::CorruptObject(_))
                ),
                "mutation {mode}"
            );
        }
        let duplicate = object(golden(), vec![reference(11), reference(11)]);
        assert!(read(&duplicate, reference(9), b"\0alpha", 0, 2, &mut budget()).is_err());
    }

    #[test]
    fn ordered_tail_join_remaps_children_at_exact_wire_bound_and_declines_one_byte_over()
    -> Result<(), IndexError> {
        for extra in [0, 1] {
            let mut work = budget();
            let mut left = Draft::new(&mut work)?;
            let mut right = Draft::new(&mut work)?;
            let mut keys = Vec::new();
            let mut memberships = Vec::new();
            for surface in 0..=1_u8 {
                let mut map = SmallMap::default();
                assert!(map.push(&owner(1), reference(11 + surface))?);
                assert!(map.push(&owner(2), reference(12 + surface))?);
                for slot in 0..8_u8 {
                    // 16 + 16 * (952 term bytes + 3 metadata + 68 rows) = 16384.
                    let mut key = vec![b'x'; 952 + usize::from(surface == 1 && slot == 7) * extra];
                    key[0] = surface;
                    key[1] = b'a' + slot;
                    let draft = if surface == 0 { &mut left } else { &mut right };
                    assert_eq!(draft.push(&key, &map, 2, &mut work)?, Some(u16::from(slot)));
                    keys.push(key);
                    memberships.push(map);
                }
            }
            let before_left = (
                left.payload.clone(),
                left.children.clone(),
                left.rows,
                left.terms,
                left.previous,
            );
            let before_right = (
                right.payload.clone(),
                right.children.clone(),
                right.rows,
                right.terms,
                right.previous,
            );
            let joined = left.try_append(&right, &mut work)?;
            if extra == 0 {
                assert_eq!(joined, Some(8));
                let encoded = left.finish(&mut work)?;
                assert_eq!(encoded.input.payload.len(), 16384);
                assert_eq!(
                    encoded.input.children,
                    vec![reference(11), reference(12), reference(13)]
                );
                let stored = object(encoded.input.payload, encoded.input.children);
                for (slot, (key, map)) in keys.iter().zip(&memberships).enumerate() {
                    assert_eq!(
                        read(
                            &stored,
                            reference(9),
                            key,
                            u16::try_from(slot).unwrap(),
                            2,
                            &mut work
                        )?,
                        *map
                    );
                }
            } else {
                assert_eq!(joined, None);
                assert_eq!(
                    (
                        left.payload.clone(),
                        left.children.clone(),
                        left.rows,
                        left.terms,
                        left.previous
                    ),
                    before_left
                );
                assert_eq!(
                    (
                        right.payload.clone(),
                        right.children.clone(),
                        right.rows,
                        right.terms,
                        right.previous
                    ),
                    before_right
                );
                for (surface, draft) in [left, right].into_iter().enumerate() {
                    let encoded = draft.finish(&mut work)?;
                    let stored = object(encoded.input.payload, encoded.input.children);
                    for slot in 0..8 {
                        let index = surface * 8 + slot;
                        assert_eq!(
                            read(
                                &stored,
                                reference(9),
                                &keys[index],
                                u16::try_from(slot).unwrap(),
                                2,
                                &mut work
                            )?,
                            memberships[index]
                        );
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn maximum_term_and_fixed_block_boundary_are_deterministic() -> Result<(), IndexError> {
        let mut work = budget();
        let mut longest = vec![b'x'; 1024];
        longest[0] = 0;
        let mut draft = Draft::new(&mut work)?;
        assert_eq!(draft.push(&longest, &membership(), 2, &mut work)?, Some(0));
        let encoded = draft.finish(&mut work)?;
        let stored = object(encoded.input.payload, encoded.input.children);
        assert_eq!(
            read(&stored, reference(9), &longest, 0, 2, &mut work)?,
            membership()
        );
        longest.push(b'x');
        assert!(
            Draft::new(&mut work)?
                .push(&longest, &membership(), 2, &mut work)
                .is_err()
        );

        let mut draft = Draft::new(&mut work)?;
        for slot in 0..215_u16 {
            let term = format!("\0t{slot:03}");
            assert_eq!(
                draft.push(term.as_bytes(), &membership(), 2, &mut work)?,
                Some(slot)
            );
        }
        let before_payload = draft.payload.clone();
        let before_children = draft.children.clone();
        assert_eq!(draft.push(b"\0t215", &membership(), 2, &mut work)?, None);
        assert_eq!(draft.payload, before_payload);
        assert_eq!(draft.children, before_children);
        let encoded = draft.finish(&mut work)?;
        assert_eq!(encoded.input.payload.len(), 16_356);
        let stored = object(encoded.input.payload, encoded.input.children);
        assert_eq!(
            read(&stored, reference(9), b"\0t214", 214, 2, &mut work)?,
            membership()
        );
        Ok(())
    }
}
