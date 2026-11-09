//! Bounded, childless posting rows and their positional page directory.
//!
//! A row is comparison data only. The selected owner's inventory remains the
//! authority for the occurrence object and must be checked against `witness`.

use quanta_index_core::index::{
    IndexError, MemoryLifetime, ObjectRef, StagedPayload, StoredObject, WorkBudget,
};

pub(in crate::native::text) const PAGE_ROWS: usize = 256;
pub(in crate::native::text) const MAX_PAGES: usize = 256;
pub(in crate::native::text) const MIN_ROWS: u64 = 9;
pub(in crate::native::text) const MAX_ROWS: u64 = 8192;

const PAGE_MAGIC: &[u8; 8] = b"QITXDP01";
const DIRECTORY_MAGIC: &[u8; 8] = b"QITXDD01";
const PAGE_HEADER: usize = 12;
const DIRECTORY_HEADER: usize = 16;
const ROW_BYTES: usize = 64;
const FENCE_BYTES: usize = 66;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) struct PostingRow {
    pub owner: [u8; 32],
    pub witness: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native::text) struct PageFence {
    pub first: [u8; 32],
    pub last: [u8; 32],
    pub count: u16,
}

fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(format!("dense posting: {reason}"))
}

fn invalid(reason: &str) -> IndexError {
    IndexError::InvalidInput(format!("dense posting: {reason}"))
}

fn width(bytes: usize) -> Result<u64, IndexError> {
    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
}

fn staged(bytes: usize, budget: &WorkBudget) -> Result<StagedPayload, IndexError> {
    let memory = budget.reserve_memory(width(bytes)?, MemoryLifetime::Temporary)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(bytes)
        .map_err(|_error| IndexError::ResourceLimited)?;
    Ok(StagedPayload {
        bytes: payload,
        memory,
    })
}

fn registered_wire(
    object: &StoredObject,
    reference: ObjectRef,
    children: usize,
) -> Result<(), IndexError> {
    if object.registration.reference != reference || object.registration.children.len() != children
    {
        return Err(corrupt("registration identity or child count differs"));
    }
    let bytes = object
        .payload
        .len()
        .checked_add(16)
        .and_then(|size| {
            children
                .checked_mul(32)
                .and_then(|edges| size.checked_add(edges))
        })
        .ok_or_else(|| corrupt("registered wire length overflows"))?;
    if object.registration.bytes != width(bytes)? {
        return Err(corrupt("registered wire length differs"));
    }
    Ok(())
}

fn read32(bytes: &[u8], start: usize) -> Result<[u8; 32], IndexError> {
    bytes
        .get(
            start
                ..start
                    .checked_add(32)
                    .ok_or_else(|| corrupt("offset overflows"))?,
        )
        .ok_or_else(|| corrupt("row is truncated"))?
        .try_into()
        .map_err(|_error| corrupt("row width differs"))
}

