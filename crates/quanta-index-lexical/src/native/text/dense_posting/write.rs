//! Bounded, content-addressed posting pages. No object is installed until the
//! caller has authenticated the complete source edit stream.

use super::wire::{
    Directory, MAX_PAGES, MAX_ROWS, MIN_ROWS, PAGE_ROWS, PageFence, PostingRow, directory_payload,
    directory_payload_bytes, page_payload, page_payload_bytes,
};
use crate::native::text::entry_map::EntryMap;
use crate::native::text_memory::PreparedRows;
use quanta_index_core::index::{
    AttemptContext, AttemptRef, IndexError, MemoryCreditLimits, MemoryLease, MemoryLifetime,
    ObjectRef, RegisteredObject, StagedObjectBatch, WorkBudget, WorkUsage,
    ports::{MAX_OBJECT_INSTALL_BATCH, ObjectInstallMemoryShape},
};
use sha2::{Digest as _, Sha256};

/// Disposable DATA about fully confirmed cold roots. Catalog child admission
/// remains authoritative when the caller installs the final term map.
pub(in crate::native::text) struct ConfirmedColdRoots {
    attempt: AttemptRef,
    entries: Vec<Option<([u8; 32], ObjectRef)>>,
    pending: Vec<Option<[u8; 32]>>,
    // Entry backing must drop before its Original grant.
    _memory: MemoryLease,
}

impl ConfirmedColdRoots {
    pub(in crate::native::text) fn try_new(
        attempt: AttemptRef,
        budget: &WorkBudget,
    ) -> Result<Option<Self>, IndexError> {
        let bytes = MAX_OBJECT_INSTALL_BATCH
            .checked_mul(std::mem::size_of::<Option<([u8; 32], ObjectRef)>>())
            .and_then(|bytes| {
                bytes.checked_add(
                    quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
                        .checked_mul(std::mem::size_of::<Option<[u8; 32]>>())?,
                )
            })
            .ok_or(IndexError::ResourceLimited)?;
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let mut entries = Vec::new();
        if entries.try_reserve_exact(MAX_OBJECT_INSTALL_BATCH).is_err()
            || entries.capacity() > MAX_OBJECT_INSTALL_BATCH
        {
            return Ok(None);
        }
        entries.resize(MAX_OBJECT_INSTALL_BATCH, None);
        let mut pending = Vec::new();
        if pending
            .try_reserve_exact(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS)
            .is_err()
            || pending.capacity() > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
        {
            return Ok(None);
        }
        pending.resize(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS, None);
        Ok(Some(Self {
            attempt,
            entries,
            pending,
            _memory: memory,
        }))
    }

    fn slot(key: [u8; 32]) -> usize {
        usize::from(u16::from_be_bytes([key[0], key[1]])) % MAX_OBJECT_INSTALL_BATCH
    }

    pub(in crate::native::text) fn get(
        &self,
        key: [u8; 32],
        attempt: AttemptRef,
        budget: &WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        budget.checkpoint()?;
        if attempt != self.attempt {
            return Err(IndexError::FenceLost);
        }
        Ok(self.entries[Self::slot(key)]
            .filter(|(candidate, _)| *candidate == key)
            .map(|(_, root)| root))
    }

    /// The caller must have validated all output ordinals and references from
    /// `install` before recording a new root. Failed prepares drop this memo.
    pub(in crate::native::text) fn remember(
        &mut self,
        key: [u8; 32],
        root: ObjectRef,
        attempt: AttemptRef,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        if self
            .get(key, attempt, budget)?
            .is_some_and(|prior| prior != root)
        {
            return Err(IndexError::BackendViolation(
                "cold posting identity changed".into(),
            ));
        }
        self.entries[Self::slot(key)] = Some((key, root));
        Ok(())
    }

    /// Called only after the caller authenticates the complete source page.
    pub(in crate::native::text) fn reuse(
        &mut self,
        drafts: &mut [Option<Draft>],
        outputs: &mut [EntryMap],
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        if drafts.len() != outputs.len() || drafts.len() > self.pending.len() {
            return Err(IndexError::BackendViolation(
                "cold memo page shape differs".into(),
            ));
        }
        self.pending.fill(None);
        for (slot, (draft, output)) in drafts.iter_mut().zip(outputs).enumerate() {
            let key = draft
                .as_ref()
                .map(|draft| draft.cold_key(context.budget))
                .transpose()?;
            if let Some(key) = key
                && let Some(root) = self.get(key, context.attempt, context.budget)?
            {
                *output = EntryMap::PostingPages(root);
                *draft = None;
            }
            self.pending[slot] = key;
        }
        Ok(())
    }

