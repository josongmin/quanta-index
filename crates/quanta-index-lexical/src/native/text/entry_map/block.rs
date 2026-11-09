//! Bounded inventory map with canonical first-use child slots. The immutable
//! object owns its digest and child edges; this codec interprets authenticated data.

use quanta_index_core::index::{
    IndexError, MapPage, MemoryLease, MemoryLifetime, ObjectRef, StagedObjectBatch, StagedPayload,
    StoredObject, WorkBudget, WorkUsage, ports::ImmutableObjectPort,
};

use crate::native::text_occurrence::block::CheckedBlock;

const MAGIC: &[u8; 8] = b"QITXEM02";
const HEADER_BYTES: usize = 12;
const MAX_ROWS: usize = 512;
const MAX_PAYLOAD_BYTES: usize = 16_384;
const MAX_KEY_BYTES: usize = 1_024;
const INLINE_ROWS: usize = 8;
const INLINE_KEY_BYTES: usize = 32;

fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(reason.into())
}

fn next_key<'a>(payload: &'a [u8], offset: &mut usize) -> Result<&'a [u8], IndexError> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| corrupt("block row offset overflow"))?;
    let encoded = payload
        .get(*offset..end)
        .ok_or_else(|| corrupt("truncated block row length"))?;
    let length = u16::from_be_bytes(
        encoded
            .try_into()
            .map_err(|_error| corrupt("block row length width differs"))?,
    );
    *offset = end;
    let length = usize::from(length);
    if length > MAX_KEY_BYTES {
        return Err(corrupt("block row key exceeds bound"));
    }
    let end = offset
        .checked_add(length)
        .ok_or_else(|| corrupt("block row offset overflow"))?;
    let key = payload
        .get(*offset..end)
        .ok_or_else(|| corrupt("truncated block row key"))?;
    *offset = end;
    Ok(key)
}

fn next_row<'a>(payload: &'a [u8], offset: &mut usize) -> Result<(&'a [u8], usize), IndexError> {
    let key = next_key(payload, offset)?;
    let end = offset
        .checked_add(2)
        .ok_or_else(|| corrupt("block child slot offset overflow"))?;
    let encoded = payload
        .get(*offset..end)
        .ok_or_else(|| corrupt("truncated block child slot"))?;
    *offset = end;
    let slot = usize::from(u16::from_be_bytes(
        encoded
            .try_into()
            .map_err(|_error| corrupt("block child slot width differs"))?,
    ));
    Ok((key, slot))
}

fn child_at(object: &StoredObject, slot: usize) -> Result<ObjectRef, IndexError> {
    object
        .registration
        .children
        .get(slot)
        .copied()
        .ok_or_else(|| corrupt("block child slot exceeds dictionary"))
}