pub(in crate::native::text) fn page_payload(
    rows: &[PostingRow],
    budget: &WorkBudget,
) -> Result<(StagedPayload, PageFence), IndexError> {
    if rows.is_empty() || rows.len() > PAGE_ROWS {
        return Err(invalid("page row count exceeds bound"));
    }
    for pair in rows.windows(2) {
        budget.checkpoint()?;
        if pair[0].owner >= pair[1].owner {
            return Err(invalid("page owners are not strictly ordered"));
        }
    }
    let count = u16::try_from(rows.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let length = rows
        .len()
        .checked_mul(ROW_BYTES)
        .and_then(|body| PAGE_HEADER.checked_add(body))
        .ok_or(IndexError::ResourceLimited)?;
    let mut payload = staged(length, budget)?;
    payload.bytes.extend_from_slice(PAGE_MAGIC);
    payload.bytes.extend_from_slice(&count.to_be_bytes());
    payload.bytes.extend_from_slice(&[0; 2]);
    for row in rows {
        budget.checkpoint()?;
        payload.bytes.extend_from_slice(&row.owner);
        payload.bytes.extend_from_slice(&row.witness);
    }
    let first = rows.first().ok_or(IndexError::ResourceLimited)?.owner;
    let last = rows.last().ok_or(IndexError::ResourceLimited)?.owner;
    Ok((payload, PageFence { first, last, count }))
}

pub(in crate::native::text) fn directory_payload(
    fences: &[PageFence],
    budget: &WorkBudget,
) -> Result<StagedPayload, IndexError> {
    if fences.is_empty() || fences.len() > MAX_PAGES {
        return Err(invalid("directory page count exceeds bound"));
    }
    let mut total = 0_u64;
    let mut prior_last = None;
    for fence in fences {
        budget.checkpoint()?;
        if fence.count == 0
            || usize::from(fence.count) > PAGE_ROWS
            || fence.first > fence.last
            || (fence.count == 1) != (fence.first == fence.last)
            || prior_last.is_some_and(|last| last >= fence.first)
        {
            return Err(invalid("directory fences are not disjoint and ordered"));
        }
        total = total
            .checked_add(u64::from(fence.count))
            .ok_or(IndexError::ResourceLimited)?;
        prior_last = Some(fence.last);
    }
    if !(MIN_ROWS..=MAX_ROWS).contains(&total) {
        return Err(invalid("directory row count exceeds bound"));
    }
    let length = fences
        .len()
        .checked_mul(FENCE_BYTES)
        .and_then(|body| DIRECTORY_HEADER.checked_add(body))
        .ok_or(IndexError::ResourceLimited)?;
    let mut payload = staged(length, budget)?;
    payload.bytes.extend_from_slice(DIRECTORY_MAGIC);
    payload.bytes.extend_from_slice(
        &u32::try_from(total)
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    payload.bytes.extend_from_slice(
        &u16::try_from(fences.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    payload.bytes.extend_from_slice(&[0; 2]);
    for fence in fences {
        budget.checkpoint()?;
        payload.bytes.extend_from_slice(&fence.first);
        payload.bytes.extend_from_slice(&fence.last);
        payload.bytes.extend_from_slice(&fence.count.to_be_bytes());
    }
    Ok(payload)
}

#[derive(Clone)]
pub(in crate::native::text) struct Directory {
    object: StoredObject,
    total: u64,
    pages: usize,
}

impl Directory {
    pub(in crate::native::text) fn len(&self) -> usize {
        self.pages
    }

    pub(in crate::native::text) fn total(&self) -> u64 {
        self.total
    }

    pub(in crate::native::text) fn descriptor(
        &self,
        slot: usize,
    ) -> Result<(PageFence, ObjectRef), IndexError> {
        if slot >= self.pages {
            return Err(invalid("directory slot exceeds bound"));
        }
        let offset = slot
            .checked_mul(FENCE_BYTES)
            .and_then(|body| DIRECTORY_HEADER.checked_add(body))
            .ok_or(IndexError::ResourceLimited)?;
        let payload = self.object.payload.as_slice();
        let count = u16::from_be_bytes(
            payload
                .get(offset + 64..offset + 66)
                .ok_or_else(|| corrupt("directory descriptor is truncated"))?
                .try_into()
                .map_err(|_error| corrupt("directory count width differs"))?,
        );
        let reference = *self
            .object
            .registration
            .children
            .get(slot)
            .ok_or_else(|| corrupt("directory child is absent"))?;
        Ok((
            PageFence {
                first: read32(payload, offset)?,
                last: read32(payload, offset + 32)?,
                count,
            },
            reference,
        ))
    }

    pub(in crate::native::text) fn object(&self) -> &StoredObject {
        &self.object
    }
}

pub(in crate::native::text) fn decode_directory(
    object: StoredObject,
    reference: ObjectRef,
    budget: &WorkBudget,
) -> Result<Directory, IndexError> {
    budget.checkpoint()?;
    let payload = object.payload.as_slice();
    if payload.len() < DIRECTORY_HEADER
        || payload.get(..8) != Some(DIRECTORY_MAGIC.as_slice())
        || payload.get(14..16) != Some([0, 0].as_slice())
    {
        return Err(corrupt("directory envelope differs"));
    }
    let total = u64::from(u32::from_be_bytes(
        payload[8..12]
            .try_into()
            .map_err(|_error| corrupt("directory total width differs"))?,
    ));
    let pages = usize::from(u16::from_be_bytes(
        payload[12..14]
            .try_into()
            .map_err(|_error| corrupt("directory page count width differs"))?,
    ));
    if pages == 0 || pages > MAX_PAGES || !(MIN_ROWS..=MAX_ROWS).contains(&total) {
        return Err(corrupt("directory count exceeds bound"));
    }
    let length = pages
        .checked_mul(FENCE_BYTES)
        .and_then(|body| DIRECTORY_HEADER.checked_add(body))
        .ok_or_else(|| corrupt("directory size overflows"))?;
    if payload.len() != length {
        return Err(corrupt("directory size differs"));
    }
    registered_wire(&object, reference, pages)?;
    let mut sum = 0_u64;
    let mut previous_last = None;
    for slot in 0..pages {
        budget.checkpoint()?;
        let offset = DIRECTORY_HEADER + slot * FENCE_BYTES;
        let first = read32(payload, offset)?;
        let last = read32(payload, offset + 32)?;
        let count = u16::from_be_bytes(
            payload[offset + 64..offset + 66]
                .try_into()
                .map_err(|_error| corrupt("directory fence count width differs"))?,
        );
        if count == 0
            || usize::from(count) > PAGE_ROWS
            || first > last
            || (count == 1) != (first == last)
            || previous_last.is_some_and(|old| old >= first)
        {
            return Err(corrupt("directory fences are not disjoint and ordered"));
        }
        let child = object
            .registration
            .children
            .get(slot)
            .ok_or_else(|| corrupt("directory child is absent"))?;
        for (prior_slot, prior_child) in object.registration.children.iter().take(slot).enumerate()
        {
            if prior_slot.is_multiple_of(32) {
                budget.checkpoint()?;
            }
            if prior_child == child {
                return Err(corrupt("directory repeats a positional child"));
            }
        }
        sum = sum
            .checked_add(u64::from(count))
            .ok_or_else(|| corrupt("directory total overflows"))?;
        previous_last = Some(last);
    }
    if sum != total {
        return Err(corrupt("directory total differs from page counts"));
    }
    Ok(Directory {
        object,
        total,
        pages,
    })
}

pub(in crate::native::text) struct Page {
    object: StoredObject,
    rows: usize,
}

impl Page {
    pub(in crate::native::text) fn len(&self) -> usize {
        self.rows
    }

    pub(in crate::native::text) fn row(&self, slot: usize) -> Result<PostingRow, IndexError> {
        if slot >= self.rows {
            return Err(invalid("page slot exceeds bound"));
        }
        let offset = slot
            .checked_mul(ROW_BYTES)
            .and_then(|body| PAGE_HEADER.checked_add(body))
            .ok_or(IndexError::ResourceLimited)?;
        let payload = self.object.payload.as_slice();
        Ok(PostingRow {
            owner: read32(payload, offset)?,
            witness: read32(payload, offset + 32)?,
        })
    }

    pub(in crate::native::text) fn object(&self) -> &StoredObject {
        &self.object
    }
}

pub(in crate::native::text) fn decode_page(
    object: StoredObject,
    reference: ObjectRef,
    fence: PageFence,
    budget: &WorkBudget,
) -> Result<Page, IndexError> {
    budget.checkpoint()?;
    let payload = object.payload.as_slice();
    if payload.len() < PAGE_HEADER
        || payload.get(..8) != Some(PAGE_MAGIC.as_slice())
        || payload.get(10..12) != Some([0, 0].as_slice())
    {
        return Err(corrupt("page envelope differs"));
    }
    let rows = usize::from(u16::from_be_bytes(
        payload[8..10]
            .try_into()
            .map_err(|_error| corrupt("page count width differs"))?,
    ));
    if rows == 0 || rows > PAGE_ROWS || rows != usize::from(fence.count) {
        return Err(corrupt("page count differs from fence"));
    }
    let length = rows
        .checked_mul(ROW_BYTES)
        .and_then(|body| PAGE_HEADER.checked_add(body))
        .ok_or_else(|| corrupt("page size overflows"))?;
    if payload.len() != length {
        return Err(corrupt("page size differs"));
    }
    registered_wire(&object, reference, 0)?;
    let mut previous = None;
    for slot in 0..rows {
        budget.checkpoint()?;
        let owner = read32(payload, PAGE_HEADER + slot * ROW_BYTES)?;
        if previous.is_some_and(|old| old >= owner) {
            return Err(corrupt("page owners are not strictly ordered"));
        }
        if slot == 0 && owner != fence.first || slot + 1 == rows && owner != fence.last {
            return Err(corrupt("page endpoints differ from directory fence"));
        }
        previous = Some(owner);
    }
    Ok(Page { object, rows })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{Digest, RegisteredObject, WorkLimits};
    use sha2::{Digest as _, Sha256};
    use std::time::{Duration, Instant};

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 64,
                writes: 64,
                bytes: 1_000_000,
                mutations: 64,
            },
            Instant::now() + Duration::from_secs(30),
        )
    }

    // The same physical identity and registered wire length as the immutable
    // object port, computed independently of the directory decoder.
    fn stored(payload: Vec<u8>, children: Vec<ObjectRef>) -> Result<StoredObject, IndexError> {
        let payload_len = u32::try_from(payload.len()).map_err(|_| IndexError::ResourceLimited)?;
        let child_count = u32::try_from(children.len()).map_err(|_| IndexError::ResourceLimited)?;
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-immutable-object-v1\0");
        hash.update(b"QIOBJ001");
        hash.update(payload_len.to_be_bytes());
        hash.update(child_count.to_be_bytes());
        for child in &children {
            hash.update(child.0.0);
        }
        hash.update(&payload);
        let reference = ObjectRef(Digest(hash.finalize().into()));
        let bytes = 16_u64
            .checked_add(u64::from(payload_len))
            .and_then(|bytes| {
                u64::from(child_count)
                    .checked_mul(32)
                    .and_then(|edges| bytes.checked_add(edges))
            })
            .ok_or(IndexError::ResourceLimited)?;
        Ok(StoredObject::new(
            RegisteredObject::unscoped(reference, bytes, children),
            payload,
            None,
        ))
    }

    fn fences() -> [PageFence; 2] {
        [
            PageFence {
                first: [1; 32],
                last: [8; 32],
                count: 8,
            },
            PageFence {
                first: [9; 32],
                last: [9; 32],
                count: 1,
            },
        ]
    }

    #[test]
    fn directory_rejects_impossible_fences_and_repeated_real_child_edges() -> Result<(), IndexError>
    {
        let work = budget();
        let fences = fences();
        let valid = directory_payload(&fences, &work)?.bytes;
        let children = vec![ObjectRef(Digest([11; 32])), ObjectRef(Digest([12; 32]))];
        let object = stored(valid.clone(), children.clone())?;
        let reference = object.registration.reference;
        let checked = decode_directory(object, reference, &work)?;
        assert_eq!(checked.len(), 2);
        assert_eq!(checked.total(), 9);

        let mut impossible = fences;
        impossible[0].last = impossible[0].first;
        assert!(directory_payload(&impossible, &work).is_err());
        impossible = fences;
        impossible[1].last = [10; 32];
        assert!(directory_payload(&impossible, &work).is_err());

        let mut wire = valid.clone();
        wire[DIRECTORY_HEADER + 32..DIRECTORY_HEADER + 64].copy_from_slice(&[1; 32]);
        let object = stored(wire, children.clone())?;
        assert!(matches!(
            decode_directory(object.clone(), object.registration.reference, &work),
            Err(IndexError::CorruptObject(_))
        ));
        wire = valid.clone();
        let second = DIRECTORY_HEADER + FENCE_BYTES;
        wire[second + 32..second + 64].copy_from_slice(&[10; 32]);
        let object = stored(wire, children)?;
        assert!(matches!(
            decode_directory(object.clone(), object.registration.reference, &work),
            Err(IndexError::CorruptObject(_))
        ));

        let same = ObjectRef(Digest([11; 32]));
        let object = stored(valid, vec![same, same])?;
        assert!(matches!(
            decode_directory(object.clone(), object.registration.reference, &work),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }
}