    pub(in crate::native::text) fn remember_installed(
        &mut self,
        outputs: &[EntryMap],
        context: &AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        if outputs.len() > self.pending.len() {
            return Err(IndexError::BackendViolation(
                "cold memo outputs exceed page".into(),
            ));
        }
        for (slot, output) in outputs.iter().enumerate() {
            if let Some(key) = self.pending[slot] {
                let EntryMap::PostingPages(root) = output else {
                    return Err(IndexError::BackendViolation(
                        "cold posting receipt lacks directory".into(),
                    ));
                };
                self.remember(key, *root, context.attempt, context.budget)?;
            }
        }
        self.pending.fill(None);
        Ok(())
    }
}

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
    /// Only the fixed cold packing is a function of these rows. Warm drafts
    /// retain historical fences and cannot use this memo key.
    pub(in crate::native::text) fn cold_key(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<[u8; 32], IndexError> {
        if !self.finished || self.local || self.prior.is_some() {
            return Err(IndexError::BackendViolation(
                "posting memo requires finished cold draft".into(),
            ));
        }
        const DOMAIN: &[u8] = b"quanta-index/cold-dense-rows/v1\0";
        let bytes = self
            .rows
            .len()
            .checked_mul(64)
            .and_then(|bytes| bytes.checked_add(DOMAIN.len() + 16))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut hash = Sha256::new();
        hash.update(DOMAIN);
        hash.update((PAGE_ROWS as u64).to_be_bytes());
        hash.update((self.rows.len() as u64).to_be_bytes());
        for row in self.rows.as_slice() {
            budget.checkpoint()?;
            hash.update(row.owner);
            hash.update(row.witness);
        }
        Ok(hash.finalize().into())
    }

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

    // This common backing is funded by the original scope in both paths and
    // stays live while the optional payload/backend credit is considered.
    let batch = StagedObjectBatch::for_install_packet(MAX_OBJECT_INSTALL_BATCH, context.budget)?;
    if let Some(limits) = coalesced_credit(drafts, context)? {
        let attempt = context.attempt;
        let objects = context.objects;
        let maps = context.maps;
        let custody = context.custody;
        let scratch = context.scratch;
        if let Some(mut credit) = context.budget.try_scoped_memory_credit(limits)? {
            let mut scoped = AttemptContext {
                attempt,
                objects,
                maps,
                custody,
                scratch,
                budget: &mut credit,
            };
            return install_coalesced(drafts, outputs, batch, &mut scoped);
        }
    }
    install_separate(drafts, outputs, batch, context)
}

