//! Bounded immutable pages for the current text term dictionary.
//!
//! The persistent terms map stores each page under its greatest full case key.
//! The page owns the exact posting value for every key in that interval. This
//! wire belongs to the text output policy; it is never a scratch SQL format.

use super::{DENSE_POSTING_TAG, small_posting_block};
use crate::native::{text_memory::PreparedKey, text_spool::check_term_key};
use quanta_index_core::index::{
    IndexError, MapPage, MapRoot, MapValue, MemoryLease, MemoryLifetime,
    NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1, ObjectBatchInput, ObjectRef, StoredObject, WorkBudget,
    WorkUsage,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

pub(super) const MAX_PAGE_BYTES: usize = 16 * 1024;
const HEADER_BYTES: usize = 16;
// The shortest valid row is key length (2), case-prefixed key (2), and
// kind/data-length/child-slot (3). The payload bound, not a batch width,
// limits page cardinality.
pub(super) const MAX_PAGE_ROWS: usize = (MAX_PAGE_BYTES - HEADER_BYTES).div_euclid(7);
const MAX_CHILDREN: usize = 255;
const CHILD_INDEX_SLOTS: usize = 512;
const EMPTY_CHILD_SLOT: u8 = u8::MAX;
pub(super) const LAYOUT_POLICY: &[u8] =
    b"QITXTD01;be16-count;u8-children;sorted-unique-fullcasekeys;first-use-children;bytefull16384-child255";
const MAGIC: &[u8; 8] = b"QITXTD01";
const OBJECT: u8 = 0;
const INLINE: u8 = 1;

fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(format!("text term dictionary: {reason}"))
}

fn width(value: usize) -> Result<u64, IndexError> {
    u64::try_from(value).map_err(|_error| IndexError::ResourceLimited)
}

/// Open addressed, deterministic first-use child index. Slot equality always
/// checks the complete reference, including after a hash collision.
struct ChildIndex {
    slots: [u8; CHILD_INDEX_SLOTS],
    _memory: Option<MemoryLease>,
}

impl ChildIndex {
    fn new(budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(width(CHILD_INDEX_SLOTS)?, MemoryLifetime::Temporary)?;
        budget.charge(WorkUsage {
            bytes: width(CHILD_INDEX_SLOTS)?,
            ..WorkUsage::default()
        })?;
        Ok(Self {
            slots: [EMPTY_CHILD_SLOT; CHILD_INDEX_SLOTS],
            _memory: memory,
        })
    }

