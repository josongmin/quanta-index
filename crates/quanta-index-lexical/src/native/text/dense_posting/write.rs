//! Bounded, content-addressed posting pages. No object is installed until the
//! caller has authenticated the complete source edit stream.

use super::wire::{
    Directory, MAX_PAGES, MAX_ROWS, MIN_ROWS, PAGE_ROWS, PageFence, PostingRow, directory_payload,
    page_payload,
};
use crate::native::text::entry_map::EntryMap;
use crate::native::text_memory::PreparedRows;
use quanta_index_core::index::{
    AttemptContext, IndexError, MemoryLease, MemoryLifetime, ObjectRef, StagedObjectBatch,
    WorkBudget, WorkUsage, ports::MAX_OBJECT_INSTALL_BATCH,
};

struct PagePlan {
    start: usize,
    end: usize,
    fence: Option<PageFence>,
    reference: Option<ObjectRef>,
}

pub(in crate::native::text) struct Draft {
    prior: Option<Directory>,
    rows: PreparedRows<PostingRow>,
    expected: usize,
    plans: Vec<PagePlan>,
    _plan_memory: Option<MemoryLease>,
    local: bool,
    planned_rows: usize,
    planned_total: usize,
    finished: bool,
}

impl Draft {
    pub(in crate::native::text) fn new(
        prior: Option<Directory>,
        count: u64,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        if !(MIN_ROWS..=MAX_ROWS).contains(&count) {
            return Err(IndexError::InvalidInput(
                "posting draft count exceeds bound".into(),
            ));
        }
        if prior
            .as_ref()
            .is_some_and(|directory| directory.len() > MAX_PAGES)
        {
            return Err(IndexError::CorruptObject(
                "posting directory exceeds page bound".into(),
            ));
        }
        let expected = usize::try_from(count).map_err(|_error| IndexError::ResourceLimited)?;
        let rows = PreparedRows::with_capacity(expected, budget)?;
        Ok(Self {
            prior,
            rows,
            expected,
            plans: Vec::new(),
            _plan_memory: None,
            local: false,
            planned_rows: 0,
            planned_total: 0,
            finished: false,
        })
    }