fn install_separate(
    drafts: &mut [Option<Draft>],
    outputs: &mut [EntryMap],
    mut batch: StagedObjectBatch,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
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

fn backing_bytes<T>(count: usize) -> Result<u64, IndexError> {
    let bytes = count
        .checked_mul(std::mem::size_of::<T>())
        .ok_or(IndexError::ResourceLimited)?;
    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
}

/// Cold windows have no confirmed prior leaves to authenticate. Warm/local
/// drafts retain the original shared-leaf path until separately qualified.
fn coalesced_credit(
    drafts: &[Option<Draft>],
    context: &AttemptContext<'_>,
) -> Result<Option<MemoryCreditLimits>, IndexError> {
    let mut shape = ObjectInstallMemoryShape {
        maximum_inputs: 0,
        maximum_total_payload_bytes: 0,
        maximum_payload_bytes: 0,
        maximum_total_children: 0,
    };
    let mut directories = 0_usize;
    let mut max_pages = 0_usize;
    for draft in drafts.iter().flatten() {
        context.budget.checkpoint()?;
        if draft.prior.is_some() || draft.plans.iter().any(|plan| plan.reference.is_some()) {
            return Ok(None);
        }
        directories = directories
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        max_pages = max_pages.max(draft.plans.len());
        shape.maximum_total_children = shape
            .maximum_total_children
            .checked_add(draft.plans.len())
            .ok_or(IndexError::ResourceLimited)?;
        for plan in &draft.plans {
            context.budget.checkpoint()?;
            let rows = plan
                .end
                .checked_sub(plan.start)
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = page_payload_bytes(rows)?;
            shape.maximum_total_payload_bytes = shape
                .maximum_total_payload_bytes
                .checked_add(bytes)
                .ok_or(IndexError::ResourceLimited)?;
            shape.maximum_payload_bytes = shape.maximum_payload_bytes.max(bytes);
        }
        let bytes = directory_payload_bytes(draft.plans.len())?;
        shape.maximum_total_payload_bytes = shape
            .maximum_total_payload_bytes
            .checked_add(bytes)
            .ok_or(IndexError::ResourceLimited)?;
        shape.maximum_payload_bytes = shape.maximum_payload_bytes.max(bytes);
    }
    if directories == 0 {
        return Ok(None);
    }
    let leaves = shape.maximum_total_children;
    shape.maximum_inputs = leaves
        .checked_add(directories)
        .ok_or(IndexError::ResourceLimited)?;
    if shape.maximum_inputs > MAX_OBJECT_INSTALL_BATCH {
        return Ok(None);
    }
    // The backend owns physical wire limits and all of its preparation bounds.
    let backend = match context.put_batch_memory_bound(shape) {
        Ok(Some(bound)) => bound,
        Ok(None) | Err(IndexError::ResourceLimited) => return Ok(None),
        Err(error) => return Err(error),
    };
    let temporary_data_bytes = u64::try_from(shape.maximum_total_payload_bytes)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(backing_bytes::<ObjectRef>(leaves)?)
        .and_then(|bytes| bytes.checked_add(backing_bytes::<PendingLeaf>(leaves).ok()?))
        .and_then(|bytes| bytes.checked_add(backing_bytes::<(usize, ObjectRef)>(directories).ok()?))
        .and_then(|bytes| bytes.checked_add(backing_bytes::<PageFence>(max_pages).ok()?))
        .and_then(|bytes| bytes.checked_add(backing_bytes::<ObjectRef>(max_pages).ok()?))
        .ok_or(IndexError::ResourceLimited)?;
    let live_leases = shape
        .maximum_inputs
        .checked_add(directories)
        .and_then(|count| count.checked_add(4))
        .ok_or(IndexError::ResourceLimited)?;
    backend
        .checked_add(MemoryCreditLimits {
            temporary_data_bytes,
            retained_data_bytes: RegisteredObject::allocation_bytes(max_pages)?,
            max_live_temporary_leases: u32::try_from(live_leases)
                .map_err(|_error| IndexError::ResourceLimited)?,
            max_live_retained_leases: 1,
        })
        .map(Some)
}

fn install_coalesced(
    drafts: &mut [Option<Draft>],
    outputs: &mut [EntryMap],
    mut batch: StagedObjectBatch,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let leaves = drafts.iter().flatten().map(|draft| draft.plans.len()).sum();
    let directories = drafts.iter().flatten().count();
    let max_pages = drafts
        .iter()
        .flatten()
        .map(|draft| draft.plans.len())
        .max()
        .ok_or(IndexError::ResourceLimited)?;
    let mut pending = PreparedRows::<PendingLeaf>::with_capacity(leaves, context.budget)?;
    let mut slots = PreparedRows::<(usize, ObjectRef)>::with_capacity(directories, context.budget)?;
    let mut fences = PreparedRows::<PageFence>::with_capacity(max_pages, context.budget)?;
    let mut children = PreparedRows::<ObjectRef>::with_capacity(max_pages, context.budget)?;
    // Retain predicted refs as DATA. No draft is marked confirmed until every
    // ordinal receipt, including the last parent, has been checked.
    for (draft_slot, draft) in drafts.iter_mut().enumerate() {
        let Some(draft) = draft else { continue };
        for (page_slot, plan) in draft.plans.iter_mut().enumerate() {
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
            plan.fence = Some(fence);
            batch.push(payload, &[], context.budget)?;
            pending.push(
                PendingLeaf {
                    draft: draft_slot,
                    page: page_slot,
                    expected,
                },
                context.budget,
            )?;
        }
    }
    let mut first = 0_usize;
    for (slot, draft) in drafts.iter().enumerate() {
        let Some(draft) = draft else { continue };
        fences.clear();
        children.clear();
        let end = first
            .checked_add(draft.plans.len())
            .ok_or(IndexError::ResourceLimited)?;
        let predictions = pending
            .as_slice()
            .get(first..end)
            .ok_or(IndexError::ResourceLimited)?;
        for (plan, prediction) in draft.plans.iter().zip(predictions) {
            fences.push(
                plan.fence.ok_or_else(|| {
                    IndexError::BackendViolation("posting page fence missing".into())
                })?,
                context.budget,
            )?;
            children.push(prediction.expected, context.budget)?;
        }
        let payload = directory_payload(fences.as_slice(), context.budget)?;
        let expected = context
            .objects
            .identify(&payload.bytes, children.as_slice(), context.budget)?
            .reference;
        batch.push(payload, children.as_slice(), context.budget)?;
        slots.push((slot, expected), context.budget)?;
        first = end;
    }
    let installed = context.put_batch(batch.inputs())?;
    if installed.len() != leaves + directories {
        return Err(IndexError::BackendViolation(
            "posting combined batch lost ordinal output".into(),
        ));
    }
    for (actual, expected) in installed.iter().zip(
        pending
            .as_slice()
            .iter()
            .map(|leaf| leaf.expected)
            .chain(slots.as_slice().iter().map(|(_, reference)| *reference)),
    ) {
        if *actual != expected {
            return Err(IndexError::BackendViolation(
                "posting combined identity changed during installation".into(),
            ));
        }
    }
    for leaf in pending.as_slice() {
        let plan = drafts
            .get_mut(leaf.draft)
            .and_then(Option::as_mut)
            .and_then(|draft| draft.plans.get_mut(leaf.page))
            .ok_or_else(|| IndexError::BackendViolation("posting leaf slot missing".into()))?;
        plan.reference = Some(leaf.expected);
    }
    for &(slot, reference) in slots.as_slice() {
        *outputs.get_mut(slot).ok_or(IndexError::ResourceLimited)? =
            EntryMap::PostingPages(reference);
    }
    Ok(())
}
