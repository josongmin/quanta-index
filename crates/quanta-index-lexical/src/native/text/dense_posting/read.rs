//! Authenticated, bounded reads of dense posting pages.
//!
//! Rows contain comparison witnesses only. The selected owner's inventory
//! retains the physical occurrence reference and is checked by the caller.

use super::wire::{Directory, Page, PageFence, PostingRow, decode_directory, decode_page};
use crate::native::text::posting_witness::OccurrenceDigestWitness;
use quanta_index_core::index::{
    IndexError, MapPage, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget, WorkUsage,
    ports::ImmutableObjectPort,
};

fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(reason.into())
}

fn directory(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<Directory, IndexError> {
    budget.checkpoint()?;
    decode_directory(objects.get(reference, budget)?, reference, budget)
}

fn page(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    fence: &PageFence,
    budget: &mut WorkBudget,
) -> Result<Page, IndexError> {
    budget.checkpoint()?;
    decode_page(objects.get(reference, budget)?, reference, *fence, budget)
}

fn first_directory_after(
    directory: &Directory,
    key: &[u8],
    strict: bool,
    budget: &WorkBudget,
) -> Result<usize, IndexError> {
    let mut lo = 0;
    let mut hi = directory.len();
    while lo < hi {
        budget.checkpoint()?;
        let mid = lo + (hi - lo) / 2;
        let (fence, _) = directory.descriptor(mid)?;
        let before = if strict {
            fence.last.as_slice() <= key
        } else {
            fence.last.as_slice() < key
        };
        if before {
            lo = mid.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

fn first_row_after(
    page: &Page,
    key: &[u8],
    strict: bool,
    budget: &WorkBudget,
) -> Result<usize, IndexError> {
    let mut lo = 0;
    let mut hi = page.len();
    while lo < hi {
        budget.checkpoint()?;
        let mid = lo + (hi - lo) / 2;
        let row = page.row(mid)?;
        let before = if strict {
            row.owner.as_slice() <= key
        } else {
            row.owner.as_slice() < key
        };
        if before {
            lo = mid.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

fn witness(row: PostingRow) -> OccurrenceDigestWitness {
    OccurrenceDigestWitness::from_bytes(row.witness)
}

fn empty_page(budget: &WorkBudget) -> Result<MapPage, IndexError> {
    let allocation =
        budget.reserve_memory(MapPage::allocation_bytes(0)?, MemoryLifetime::Retained)?;
    Ok(MapPage::new(Vec::new(), false, Vec::new(), allocation))
}

fn map_page(
    page: &Page,
    start: usize,
    limit: usize,
    more_pages: bool,
    budget: &mut WorkBudget,
) -> Result<MapPage, IndexError> {
    budget.checkpoint()?;
    let remaining = page
        .len()
        .checked_sub(start)
        .ok_or_else(|| corrupt("dense posting page cursor exceeds row count"))?;
    let capacity = remaining.min(limit);
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
    for slot in start
        ..start
            .checked_add(capacity)
            .ok_or(IndexError::ResourceLimited)?
    {
        budget.checkpoint()?;
        let row = page.row(slot)?;
        budget.charge(WorkUsage {
            bytes: 32,
            ..WorkUsage::default()
        })?;
        let memory = budget.reserve_memory(32, MemoryLifetime::Retained)?;
        let mut key = Vec::new();
        key.try_reserve_exact(32)
            .map_err(|_error| IndexError::ResourceLimited)?;
        key.extend_from_slice(&row.owner);
        entries.push((key, witness(row).into_value()?));
        leases.extend(memory);
    }
    let end = start
        .checked_add(capacity)
        .ok_or(IndexError::ResourceLimited)?;
    Ok(MapPage::new(
        entries,
        end < page.len() || more_pages,
        leases,
        allocation,
    ))
}

pub(in crate::native::text) fn lookup(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    key: &[u8],
    df: u64,
    budget: &mut WorkBudget,
) -> Result<Option<OccurrenceDigestWitness>, IndexError> {
    budget.checkpoint()?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let directory = directory(objects, reference, budget)?;
    if directory.total() != df {
        return Err(corrupt("packed posting directory DF differs from metadata"));
    }
    let slot = first_directory_after(&directory, key, false, budget)?;
    if slot == directory.len() {
        return Ok(None);
    }
    let (fence, page_ref) = directory.descriptor(slot)?;
    if key < fence.first.as_slice() {
        return Ok(None);
    }
    let page = page(objects, page_ref, &fence, budget)?;
    let row_slot = first_row_after(&page, key, false, budget)?;
    if row_slot == page.len() {
        return Ok(None);
    }
    let row = page.row(row_slot)?;
    Ok((row.owner.as_slice() == key).then(|| witness(row)))
}

/// One scan invocation may end at a physical page boundary with `has_more`.
pub(in crate::native::text) fn scan(
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
    let directory = directory(objects, reference, budget)?;
    let slot = match after {
        Some(after) => first_directory_after(&directory, after, true, budget)?,
        None => 0,
    };
    if slot == directory.len() {
        return empty_page(budget);
    }
    let (fence, page_ref) = directory.descriptor(slot)?;
    let page = page(objects, page_ref, &fence, budget)?;
    let first = match after {
        Some(after) => first_row_after(&page, after, true, budget)?,
        None => 0,
    };
    if first == page.len() {
        return Err(corrupt("dense posting directory continuation differs"));
    }
    map_page(
        &page,
        first,
        usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?,
        slot + 1 < directory.len(),
        budget,
    )
}

/// Retains authenticated directory and page DATA within one protected read.
/// Refused reads or allocation leave the next row unchanged.
pub(in crate::native::text) struct PackedCursor {
    directory: Directory,
    page_slot: usize,
    row_slot: usize,
    page: Option<Page>,
    emitted: u64,
}

impl PackedCursor {
    pub(in crate::native::text) fn open(
        objects: &dyn ImmutableObjectPort,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        Ok(Self {
            directory: directory(objects, reference, budget)?,
            page_slot: 0,
            row_slot: 0,
            page: None,
            emitted: 0,
        })
    }

    pub(in crate::native::text) fn total(&self) -> u64 {
        self.directory.total()
    }

    pub(in crate::native::text) fn next_page(
        &mut self,
        objects: &dyn ImmutableObjectPort,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        budget.checkpoint()?;
        if limit == 0 || limit > 64 {
            return Err(IndexError::InvalidInput(
                "dense posting cursor page exceeds bound".into(),
            ));
        }
        if self.emitted == self.directory.total() {
            return empty_page(budget);
        }
        if self.emitted != 0 {
            let retained = objects.read_retained(self.directory.object(), budget)?;
            if retained != *self.directory.object() {
                return Err(corrupt("retained dense posting directory changed"));
            }
        }
        let exhausted = self
            .page
            .as_ref()
            .is_some_and(|page| self.row_slot == page.len());
        let slot = self.page_slot + usize::from(exhausted);
        if slot >= self.directory.len() {
            return Err(corrupt(
                "dense posting directory ended before declared total",
            ));
        }
        let (fence, page_ref) = self.directory.descriptor(slot)?;
        let next_page = if self.page.is_none() || exhausted {
            Some(page(objects, page_ref, &fence, budget)?)
        } else {
            let current = self.page.as_ref().ok_or(IndexError::ResourceLimited)?;
            let retained = objects.read_retained(current.object(), budget)?;
            if retained != *current.object() {
                return Err(corrupt("retained dense posting page changed"));
            }
            None
        };
        let current = next_page
            .as_ref()
            .or(self.page.as_ref())
            .ok_or(IndexError::ResourceLimited)?;
        let start = if next_page.is_some() {
            0
        } else {
            self.row_slot
        };
        let output = map_page(
            current,
            start,
            usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?,
            slot + 1 < self.directory.len(),
            budget,
        )?;
        let emitted = self
            .emitted
            .checked_add(
                u64::try_from(output.entries.len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        if emitted > self.directory.total()
            || (emitted == self.directory.total()) == output.has_more
        {
            return Err(corrupt("dense posting cursor cardinality differs"));
        }
        let next_row = start
            .checked_add(output.entries.len())
            .ok_or(IndexError::ResourceLimited)?;
        if let Some(next_page) = next_page {
            self.page = Some(next_page);
        }
        self.page_slot = slot;
        self.row_slot = next_row;
        self.emitted = emitted;
        Ok(output)
    }
}