    /// A local draft stores only changed rows. The caller owns a fully
    /// authenticated prior directory and passes unchanged descriptors through
    /// `keep_page` without reading or re-encoding their page bodies.
    pub(in crate::native::text) fn new_local(
        prior: Directory,
        count: u64,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        if !(MIN_ROWS..=MAX_ROWS).contains(&count) || prior.len() > MAX_PAGES {
            return Err(IndexError::InvalidInput(
                "posting local draft count or pages exceed bound".into(),
            ));
        }
        let expected = usize::try_from(count).map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            prior: Some(prior),
            rows: PreparedRows::new(expected)?,
            expected,
            plans: Vec::new(),
            _plan_memory: None,
            local: true,
            planned_rows: 0,
            planned_total: 0,
            finished: false,
        })
    }

    pub(in crate::native::text) fn row_offset(&self) -> usize {
        self.rows.len()
    }

    fn admit_plans(&mut self, budget: &WorkBudget) -> Result<(), IndexError> {
        if self.plans.capacity() != 0 {
            return budget.checkpoint();
        }
        let bytes = MAX_PAGES
            .checked_mul(std::mem::size_of::<PagePlan>())
            .ok_or(IndexError::ResourceLimited)?;
        let lease = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        self.plans
            .try_reserve_exact(MAX_PAGES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        self._plan_memory = lease;
        Ok(())
    }

    pub(in crate::native::text) fn keep_page(
        &mut self,
        fence: PageFence,
        reference: ObjectRef,
        budget: &WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        if !self.local || self.finished || self.planned_rows != self.rows.len() {
            return Err(IndexError::BackendViolation(
                "posting kept page is out of draft order".into(),
            ));
        }
        if fence.count == 0 || usize::from(fence.count) > PAGE_ROWS || fence.first > fence.last {
            return Err(IndexError::CorruptObject(
                "posting kept page fence is invalid".into(),
            ));
        }
        if !reused_leaf(self.prior.as_ref(), &fence, reference)? {
            return Err(IndexError::CorruptObject(
                "posting kept page differs from prior directory".into(),
            ));
        }
        if self
            .plans
            .last()
            .and_then(|plan| plan.fence)
            .is_some_and(|prior| prior.last >= fence.first)
        {
            return Err(IndexError::CorruptObject(
                "posting kept page is not ordered".into(),
            ));
        }
        let total = self
            .planned_total
            .checked_add(usize::from(fence.count))
            .ok_or(IndexError::ResourceLimited)?;
        if total > self.expected {
            return Err(IndexError::CorruptObject(
                "posting kept pages exceed declared count".into(),
            ));
        }
        if self.plans.len() == MAX_PAGES {
            return Ok(false);
        }
        self.admit_plans(budget)?;
        self.plans.push(PagePlan {
            start: self.planned_rows,
            end: self.planned_rows,
            fence: Some(fence),
            reference: Some(reference),
        });
        self.planned_total = total;
        Ok(true)
    }

    /// Plan only the rows appended since `start`; callers may interleave this
    /// with `keep_page` for untouched ranges.
    pub(in crate::native::text) fn finish_range(
        &mut self,
        start: usize,
        budget: &WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        if !self.local || self.finished || start != self.planned_rows || start > self.rows.len() {
            return Err(IndexError::BackendViolation(
                "posting local range is out of draft order".into(),
            ));
        }
        self.admit_plans(budget)?;
        let mut cursor = start;
        while cursor < self.rows.len() {
            budget.checkpoint()?;
            if self.plans.len() == MAX_PAGES {
                return Ok(false);
            }
            let end = cursor.saturating_add(PAGE_ROWS).min(self.rows.len());
            let first = self
                .rows
                .as_slice()
                .get(cursor)
                .ok_or(IndexError::ResourceLimited)?
                .owner;
            let last = self
                .rows
                .as_slice()
                .get(end - 1)
                .ok_or(IndexError::ResourceLimited)?
                .owner;
            if self
                .plans
                .last()
                .and_then(|plan| plan.fence)
                .is_some_and(|prior| prior.last >= first)
            {
                return Err(IndexError::CorruptObject(
                    "posting local range is not ordered".into(),
                ));
            }
            let count = end - cursor;
            let total = self
                .planned_total
                .checked_add(count)
                .ok_or(IndexError::ResourceLimited)?;
            if total > self.expected {
                return Err(IndexError::CorruptObject(
                    "posting local range exceeds declared count".into(),
                ));
            }
            self.plans.push(PagePlan {
                start: cursor,
                end,
                fence: Some(PageFence {
                    first,
                    last,
                    count: u16::try_from(count).map_err(|_error| IndexError::ResourceLimited)?,
                }),
                reference: None,
            });
            self.planned_total = total;
            self.planned_rows = end;
            cursor = end;
        }
        Ok(true)
    }

    pub(in crate::native::text) fn finish_local(
        &mut self,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if !self.local || self.finished || self.planned_rows != self.rows.len() {
            return Err(IndexError::BackendViolation(
                "posting local draft did not close its ranges".into(),
            ));
        }
        if self.planned_total != self.expected || self.plans.is_empty() {
            return Err(IndexError::CorruptObject(
                "posting local draft cardinality differs".into(),
            ));
        }
        self.finished = true;
        Ok(())
    }

    pub(in crate::native::text) fn push(
        &mut self,
        row: PostingRow,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        if self.finished {
            return Err(IndexError::BackendViolation(
                "posting draft changed after finish".into(),
            ));
        }
        if self
            .rows
            .as_slice()
            .last()
            .is_some_and(|previous| previous.owner >= row.owner)
            || (self.local
                && self.rows.len() == self.planned_rows
                && self
                    .plans
                    .last()
                    .and_then(|plan| plan.fence)
                    .is_some_and(|prior| prior.last >= row.owner))
        {
            return Err(IndexError::CorruptObject(
                "posting draft owners are not strictly ordered".into(),
            ));
        }
        let pending = self
            .rows
            .len()
            .checked_sub(self.planned_rows)
            .ok_or(IndexError::ResourceLimited)?;
        let local_count = self
            .planned_total
            .checked_add(pending)
            .ok_or(IndexError::ResourceLimited)?;
        if self.rows.len() == self.expected || (self.local && local_count == self.expected) {
            return Err(IndexError::CorruptObject(
                "posting draft exceeds declared row count".into(),
            ));
        }
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(std::mem::size_of::<PostingRow>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.rows.push(row, context.budget)?;
        Ok(())
    }

    pub(in crate::native::text) fn finish(
        &mut self,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        context.budget.checkpoint()?;
        if self.finished || self.local {
            return Err(IndexError::BackendViolation(
                "posting draft finished twice".into(),
            ));
        }
        if self.rows.len() != self.expected {
            return Err(IndexError::CorruptObject(
                "posting draft cardinality differs".into(),
            ));
        }
        let prior_pages = self.prior.as_ref().map_or(0, Directory::len);
        let capacity = (prior_pages + self.rows.len().div_ceil(PAGE_ROWS)).min(MAX_PAGES);
        let bytes = capacity
            .checked_mul(std::mem::size_of::<PagePlan>())
            .ok_or(IndexError::ResourceLimited)?;
        let lease = context.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        self.plans
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        self._plan_memory = lease;

        // Old high fences define stable ranges. An edit only repacks its own
        // range, so neighboring immutable leaves can keep their identities.
        // The last range also owns appends beyond the previous final owner.
        let mut start = 0;
        let mut stable = true;
        for slot in 0..prior_pages.saturating_sub(1) {
            context.budget.checkpoint()?;
            let (fence, _reference) = self
                .prior
                .as_ref()
                .ok_or(IndexError::ResourceLimited)?
                .descriptor(slot)?;
            let mut end = start;
            while self
                .rows
                .as_slice()
                .get(end)
                .is_some_and(|row| row.owner <= fence.last)
            {
                end += 1;
            }
            if !self.plan_range(start, end) {
                stable = false;
                break;
            }
            start = end;
        }
        if !stable || !self.plan_range(start, self.rows.len()) {
            // Heavy churn can fragment a bounded posting into many short
            // pages. Repack it within the same representation at the cap.
            self.plans.clear();
            if !self.plan_range(0, self.rows.len()) {
                return Err(IndexError::BackendViolation(
                    "bounded posting exceeds compact page count".into(),
                ));
            }
        }
        self.finished = true;
        Ok(())
    }

    fn plan_range(&mut self, mut start: usize, end: usize) -> bool {
        while start < end {
            if self.plans.len() == MAX_PAGES {
                return false;
            }
            let next = start.saturating_add(PAGE_ROWS).min(end);
            self.plans.push(PagePlan {
                start,
                end: next,
                fence: None,
                reference: None,
            });
            start = next;
        }
        true
    }
}

struct PendingLeaf {
    draft: usize,
    page: usize,
    expected: ObjectRef,
}

fn flush_leaves(
    batch: &mut StagedObjectBatch,
    pending: &mut Vec<PendingLeaf>,
    drafts: &mut [Option<Draft>],
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if pending.is_empty() {
        return Ok(());
    }
    let installed = context.put_batch(batch.inputs())?;
    if installed.len() != pending.len() {
        return Err(IndexError::BackendViolation(
            "posting leaf batch lost ordinal output".into(),
        ));
    }
    for (installed, slot) in installed.iter().zip(pending.drain(..)) {
        if *installed != slot.expected {
            return Err(IndexError::BackendViolation(
                "posting leaf identity changed during installation".into(),
            ));
        }
        let plan = drafts
            .get_mut(slot.draft)
            .and_then(Option::as_mut)
            .and_then(|draft| draft.plans.get_mut(slot.page))
            .ok_or_else(|| IndexError::BackendViolation("posting leaf slot missing".into()))?;
        plan.reference = Some(*installed);
    }
    Ok(())
}

fn reused_leaf(
    prior: Option<&Directory>,
    fence: &PageFence,
    expected: ObjectRef,
) -> Result<bool, IndexError> {
    let Some(prior) = prior else {
        return Ok(false);
    };
    // Descriptor first keys are strictly ordered. A changed earlier range may
    // shift ordinals without changing the content identity of this page.
    let mut low = 0;
    let mut high = prior.len();
    while low < high {
        let mid = low + (high - low) / 2;
        let (candidate, reference) = prior.descriptor(mid)?;
        match candidate.first.cmp(&fence.first) {
            std::cmp::Ordering::Less => low = mid + 1,
            std::cmp::Ordering::Greater => high = mid,
            std::cmp::Ordering::Equal => {
                return Ok(candidate.last == fence.last
                    && candidate.count == fence.count
                    && reference == expected);
            }
        }
    }
    Ok(false)
}

/// Fill only selected output slots. The caller must call this after complete
/// source EOF and digest validation for every draft in the batch.
pub(in crate::native::text) fn install(
    drafts: &mut [Option<Draft>],
    outputs: &mut [EntryMap],
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    context.budget.checkpoint()?;
    if drafts.len() != outputs.len() || drafts.len() > MAX_OBJECT_INSTALL_BATCH {
        return Err(IndexError::ResourceLimited);
    }
    for draft in drafts.iter().flatten() {
        if !draft.finished || draft.plans.is_empty() {
            return Err(IndexError::BackendViolation(
                "posting draft installed before source close".into(),
            ));
        }
    }

    let pending_bytes = MAX_OBJECT_INSTALL_BATCH
        .checked_mul(std::mem::size_of::<PendingLeaf>())
        .ok_or(IndexError::ResourceLimited)?;
    let pending_memory = context.budget.reserve_memory(
        u64::try_from(pending_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(MAX_OBJECT_INSTALL_BATCH)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut batch =
        StagedObjectBatch::for_install_packet(MAX_OBJECT_INSTALL_BATCH, context.budget)?;
    for draft_slot in 0..drafts.len() {
        let Some(page_count) = drafts
            .get(draft_slot)
            .and_then(Option::as_ref)
            .map(|draft| draft.plans.len())
        else {
            continue;
        };
        for page_slot in 0..page_count {
            context.budget.checkpoint()?;
            let draft = drafts
                .get_mut(draft_slot)
                .and_then(Option::as_mut)
                .ok_or(IndexError::ResourceLimited)?;
            let plan = draft
                .plans
                .get(page_slot)
                .ok_or(IndexError::ResourceLimited)?;
            if plan.reference.is_some() {
                if plan.fence.is_none() {
                    return Err(IndexError::BackendViolation(
                        "posting kept page lacks a fence".into(),
                    ));
                }
                continue;
            }
            let rows = draft
                .rows
                .as_slice()
                .get(plan.start..plan.end)
                .ok_or(IndexError::ResourceLimited)?;
            let (payload, fence) = page_payload(rows, context.budget)?;
            if plan.fence.is_some_and(|planned| planned != fence) {
                return Err(IndexError::BackendViolation(
                    "posting planned page fence changed".into(),
                ));
            }
            let expected = context
                .objects
                .identify(&payload.bytes, &[], context.budget)?
                .reference;
            draft
                .plans
                .get_mut(page_slot)
                .ok_or(IndexError::ResourceLimited)?
                .fence = Some(fence);
            if reused_leaf(draft.prior.as_ref(), &fence, expected)? {
                draft
                    .plans
                    .get_mut(page_slot)
                    .ok_or(IndexError::ResourceLimited)?
                    .reference = Some(expected);
                continue;
            }
            if pending.len() == MAX_OBJECT_INSTALL_BATCH {
                flush_leaves(&mut batch, &mut pending, drafts, context)?;
                drop(batch);
                batch = StagedObjectBatch::for_install_packet(
                    MAX_OBJECT_INSTALL_BATCH,
                    context.budget,
                )?;
            }
            batch.push(payload, &[], context.budget)?;
            pending.push(PendingLeaf {
                draft: draft_slot,
                page: page_slot,
                expected,
            });
        }
    }
    flush_leaves(&mut batch, &mut pending, drafts, context)?;
    drop(batch);
    drop(pending);
    drop(pending_memory);

    let directory_count = drafts.iter().flatten().count();
    if directory_count == 0 {
        return Ok(());
    }
    let directory_bytes = directory_count
        .checked_mul(std::mem::size_of::<(usize, ObjectRef)>())
        .ok_or(IndexError::ResourceLimited)?;
    let _directory_memory = context.budget.reserve_memory(
        u64::try_from(directory_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut directory_slots = Vec::new();
    directory_slots
        .try_reserve_exact(directory_count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut directories = StagedObjectBatch::for_install_packet(directory_count, context.budget)?;
    for (slot, draft) in drafts.iter().enumerate() {
        let Some(draft) = draft else { continue };
        let fence_bytes = draft
            .plans
            .len()
            .checked_mul(std::mem::size_of::<PageFence>())
            .ok_or(IndexError::ResourceLimited)?;
        let child_bytes = draft
            .plans
            .len()
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let _fence_memory = context.budget.reserve_memory(
            u64::try_from(fence_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let _child_memory = context.budget.reserve_memory(
            u64::try_from(child_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut fences = Vec::new();
        let mut children = Vec::new();
        fences
            .try_reserve_exact(draft.plans.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        children
            .try_reserve_exact(draft.plans.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for plan in &draft.plans {
            fences.push(plan.fence.ok_or_else(|| {
                IndexError::BackendViolation("posting page fence missing".into())
            })?);
            children.push(plan.reference.ok_or_else(|| {
                IndexError::BackendViolation("posting page child missing".into())
            })?);
        }
        let payload = directory_payload(&fences, context.budget)?;
        let expected = context
            .objects
            .identify(&payload.bytes, &children, context.budget)?
            .reference;
        if draft
            .prior
            .as_ref()
            .is_some_and(|prior| prior.object().registration.reference == expected)
        {
            *outputs.get_mut(slot).ok_or(IndexError::ResourceLimited)? =
                EntryMap::PostingPages(expected);
            continue;
        }
        directories.push(payload, &children, context.budget)?;
        directory_slots.push((slot, expected));
    }
    if !directory_slots.is_empty() {
        let installed = context.put_batch(directories.inputs())?;
        if installed.len() != directory_slots.len() {
            return Err(IndexError::BackendViolation(
                "posting directory batch lost ordinal output".into(),
            ));
        }
        for (actual, (slot, expected)) in installed.iter().zip(directory_slots) {
            if *actual != expected {
                return Err(IndexError::BackendViolation(
                    "posting directory identity changed during installation".into(),
                ));
            }
            *outputs.get_mut(slot).ok_or(IndexError::ResourceLimited)? =
                EntryMap::PostingPages(*actual);
        }
    }
    Ok(())
}