    fn hash(reference: ObjectRef, budget: &mut WorkBudget) -> Result<usize, IndexError> {
        budget.charge(WorkUsage {
            bytes: 32,
            ..WorkUsage::default()
        })?;
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in reference.0.0 {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        usize::try_from(hash & 511).map_err(|_error| IndexError::ResourceLimited)
    }

    /// Return the existing child slot, or the vacant table index for a new
    /// child. The caller commits the vacant slot only after row admission.
    fn find(
        &self,
        reference: ObjectRef,
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<(Option<usize>, usize), IndexError> {
        let start = Self::hash(reference, budget)?;
        for probe in 0..CHILD_INDEX_SLOTS {
            budget.checkpoint()?;
            let index = start
                .checked_add(probe)
                .ok_or(IndexError::ResourceLimited)?
                & (CHILD_INDEX_SLOTS - 1);
            budget.charge(WorkUsage {
                bytes: 1,
                ..WorkUsage::default()
            })?;
            let slot = *self.slots.get(index).ok_or(IndexError::ResourceLimited)?;
            if slot == EMPTY_CHILD_SLOT {
                return Ok((None, index));
            }
            let stored = children
                .get(usize::from(slot))
                .ok_or_else(|| corrupt("child index slot out of bounds"))?;
            budget.charge(WorkUsage {
                bytes: 32,
                ..WorkUsage::default()
            })?;
            if *stored == reference {
                return Ok((Some(usize::from(slot)), index));
            }
        }
        Err(corrupt("child index has no vacant slot"))
    }

    fn insert(&mut self, index: usize, slot: usize) -> Result<(), IndexError> {
        let cell = self
            .slots
            .get_mut(index)
            .ok_or(IndexError::ResourceLimited)?;
        if *cell != EMPTY_CHILD_SLOT || slot >= MAX_CHILDREN {
            return Err(corrupt("child index insertion differs"));
        }
        *cell = u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?;
        Ok(())
    }
}

fn posting_value_parts(value: &MapValue) -> Result<(u8, &[u8], ObjectRef), IndexError> {
    match value {
        MapValue::Object(reference) => Ok((OBJECT, &[], *reference)),
        MapValue::Inline(inline) => {
            let child = inline
                .single_child()?
                .ok_or_else(|| corrupt("inline posting lacks child"))?;
            let data = inline.data()?;
            let valid = matches!(data.len(), 8 | 32)
                || (data.len() == 11 && data.first() == Some(&small_posting_block::INLINE_TAG))
                || (data.len() == 9 && data.first() == Some(&DENSE_POSTING_TAG));
            if !valid {
                return Err(corrupt("unsupported inline posting shape"));
            }
            Ok((INLINE, data, child))
        }
    }
}

/// A page is staged in already admitted Original backing. A capacity miss is
/// the only `false` result; invalid input and budget refusal remain errors.
pub(super) struct PageDraft {
    bytes: Vec<u8>,
    children: Vec<ObjectRef>,
    child_index: ChildIndex,
    last_key: Option<(usize, usize)>,
    rows: usize,
    // Both Vec allocations die before their grants.
    payload_memory: Option<MemoryLease>,
    children_memory: Option<MemoryLease>,
}

impl PageDraft {
    pub(super) fn new(budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let payload_memory =
            budget.reserve_memory(width(MAX_PAGE_BYTES)?, MemoryLifetime::Temporary)?;
        let children_memory = budget.reserve_memory(
            width(
                MAX_CHILDREN
                    .checked_mul(std::mem::size_of::<ObjectRef>())
                    .ok_or(IndexError::ResourceLimited)?,
            )?,
            MemoryLifetime::Temporary,
        )?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(MAX_PAGE_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(MAX_CHILDREN)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if bytes.capacity() > MAX_PAGE_BYTES || children.capacity() > MAX_CHILDREN {
            return Err(IndexError::ResourceLimited);
        }
        let child_index = ChildIndex::new(budget)?;
        budget.charge(WorkUsage {
            bytes: width(HEADER_BYTES)?,
            ..WorkUsage::default()
        })?;
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&[0; HEADER_BYTES - MAGIC.len()]);
        Ok(Self {
            bytes,
            children,
            child_index,
            last_key: None,
            rows: 0,
            payload_memory,
            children_memory,
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.rows == 0
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.rows
    }

    pub(super) fn try_push(
        &mut self,
        key: &[u8],
        value: MapValue,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        check_term_key(key)?;
        let (kind, data, child) = posting_value_parts(&value)?;
        if let Some((start, end)) = self.last_key {
            let prior = self
                .bytes
                .get(start..end)
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes: width(prior.len().min(key.len()))?,
                ..WorkUsage::default()
            })?;
            if prior >= key {
                return Err(corrupt("keys are not strictly increasing"));
            }
        }
        let (selected, vacant_index) = self.child_index.find(child, &self.children, budget)?;
        let fresh_child = selected.is_none();
        let slot = selected.unwrap_or(self.children.len());
        let row_bytes = 2_usize
            .checked_add(key.len())
            .and_then(|bytes| bytes.checked_add(3))
            .and_then(|bytes| bytes.checked_add(data.len()))
            .ok_or(IndexError::ResourceLimited)?;
        let end = self
            .bytes
            .len()
            .checked_add(row_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        if self.rows == MAX_PAGE_ROWS
            || end > MAX_PAGE_BYTES
            || (fresh_child && self.children.len() == MAX_CHILDREN)
        {
            return Ok(false);
        }
        budget.charge(WorkUsage {
            bytes: width(
                row_bytes
                    .checked_add(if fresh_child { 33 } else { 0 })
                    .ok_or(IndexError::ResourceLimited)?,
            )?,
            ..WorkUsage::default()
        })?;
        let key_len = u16::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        self.bytes.extend_from_slice(&key_len.to_be_bytes());
        let start = self.bytes.len();
        self.bytes.extend_from_slice(key);
        let key_end = self.bytes.len();
        self.bytes.push(kind);
        self.bytes
            .push(u8::try_from(data.len()).map_err(|_error| IndexError::ResourceLimited)?);
        self.bytes.extend_from_slice(data);
        self.bytes
            .push(u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?);
        if fresh_child {
            self.child_index.insert(vacant_index, slot)?;
            self.children.push(child);
        }
        self.rows = self
            .rows
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.last_key = Some((start, key_end));
        Ok(true)
    }

    pub(super) fn finish(mut self, budget: &mut WorkBudget) -> Result<SealedPage, IndexError> {
        let (start, end) = self.last_key.ok_or_else(|| corrupt("empty page"))?;
        let fence = PreparedKey::copy(
            self.bytes
                .get(start..end)
                .ok_or(IndexError::ResourceLimited)?,
            budget,
        )?;
        budget.charge(WorkUsage {
            bytes: width(
                fence
                    .as_slice()
                    .len()
                    .checked_add(3)
                    .ok_or(IndexError::ResourceLimited)?,
            )?,
            ..WorkUsage::default()
        })?;
        let count = u16::try_from(self.rows).map_err(|_error| IndexError::ResourceLimited)?;
        self.bytes
            .get_mut(8..10)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(&count.to_be_bytes());
        *self.bytes.get_mut(10).ok_or(IndexError::ResourceLimited)? =
            u8::try_from(self.children.len()).map_err(|_error| IndexError::ResourceLimited)?;
        Ok(SealedPage {
            input: ObjectBatchInput {
                payload: self.bytes,
                children: self.children,
            },
            fence,
            payload_memory: self.payload_memory,
            children_memory: self.children_memory,
        })
    }
}

/// Keep both object inputs and their Original grants alive through `put_batch`.
pub(super) struct SealedPage {
    pub(super) input: ObjectBatchInput,
    pub(super) fence: PreparedKey,
    payload_memory: Option<MemoryLease>,
    children_memory: Option<MemoryLease>,
}

impl SealedPage {
    /// Move the already encoded page into a bounded install packet. The
    /// caller retains both Original grants until its input DATA is dropped.
    pub(super) fn into_parts(
        self,
    ) -> (
        ObjectBatchInput,
        PreparedKey,
        Option<MemoryLease>,
        Option<MemoryLease>,
    ) {
        (
            self.input,
            self.fence,
            self.payload_memory,
            self.children_memory,
        )
    }
}

#[derive(Clone, Copy)]
struct Row<'a> {
    key: &'a [u8],
    kind: u8,
    data: &'a [u8],
    slot: usize,
    end: usize,
}

fn take<'a>(payload: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8], IndexError> {
    let end = offset.checked_add(len).ok_or(IndexError::ResourceLimited)?;
    let slice = payload
        .get(*offset..end)
        .ok_or_else(|| corrupt("truncated row"))?;
    *offset = end;
    Ok(slice)
}

fn read_row(payload: &[u8], start: usize) -> Result<Row<'_>, IndexError> {
    let mut offset = start;
    let len: [u8; 2] = take(payload, &mut offset, 2)?
        .try_into()
        .map_err(|_error| corrupt("key length width"))?;
    let key = take(payload, &mut offset, usize::from(u16::from_be_bytes(len)))?;
    let kind = *take(payload, &mut offset, 1)?
        .first()
        .ok_or_else(|| corrupt("kind absent"))?;
    let data_len = usize::from(
        *take(payload, &mut offset, 1)?
            .first()
            .ok_or_else(|| corrupt("data length absent"))?,
    );
    let data = take(payload, &mut offset, data_len)?;
    let slot = usize::from(
        *take(payload, &mut offset, 1)?
            .first()
            .ok_or_else(|| corrupt("child slot absent"))?,
    );
    if (kind == OBJECT && data_len != 0) || (kind != OBJECT && kind != INLINE) {
        return Err(corrupt("posting kind or data width differs"));
    }
    if kind == INLINE {
        let valid = matches!(data_len, 8 | 32)
            || (data_len == 11 && data.first() == Some(&small_posting_block::INLINE_TAG))
            || (data_len == 9 && data.first() == Some(&DENSE_POSTING_TAG));
        if !valid {
            return Err(corrupt("unsupported inline posting shape"));
        }
    }
    Ok(Row {
        key,
        kind,
        data,
        slot,
        end: offset,
    })
}