/// Checks the complete body before a requested hit or page may escape.
fn validate(
    object: &StoredObject,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<usize, IndexError> {
    budget.checkpoint()?;
    let payload = object.payload.as_slice();
    if payload.len() < HEADER_BYTES || payload.len() > MAX_PAYLOAD_BYTES {
        return Err(corrupt("block payload size differs"));
    }
    if payload.get(..8) != Some(MAGIC.as_slice()) {
        return Err(corrupt("block magic differs"));
    }
    let encoded = payload
        .get(8..10)
        .ok_or_else(|| corrupt("truncated block row count"))?;
    let count = usize::from(u16::from_be_bytes(
        encoded
            .try_into()
            .map_err(|_error| corrupt("block row count width differs"))?,
    ));
    if count == 0 || count > MAX_ROWS {
        return Err(corrupt("block row count exceeds bound"));
    }
    let encoded = payload
        .get(10..12)
        .ok_or_else(|| corrupt("truncated block child count"))?;
    let child_count = usize::from(u16::from_be_bytes(
        encoded
            .try_into()
            .map_err(|_error| corrupt("block child count width differs"))?,
    ));
    if child_count == 0 || child_count > count {
        return Err(corrupt("block child count exceeds row count"));
    }
    if object.registration.reference != reference
        || object.registration.children.len() != child_count
    {
        return Err(corrupt("block registration or child count differs"));
    }
    for (index, child) in object.registration.children.iter().enumerate() {
        budget.checkpoint()?;
        for prior in object.registration.children.iter().take(index) {
            if prior == child {
                return Err(corrupt("block dictionary repeats child"));
            }
        }
    }
    let wire_bytes = 16_usize
        .checked_add(payload.len())
        .and_then(|bytes| {
            child_count
                .checked_mul(32)
                .and_then(|children| bytes.checked_add(children))
        })
        .ok_or_else(|| corrupt("block wire length overflow"))?;
    if object.registration.bytes
        != u64::try_from(wire_bytes).map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(corrupt("block registered wire length differs"));
    }
    budget.charge(WorkUsage {
        bytes: u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let mut offset = HEADER_BYTES;
    let mut previous: Option<&[u8]> = None;
    let mut small_eligible = count <= INLINE_ROWS;
    let mut first_unused_slot = 0_usize;
    for _ in 0..count {
        budget.checkpoint()?;
        let (key, slot) = next_row(payload, &mut offset)?;
        if previous.is_some_and(|prior| prior >= key) {
            return Err(corrupt("block keys are not strictly ordered"));
        }
        if slot >= child_count || slot > first_unused_slot {
            return Err(corrupt("block child slot is out of range or noncanonical"));
        }
        if slot == first_unused_slot {
            first_unused_slot = first_unused_slot
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        small_eligible &= key.len() <= INLINE_KEY_BYTES;
        previous = Some(key);
    }
    if first_unused_slot != child_count {
        return Err(corrupt("block dictionary has unused child"));
    }
    if offset != payload.len() {
        return Err(corrupt("block payload has trailing bytes"));
    }
    if small_eligible {
        return Err(corrupt("block can be represented as Small"));
    }
    Ok(count)
}

pub(super) fn lookup(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<ObjectRef>, IndexError> {
    budget.checkpoint()?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let object = objects.get(reference, budget)?;
    let count = validate(&object, reference, budget)?;
    let mut offset = HEADER_BYTES;
    for _ in 0..count {
        budget.checkpoint()?;
        let (stored, slot) = next_row(&object.payload, &mut offset)?;
        match stored.cmp(key) {
            std::cmp::Ordering::Equal => return Ok(Some(child_at(&object, slot)?)),
            std::cmp::Ordering::Greater => return Ok(None),
            std::cmp::Ordering::Less => {}
        }
    }
    Ok(None)
}

pub(super) fn scan(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    after: Option<&[u8]>,
    limit: u32,
    budget: &mut WorkBudget,
) -> Result<MapPage, IndexError> {
    budget.checkpoint()?;
    if limit == 0 || limit > 4096 {
        return Err(IndexError::InvalidInput(
            "entry scan limit exceeds bound".into(),
        ));
    }
    let object = objects.get(reference, budget)?;
    let count = validate(&object, reference, budget)?;
    let limit = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
    let mut offset = HEADER_BYTES;
    let mut eligible = 0_usize;
    for _ in 0..count {
        budget.checkpoint()?;
        let (key, _) = next_row(&object.payload, &mut offset)?;
        if after.is_none_or(|after| key > after) {
            eligible = eligible.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
    }
    let capacity = eligible.min(limit);
    let allocation = budget.reserve_memory(
        MapPage::allocation_bytes(capacity)?,
        MemoryLifetime::Retained,
    )?;
    // Local error paths also release entry/key data before their grants.
    let mut leases: Vec<MemoryLease> = Vec::new();
    leases
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    offset = HEADER_BYTES;
    for _ in 0..count {
        budget.checkpoint()?;
        let (key, slot) = next_row(&object.payload, &mut offset)?;
        if after.is_some_and(|after| key <= after) {
            continue;
        }
        if entries.len() == capacity {
            break;
        }
        let bytes = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Retained)?
        };
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(key.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        owned.extend_from_slice(key);
        let value = child_at(&object, slot)?;
        entries.push((owned, value.into()));
        leases.extend(memory);
    }
    Ok(MapPage::new(
        entries,
        eligible > capacity,
        leases,
        allocation,
    ))
}

/// One fully authenticated Block retained across the file-local 64-row pages.
///
/// Each continuation goes through the port's same-attempt retained-read policy;
/// this snapshot is not fresh namespace authentication or publication authority.
pub(in crate::native::text) struct BlockCursor<'a> {
    objects: &'a dyn ImmutableObjectPort,
    reference: ObjectRef,
    object: StoredObject,
    count: usize,
    ordinal: usize,
    offset: usize,
}

impl<'a> BlockCursor<'a> {
    pub(in crate::native::text) fn open(
        objects: &'a dyn ImmutableObjectPort,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let object = objects.get(reference, budget)?;
        let count = validate(&object, reference, budget)?;
        Ok(Self {
            objects,
            reference,
            object,
            count,
            ordinal: 0,
            offset: HEADER_BYTES,
        })
    }

    pub(in crate::native::text) fn next_page(
        &mut self,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        budget.checkpoint()?;
        if limit == 0 || limit > 64 {
            return Err(IndexError::InvalidInput(
                "block cursor page exceeds native inventory bound".into(),
            ));
        }
        if self.ordinal != 0 {
            let retained = self.objects.read_retained(&self.object, budget)?;
            // The default policy may perform a fresh read. If it returns new
            // DATA, demand the same authenticated object and full Block shape
            // before a continuation can escape.
            if !std::ptr::eq(&raw const *retained, &raw const *self.object) {
                if retained != self.object {
                    return Err(corrupt("retained block identity or data changed"));
                }
                let repeated_count = validate(&retained, self.reference, budget)?;
                if repeated_count != self.count {
                    return Err(corrupt("retained block row count changed"));
                }
            }
        }
        let remaining = self
            .count
            .checked_sub(self.ordinal)
            .ok_or_else(|| IndexError::BackendViolation("block cursor passed final row".into()))?;
        let capacity =
            remaining.min(usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?);
        let allocation = budget.reserve_memory(
            MapPage::allocation_bytes(capacity)?,
            MemoryLifetime::Retained,
        )?;
        let mut leases: Vec<MemoryLease> = Vec::new();
        leases
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        // A denied charge, grant, allocation or cancellation leaves this page
        // unread. The caller can retry with the same cursor and original row.
        let mut offset = self.offset;
        let mut ordinal = self.ordinal;
        for _ in 0..capacity {
            budget.checkpoint()?;
            let (key, slot) = next_row(&self.object.payload, &mut offset)?;
            let value = child_at(&self.object, slot)?;
            let bytes = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })?;
            let memory = if bytes == 0 {
                None
            } else {
                budget.reserve_memory(bytes, MemoryLifetime::Retained)?
            };
            let mut owned = Vec::new();
            owned
                .try_reserve_exact(key.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            owned.extend_from_slice(key);
            entries.push((owned, value.into()));
            leases.extend(memory);
            ordinal = ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        self.offset = offset;
        self.ordinal = ordinal;
        Ok(MapPage::new(
            entries,
            ordinal < self.count,
            leases,
            allocation,
        ))
    }
}

/// Bounded staging only. It does not identify or install immutable objects.
pub(in crate::native::text) struct BlockDraft {
    payload: StagedPayload,
    rows: usize,
    children: Vec<ObjectRef>,
    // Children die before their reservation, including on stage errors.
    _children_memory: Option<MemoryLease>,
    last_key: Option<(usize, usize)>,
    small_eligible: bool,
}

impl BlockDraft {
    /// A cold inventory can use its one packed occurrence as the map only
    /// when every canonical key in this draft is present in that object.
    pub(in crate::native::text) fn single_child(&self) -> Option<ObjectRef> {
        match self.children.as_slice() {
            [reference] => Some(*reference),
            _ => None,
        }
    }

    pub(in crate::native::text) fn matches_occurrence_keys(
        &self,
        occurrence: &CheckedBlock,
        budget: &WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        if self.rows != occurrence.term_count() || self.children.len() != 1 {
            return Ok(false);
        }
        let mut offset = HEADER_BYTES;
        for slot in 0..self.rows {
            budget.checkpoint()?;
            let (key, child_slot) = next_row(&self.payload.bytes, &mut offset)?;
            if child_slot != 0 || occurrence.key(slot)? != key {
                return Ok(false);
            }
        }
        if offset != self.payload.bytes.len() {
            return Err(corrupt("block draft has trailing bytes"));
        }
        Ok(true)
    }

    pub(in crate::native::text) fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let payload_memory = budget.reserve_memory(
            u64::try_from(MAX_PAYLOAD_BYTES).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let child_bytes = MAX_ROWS
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let children_memory = budget.reserve_memory(
            u64::try_from(child_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(MAX_PAYLOAD_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(MAX_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&[0; 4]);
        Ok(Self {
            payload: StagedPayload {
                bytes,
                memory: payload_memory,
            },
            rows: 0,
            children,
            _children_memory: children_memory,
            last_key: None,
            small_eligible: true,
        })
    }

    /// False only means a valid ordered entry cannot fit this bounded shape.
    pub(in crate::native::text) fn push(
        &mut self,
        key: &[u8],
        value: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        if let Some((start, len)) = self.last_key {
            let end = start.checked_add(len).ok_or(IndexError::ResourceLimited)?;
            let previous = self
                .payload
                .bytes
                .get(start..end)
                .ok_or_else(|| corrupt("draft last key offset differs"))?;
            if previous >= key {
                return Err(corrupt("block draft keys are not strictly ordered"));
            }
        }
        if key.len() > MAX_KEY_BYTES || self.rows == MAX_ROWS {
            return Ok(false);
        }
        let next_size = self
            .payload
            .bytes
            .len()
            .checked_add(2)
            .and_then(|bytes| bytes.checked_add(key.len()))
            .and_then(|bytes| bytes.checked_add(2))
            .ok_or(IndexError::ResourceLimited)?;
        if next_size > MAX_PAYLOAD_BYTES {
            return Ok(false);
        }
        let mut existing_slot = None;
        for (slot, child) in self.children.iter().enumerate() {
            if slot.is_multiple_of(32) {
                budget.checkpoint()?;
            }
            if *child == value {
                existing_slot = Some(slot);
                break;
            }
        }
        let slot = existing_slot.unwrap_or(self.children.len());
        budget.charge(WorkUsage {
            bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let length = u16::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        self.payload.bytes.extend_from_slice(&length.to_be_bytes());
        let start = self.payload.bytes.len();
        self.payload.bytes.extend_from_slice(key);
        self.payload.bytes.extend_from_slice(
            &u16::try_from(slot)
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        if existing_slot.is_none() {
            self.children.push(value);
        }
        self.rows = self
            .rows
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.last_key = Some((start, key.len()));
        self.small_eligible &= self.rows <= INLINE_ROWS && key.len() <= INLINE_KEY_BYTES;
        Ok(true)
    }

    pub(in crate::native::text) fn stage(
        mut self,
        batch: &mut StagedObjectBatch,
        budget: &WorkBudget,
    ) -> Result<usize, IndexError> {
        budget.checkpoint()?;
        if self.rows == 0 || self.small_eligible {
            return Err(corrupt("noncanonical block draft"));
        }
        let count = u16::try_from(self.rows).map_err(|_error| IndexError::ResourceLimited)?;
        self.payload
            .bytes
            .get_mut(8..10)
            .ok_or_else(|| corrupt("draft row count header missing"))?
            .copy_from_slice(&count.to_be_bytes());
        let child_count =
            u16::try_from(self.children.len()).map_err(|_error| IndexError::ResourceLimited)?;
        self.payload
            .bytes
            .get_mut(10..12)
            .ok_or_else(|| corrupt("draft child count header missing"))?
            .copy_from_slice(&child_count.to_be_bytes());
        let ordinal = batch.inputs().len();
        batch.push(self.payload, &self.children, budget)?;
        Ok(ordinal)
    }
}