/// Parsed row offsets borrow the authenticated stored object; no row key or
/// posting value is copied into a second page-sized buffer.
pub(super) struct CheckedPage {
    object: StoredObject,
    offsets: Vec<u16>,
    count: usize,
    _memory: Option<MemoryLease>,
}

impl CheckedPage {
    pub(super) fn reference(&self) -> ObjectRef {
        self.object.registration.reference
    }

    pub(super) fn parse(
        object: StoredObject,
        expected: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let payload = object.payload.as_slice();
        let payload_len = payload.len();
        let wire_len = NATIVE_IMMUTABLE_OBJECT_HEADER_BYTES_V1
            .checked_add(
                object
                    .registration
                    .children
                    .len()
                    .checked_mul(32)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .and_then(|bytes| bytes.checked_add(payload_len))
            .ok_or(IndexError::ResourceLimited)?;
        if object.registration.reference != expected
            || object.registration.bytes != width(wire_len)?
            || !(HEADER_BYTES..=MAX_PAGE_BYTES).contains(&payload_len)
            || payload.get(..8) != Some(MAGIC.as_slice())
        {
            return Err(corrupt("object identity or envelope differs"));
        }
        let count = usize::from(u16::from_be_bytes(
            payload
                .get(8..10)
                .ok_or_else(|| corrupt("count absent"))?
                .try_into()
                .map_err(|_error| corrupt("count width"))?,
        ));
        let children = usize::from(
            *payload
                .get(10)
                .ok_or_else(|| corrupt("child count absent"))?,
        );
        if !(1..=MAX_PAGE_ROWS).contains(&count)
            || count
                .checked_mul(7)
                .and_then(|bytes| bytes.checked_add(HEADER_BYTES))
                .is_none_or(|bytes| bytes > payload_len)
        {
            return Err(corrupt("row count exceeds page wire"));
        }
        if !(1..=count).contains(&children)
            || children > MAX_CHILDREN
            || children != object.registration.children.len()
            || payload.get(11..HEADER_BYTES) != Some(&[0; HEADER_BYTES - 11][..])
        {
            return Err(corrupt("header or child count differs"));
        }
        let backing = count
            .checked_mul(std::mem::size_of::<u16>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            width(
                std::mem::size_of::<Self>()
                    .checked_add(backing)
                    .ok_or(IndexError::ResourceLimited)?,
            )?,
            MemoryLifetime::Temporary,
        )?;
        let mut offsets = Vec::new();
        offsets
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if offsets.capacity() > count {
            return Err(IndexError::ResourceLimited);
        }
        let mut child_index = ChildIndex::new(budget)?;
        let mut checked = Self {
            object,
            offsets,
            count,
            _memory: memory,
        };
        budget.charge(WorkUsage {
            bytes: width(payload_len)?,
            ..WorkUsage::default()
        })?;
        let mut offset = HEADER_BYTES;
        let mut prior_key: Option<&[u8]> = None;
        let mut next_child = 0usize;
        for _ordinal in 0..count {
            budget.checkpoint()?;
            let row = read_row(checked.object.payload.as_slice(), offset)?;
            check_term_key(row.key)?;
            if let Some(prior) = prior_key {
                budget.charge(WorkUsage {
                    bytes: width(prior.len().min(row.key.len()))?,
                    ..WorkUsage::default()
                })?;
                if prior >= row.key {
                    return Err(corrupt("keys are not strictly increasing"));
                }
            }
            if row.slot > next_child || row.slot >= children {
                return Err(corrupt("child slot skips first use"));
            }
            if row.slot == next_child {
                let child = checked
                    .object
                    .registration
                    .children
                    .get(row.slot)
                    .ok_or_else(|| corrupt("child absent"))?;
                let (duplicate, vacant) = child_index.find(
                    *child,
                    checked
                        .object
                        .registration
                        .children
                        .get(..row.slot)
                        .ok_or(IndexError::ResourceLimited)?,
                    budget,
                )?;
                if duplicate.is_some() {
                    return Err(corrupt("duplicate child slot"));
                }
                budget.charge(WorkUsage {
                    bytes: 1,
                    ..WorkUsage::default()
                })?;
                child_index.insert(vacant, row.slot)?;
                next_child = next_child
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            budget.charge(WorkUsage {
                bytes: 2,
                ..WorkUsage::default()
            })?;
            checked
                .offsets
                .push(u16::try_from(offset).map_err(|_error| IndexError::ResourceLimited)?);
            prior_key = Some(row.key);
            offset = row.end;
        }
        if offset != checked.object.payload.len() || next_child != children {
            return Err(corrupt("page has trailing bytes or unused children"));
        }
        budget.checkpoint()?;
        Ok(checked)
    }

    pub(super) fn len(&self) -> usize {
        self.count
    }

    #[cfg(test)]
    pub(super) fn entry(&self, ordinal: usize) -> Result<(&[u8], MapValue), IndexError> {
        let start = usize::from(
            *self
                .offsets
                .get(ordinal)
                .filter(|_| ordinal < self.count)
                .ok_or(IndexError::ResourceLimited)?,
        );
        self.entry_at(start)
    }

    /// Charge the actual second row traversal made by the COW writer. Parse
    /// has already authenticated every offset and charged its first pass.
    pub(super) fn entry_metered(
        &self,
        ordinal: usize,
        budget: &mut WorkBudget,
    ) -> Result<(&[u8], MapValue), IndexError> {
        let start = usize::from(
            *self
                .offsets
                .get(ordinal)
                .filter(|_| ordinal < self.count)
                .ok_or(IndexError::ResourceLimited)?,
        );
        let next_ordinal = ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        let (end, offset_bytes) = if next_ordinal < self.count {
            (
                usize::from(
                    *self
                        .offsets
                        .get(next_ordinal)
                        .ok_or(IndexError::ResourceLimited)?,
                ),
                4_usize,
            )
        } else {
            (self.object.payload.len(), 2_usize)
        };
        let bytes = end
            .checked_sub(start)
            .and_then(|bytes| bytes.checked_add(32))
            .and_then(|bytes| bytes.checked_add(offset_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: width(bytes)?,
            ..WorkUsage::default()
        })?;
        self.entry_at(start)
    }

    fn entry_at(&self, start: usize) -> Result<(&[u8], MapValue), IndexError> {
        let row = read_row(self.object.payload.as_slice(), start)?;
        let child = *self
            .object
            .registration
            .children
            .get(row.slot)
            .ok_or_else(|| corrupt("child slot out of bounds"))?;
        let value = if row.kind == OBJECT {
            MapValue::Object(child)
        } else {
            MapValue::inline(row.data, Some(child))
                .map_err(|_error| corrupt("inline posting value differs"))?
        };
        Ok((row.key, value))
    }

    /// Project only the authenticated key. The page parser already proved the
    /// row bounds; lookup does not need to reparse its value on every probe.
    fn key_metered(&self, ordinal: usize, budget: &mut WorkBudget) -> Result<&[u8], IndexError> {
        budget.charge(WorkUsage {
            bytes: 4, // offset and serialized key length
            ..WorkUsage::default()
        })?;
        let start = usize::from(
            *self
                .offsets
                .get(ordinal)
                .filter(|_| ordinal < self.count)
                .ok_or(IndexError::ResourceLimited)?,
        );
        let length = usize::from(u16::from_be_bytes(
            self.object
                .payload
                .get(start..start.checked_add(2).ok_or(IndexError::ResourceLimited)?)
                .ok_or_else(|| corrupt("key length is truncated"))?
                .try_into()
                .map_err(|_error| corrupt("key length width differs"))?,
        ));
        budget.charge(WorkUsage {
            bytes: width(length)?,
            ..WorkUsage::default()
        })?;
        self.object
            .payload
            .get(
                start.checked_add(2).ok_or(IndexError::ResourceLimited)?
                    ..start
                        .checked_add(2)
                        .and_then(|start| start.checked_add(length))
                        .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or_else(|| corrupt("key is truncated"))
    }

    pub(super) fn first_key_metered(&self, budget: &mut WorkBudget) -> Result<&[u8], IndexError> {
        self.key_metered(0, budget)
    }

    pub(super) fn last_key_metered(&self, budget: &mut WorkBudget) -> Result<&[u8], IndexError> {
        self.key_metered(
            self.count
                .checked_sub(1)
                .ok_or(IndexError::ResourceLimited)?,
            budget,
        )
    }

    #[cfg(test)]
    pub(super) fn first_key(&self) -> Result<&[u8], IndexError> {
        self.entry(0).map(|(key, _)| key)
    }
    #[cfg(test)]
    pub(super) fn last_key(&self) -> Result<&[u8], IndexError> {
        self.entry(
            self.count
                .checked_sub(1)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map(|(key, _)| key)
    }
    #[cfg(test)]
    pub(super) fn validate_fence(&self, fence: &[u8]) -> Result<(), IndexError> {
        if self.last_key()? != fence {
            return Err(corrupt("page upper fence differs"));
        }
        Ok(())
    }

    pub(super) fn lookup(
        &self,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        check_term_key(key)?;
        let mut lower = 0usize;
        let mut upper = self.count;
        while lower < upper {
            budget.checkpoint()?;
            let middle = lower
                .checked_add(
                    upper
                        .checked_sub(lower)
                        .and_then(|range| range.checked_div(2))
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
            let candidate = self.key_metered(middle, budget)?;
            budget.charge(WorkUsage {
                bytes: width(candidate.len().min(key.len()))?,
                ..WorkUsage::default()
            })?;
            match candidate.cmp(key) {
                std::cmp::Ordering::Less => {
                    lower = middle.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
                std::cmp::Ordering::Greater => upper = middle,
                std::cmp::Ordering::Equal => {
                    return self
                        .entry_metered(middle, budget)
                        .map(|(_, value)| Some(value));
                }
            }
        }
        Ok(None)
    }
}

fn scan_one(
    maps: &dyn PersistentMapPort,
    root: MapRoot,
    after: &[u8],
    budget: &mut WorkBudget,
) -> Result<MapPage, IndexError> {
    let page = maps.scan(root, Some(after), 1, budget)?;
    if page.entries.len() > 1 || (page.entries.is_empty() && page.has_more) {
        return Err(IndexError::BackendViolation(
            "term dictionary scan page exceeds one row".into(),
        ));
    }
    if let Some((fence, _)) = page.entries.first() {
        budget.charge(WorkUsage {
            bytes: width(fence.len().min(after.len()))?,
            ..WorkUsage::default()
        })?;
        if fence.as_slice() <= after {
            return Err(IndexError::BackendViolation(
                "term dictionary scan did not advance".into(),
            ));
        }
    }
    Ok(page)
}

fn checked_at(
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    root: MapRoot,
    fence: &[u8],
    value: MapValue,
    random: bool,
    previous: Option<&[u8]>,
    budget: &mut WorkBudget,
) -> Result<CheckedPage, IndexError> {
    let reference = value
        .object_ref()
        .map_err(|_error| corrupt("page map value is not an object"))?;
    let checked = CheckedPage::parse(objects.get(reference, budget)?, reference, budget)?;
    let last = checked.last_key_metered(budget)?;
    budget.charge(WorkUsage {
        bytes: width(last.len().min(fence.len()))?,
        ..WorkUsage::default()
    })?;
    if last != fence {
        return Err(corrupt("page upper fence differs"));
    }
    let first = if checked.len() == 1 {
        last
    } else {
        checked.first_key_metered(budget)?
    };
    if let Some(prior) = previous {
        budget.charge(WorkUsage {
            bytes: width(first.len().min(prior.len()))?,
            ..WorkUsage::default()
        })?;
        if first <= prior {
            return Err(corrupt("page crosses previous upper fence"));
        }
    } else if random {
        // A random ceiling must be the first map fence strictly above the
        // page's first key. A single-row page is its own exact fence.
        budget.charge(WorkUsage {
            bytes: width(first.len().min(fence.len()))?,
            ..WorkUsage::default()
        })?;
        if first != fence {
            if maps.lookup(root, first, budget)?.is_some() {
                return Err(corrupt("page first key overlaps another upper fence"));
            }
            let next = scan_one(maps, root, first, budget)?;
            let matches = if let Some((selected, actual)) = next.entries.first() {
                budget.charge(WorkUsage {
                    bytes: width(selected.len().min(fence.len()))?,
                    ..WorkUsage::default()
                })?;
                if selected.as_slice() == fence {
                    budget.charge(WorkUsage {
                        bytes: width(actual.encoded_len()?.min(value.encoded_len()?))?,
                        ..WorkUsage::default()
                    })?;
                    *actual == value
                } else {
                    false
                }
            } else {
                false
            };
            if !matches {
                return Err(corrupt("page first key selects a different upper fence"));
            }
        }
    }
    Ok(checked)
}

fn initial_page(
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    root: MapRoot,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<CheckedPage>, IndexError> {
    if let Some(value) = maps.lookup(root, key, budget)? {
        return checked_at(objects, maps, root, key, value, true, None, budget).map(Some);
    }
    let page = scan_one(maps, root, key, budget)?;
    page.entries
        .first()
        .map(|(fence, value)| checked_at(objects, maps, root, fence, *value, true, None, budget))
        .transpose()
}

/// Select the authenticated page whose upper fence is the first fence at or
/// above `key`. A missing page is the map's checked end of stream.
pub(super) fn page_for_key(
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    root: MapRoot,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<CheckedPage>, IndexError> {
    check_term_key(key)?;
    if root.0.is_none() {
        return Ok(None);
    }
    initial_page(objects, maps, root, key, budget)
}

/// Sorted requests share one authenticated page until its upper fence passes.
///
/// The caller funds its key and output arrays; this function retains only one
/// page and the existing map scan's one-row result at a time.
pub(super) fn lookup_batch_sorted(
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    root: MapRoot,
    keys: &[&[u8]],
    out: &mut [Option<MapValue>],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if keys.len() != out.len() {
        return Err(IndexError::InvalidInput(
            "term lookup output width differs".into(),
        ));
    }
    let mut prior_key: Option<&[u8]> = None;
    for key in keys {
        check_term_key(key)?;
        if let Some(prior) = prior_key {
            budget.charge(WorkUsage {
                bytes: width(prior.len().min(key.len()))?,
                ..WorkUsage::default()
            })?;
            if prior >= *key {
                return Err(IndexError::InvalidInput(
                    "term lookup keys must be sorted unique".into(),
                ));
            }
        }
        prior_key = Some(key);
    }
    if keys.is_empty() || root.0.is_none() {
        out.fill(None);
        return Ok(());
    }
    let mut current = page_for_key(
        objects,
        maps,
        root,
        keys.first().ok_or(IndexError::ResourceLimited)?,
        budget,
    )?;
    for (key, selected) in keys.iter().zip(out.iter_mut()) {
        budget.checkpoint()?;
        while let Some(page) = current.as_ref() {
            let last = page.last_key_metered(budget)?;
            budget.charge(WorkUsage {
                bytes: width(last.len().min(key.len()))?,
                ..WorkUsage::default()
            })?;
            if last >= *key {
                break;
            }
            budget.charge(WorkUsage {
                bytes: width(last.len())?,
                ..WorkUsage::default()
            })?;
            let fence = PreparedKey::copy(last, budget)?;
            drop(current.take());
            let next = scan_one(maps, root, fence.as_slice(), budget)?;
            current = next
                .entries
                .first()
                .map(|(next_fence, value)| {
                    checked_at(
                        objects,
                        maps,
                        root,
                        next_fence,
                        *value,
                        false,
                        Some(fence.as_slice()),
                        budget,
                    )
                })
                .transpose()?;
        }
        *selected = if let Some(page) = &current {
            let first = page.first_key_metered(budget)?;
            budget.charge(WorkUsage {
                bytes: width(key.len().min(first.len()))?,
                ..WorkUsage::default()
            })?;
            if *key < first {
                None
            } else {
                page.lookup(key, budget)?
            }
        } else {
            None
        };
    }
    Ok(())
}

pub(super) fn lookup(
    objects: &dyn ImmutableObjectPort,
    maps: &dyn PersistentMapPort,
    root: MapRoot,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<MapValue>, IndexError> {
    let mut value = [None];
    lookup_batch_sorted(objects, maps, root, &[key], &mut value, budget)?;
    value.into_iter().next().ok_or(IndexError::ResourceLimited)
}
