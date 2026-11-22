//! COW term-page production before the final multi-root map transaction.
//!
//! Logical term edits remain disposable input. Only fully installed page
//! references become upper-fence map edits; owner edits keep their original
//! ordinal and are consumed by the same final map apply.

use quanta_index_core::index::{
    AttemptContext, IndexError, MapEdit, MapRoot, MapValue, MemoryLease, MemoryLifetime,
    ObjectBatchInput, ObjectRef, OrdinalMapEdit, WorkBudget, WorkUsage, ports::MapEditCursor,
};

use super::term_dictionary::{self, CheckedPage, PageDraft, SealedPage};
use super::{PreparedKey, TextSpool, check_term_key};
use crate::native::text_memory::PreparedRows;

fn invalid(message: &'static str) -> IndexError {
    IndexError::CorruptObject(message.into())
}

fn compare(
    left: &[u8],
    right: &[u8],
    budget: &mut WorkBudget,
) -> Result<std::cmp::Ordering, IndexError> {
    budget.charge(WorkUsage {
        bytes: u64::try_from(left.len().min(right.len()))
            .map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    Ok(left.cmp(right))
}

fn equal_references(
    left: Option<ObjectRef>,
    right: Option<ObjectRef>,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    if left.is_some() && right.is_some() {
        budget.charge(WorkUsage {
            bytes: 32,
            ..WorkUsage::default()
        })?;
    }
    Ok(left == right)
}

fn equal_values(
    left: Option<MapValue>,
    right: Option<MapValue>,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    if let (Some(left), Some(right)) = (left, right) {
        budget.charge(WorkUsage {
            bytes: u64::try_from(left.encoded_len()?.min(right.encoded_len()?))
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
    }
    Ok(left == right)
}

struct TermSource<C> {
    cursor: C,
    pending: Option<OrdinalMapEdit>,
    prior: Option<OrdinalMapEdit>,
    initialized: bool,
    done: bool,
}

impl<C: MapEditCursor> TermSource<C> {
    fn new(cursor: C) -> Self {
        Self {
            cursor,
            pending: None,
            prior: None,
            initialized: false,
            done: false,
        }
    }

    fn load(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.initialized || self.done {
            return Ok(());
        }
        self.initialized = true;
        let Some(edit) = self.cursor.next_edit(budget)? else {
            self.done = true;
            return Ok(());
        };
        if edit.root() == 1 {
            // Owners have a fresh direct surface-one cursor after all page
            // anchors are ready. Do not replay any source term edits.
            self.done = true;
            return Ok(());
        }
        if edit.root() != 0 {
            return Err(invalid("term edit has an unknown root ordinal"));
        }
        check_term_key(edit.key())?;
        if let Some(prior) = self.prior.as_ref()
            && compare(prior.key(), edit.key(), budget)? != std::cmp::Ordering::Less
        {
            return Err(invalid("term edits are not strictly ordered"));
        }
        self.pending = Some(edit);
        Ok(())
    }

    fn consume(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        self.prior = self.pending.take();
        self.initialized = false;
        self.load(budget)
    }
}

/// The row backing dies before its separate key grants on every error path.
struct Anchors {
    rows: PreparedRows<MapEdit>,
    keys: PreparedRows<Option<MemoryLease>>,
}

impl Anchors {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        Ok(Self {
            rows: PreparedRows::with_capacity(64, budget)?,
            keys: PreparedRows::with_capacity(64, budget)?,
        })
    }

    fn push(
        &mut self,
        key: PreparedKey,
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
        spool: &TextSpool,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if equal_references(expected, next, budget)? {
            return Ok(());
        }
        let (key, memory) = key.into_parts();
        self.rows.push(
            MapEdit {
                key,
                expected: expected.map(MapValue::Object),
                next: next.map(MapValue::Object),
            },
            budget,
        )?;
        self.keys.push(memory, budget)?;
        if self.rows.len() == 64 {
            self.flush(spool, budget)?;
        }
        Ok(())
    }

    fn flush(&mut self, spool: &TextSpool, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.rows.len() != 0 {
            spool.term_anchor_batch(self.rows.as_slice(), budget)?;
        }
        self.rows.clear();
        self.keys.clear();
        Ok(())
    }
}

const PAGE_INSTALL_BATCH: usize = 32;

struct PendingPage {
    fence: PreparedKey,
    expected: Option<ObjectRef>,
    identified: ObjectRef,
    // The input Vec drops before these payload/child grants.
    _payload_memory: Option<MemoryLease>,
    _children_memory: Option<MemoryLease>,
}

/// One bounded immutable install packet. Both Vecs and every page payload
/// retain their Original grants through the complete ordinal receipt check.
struct PageBatch {
    inputs: Vec<ObjectBatchInput>,
    pending: Vec<PendingPage>,
    _inputs_memory: Option<MemoryLease>,
    _pending_memory: Option<MemoryLease>,
}

impl PageBatch {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let inputs_bytes = PAGE_INSTALL_BATCH
            .checked_mul(std::mem::size_of::<ObjectBatchInput>())
            .ok_or(IndexError::ResourceLimited)?;
        let inputs_memory = budget.reserve_memory(
            u64::try_from(inputs_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut inputs = Vec::new();
        inputs
            .try_reserve_exact(PAGE_INSTALL_BATCH)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if inputs.capacity() > PAGE_INSTALL_BATCH {
            return Err(IndexError::ResourceLimited);
        }
        let pending_bytes = PAGE_INSTALL_BATCH
            .checked_mul(std::mem::size_of::<PendingPage>())
            .ok_or(IndexError::ResourceLimited)?;
        let pending_memory = budget.reserve_memory(
            u64::try_from(pending_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(PAGE_INSTALL_BATCH)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if pending.capacity() > PAGE_INSTALL_BATCH {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            inputs,
            pending,
            _inputs_memory: inputs_memory,
            _pending_memory: pending_memory,
        })
    }

    fn push(
        &mut self,
        page: SealedPage,
        expected: Option<ObjectRef>,
        identified: ObjectRef,
        anchors: &mut Anchors,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
    ) -> Result<(), IndexError> {
        if self.inputs.len() == PAGE_INSTALL_BATCH {
            self.flush(anchors, context, spool)?;
        }
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(
                std::mem::size_of::<ObjectBatchInput>()
                    .checked_add(std::mem::size_of::<PendingPage>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let (input, fence, payload_memory, children_memory) = page.into_parts();
        self.inputs.push(input);
        self.pending.push(PendingPage {
            fence,
            expected,
            identified,
            _payload_memory: payload_memory,
            _children_memory: children_memory,
        });
        Ok(())
    }

    fn flush(
        &mut self,
        anchors: &mut Anchors,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
    ) -> Result<(), IndexError> {
        if self.inputs.is_empty() {
            return context.budget.checkpoint();
        }
        let installed = context.put_batch(&self.inputs)?;
        if installed.len() != self.pending.len() {
            return Err(IndexError::BackendViolation(
                "term page install lost ordinal output".into(),
            ));
        }
        for (actual, pending) in installed.iter().zip(&self.pending) {
            if !equal_references(Some(*actual), Some(pending.identified), context.budget)? {
                return Err(IndexError::BackendViolation(
                    "term page install receipt differs".into(),
                ));
            }
        }
        drop(installed);
        self.inputs.clear();
        for pending in self.pending.drain(..) {
            anchors.push(
                pending.fence,
                pending.expected,
                Some(pending.identified),
                spool,
                context.budget,
            )?;
        }
        Ok(())
    }
}

struct Window {
    old: Option<CheckedPage>,
    old_fence: Option<PreparedKey>,
    old_reference: Option<ObjectRef>,
    old_at: usize,
    replaced_old_anchor: bool,
    draft: Option<PageDraft>,
}

impl Window {
    fn new(
        prior: MapRoot,
        first: &[u8],
        context: &mut AttemptContext<'_>,
    ) -> Result<Self, IndexError> {
        let old = term_dictionary::page_for_key(
            context.objects,
            context.maps,
            prior,
            first,
            context.budget,
        )?;
        let old_fence = if let Some(page) = old.as_ref() {
            let last = page.last_key_metered(context.budget)?;
            context.budget.charge(WorkUsage {
                bytes: u64::try_from(last.len()).map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            Some(PreparedKey::copy(last, context.budget)?)
        } else {
            None
        };
        let old_reference = old.as_ref().map(CheckedPage::reference);
        Ok(Self {
            old,
            old_fence,
            old_reference,
            old_at: 0,
            replaced_old_anchor: false,
            draft: Some(PageDraft::new(context.budget)?),
        })
    }

    fn accepts(&self, key: &[u8], budget: &mut WorkBudget) -> Result<bool, IndexError> {
        self.old_fence.as_ref().map_or(Ok(true), |fence| {
            compare(key, fence.as_slice(), budget)
                .map(|ordering| ordering != std::cmp::Ordering::Greater)
        })
    }

    fn emit(
        &mut self,
        anchors: &mut Anchors,
        pages: &mut Option<PageBatch>,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
    ) -> Result<(), IndexError> {
        let Some(draft) = self.draft.take() else {
            return Err(IndexError::BackendViolation(
                "term page draft missing".into(),
            ));
        };
        if draft.is_empty() {
            return Ok(());
        }
        let sealed = draft.finish(context.budget)?;
        if !self.accepts(sealed.fence.as_slice(), context.budget)? {
            return Err(invalid("term page exceeded its prior upper fence"));
        }
        let same_fence = if let Some(old) = self.old_fence.as_ref() {
            compare(old.as_slice(), sealed.fence.as_slice(), context.budget)?
                == std::cmp::Ordering::Equal
        } else {
            false
        };
        let expected = if same_fence {
            self.replaced_old_anchor = true;
            self.old_reference
        } else {
            None
        };
        let identified = context
            .objects
            .identify(
                &sealed.input.payload,
                &sealed.input.children,
                context.budget,
            )?
            .reference;
        if equal_references(expected, Some(identified), context.budget)? {
            // The old authenticated page has the same canonical bytes. No
            // new page installation or map operation is necessary.
            return Ok(());
        }
        if pages.is_none() {
            *pages = Some(PageBatch::new(context.budget)?);
        }
        pages
            .as_mut()
            .ok_or(IndexError::ResourceLimited)?
            .push(sealed, expected, identified, anchors, context, spool)
    }

    fn finish(
        &mut self,
        anchors: &mut Anchors,
        pages: &mut Option<PageBatch>,
        context: &mut AttemptContext<'_>,
        spool: &TextSpool,
    ) -> Result<(), IndexError> {
        self.emit(anchors, pages, context, spool)?;
        if let Some(pages) = pages.as_mut() {
            pages.flush(anchors, context, spool)?;
        }
        if !self.replaced_old_anchor
            && let Some(fence) = self.old_fence.take()
        {
            anchors.push(fence, self.old_reference, None, spool, context.budget)?;
        }
        Ok(())
    }
}

fn rewrite_window<C: MapEditCursor>(
    window: &mut Window,
    source: &mut TermSource<C>,
    anchors: &mut Anchors,
    pages: &mut Option<PageBatch>,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
) -> Result<(), IndexError> {
    loop {
        context.budget.checkpoint()?;
        source.load(context.budget)?;
        let edit = match source.pending.as_ref() {
            Some(edit) if window.accepts(edit.key(), context.budget)? => Some(edit),
            Some(_) | None => None,
        };
        let old = match window.old.as_ref() {
            Some(page) if window.old_at < page.len() => {
                Some(page.entry_metered(window.old_at, context.budget)?)
            }
            Some(_) | None => None,
        };
        let ordering = match (old.as_ref(), edit) {
            (Some((key, _)), Some(edit)) => compare(key, edit.key(), context.budget)?,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => break,
        };
        let (key, value, advance_old, advance_edit) = match ordering {
            std::cmp::Ordering::Less => {
                let (key, value) = old.ok_or(IndexError::ResourceLimited)?;
                (key, Some(value), true, false)
            }
            std::cmp::Ordering::Greater => {
                let edit = edit.ok_or(IndexError::ResourceLimited)?;
                if edit.expected()?.is_some() {
                    return Err(IndexError::Conflict);
                }
                (edit.key(), edit.next()?, false, true)
            }
            std::cmp::Ordering::Equal => {
                let (key, value) = old.ok_or(IndexError::ResourceLimited)?;
                let edit = edit.ok_or(IndexError::ResourceLimited)?;
                if !equal_values(edit.expected()?, Some(value), context.budget)? {
                    return Err(IndexError::Conflict);
                }
                (key, edit.next()?, true, true)
            }
        };
        if let Some(value) = value {
            let draft = window.draft.as_mut().ok_or_else(|| {
                IndexError::BackendViolation("term page draft missing during merge".into())
            })?;
            if !draft.try_push(key, value, context.budget)? {
                window.emit(anchors, pages, context, spool)?;
                window.draft = Some(PageDraft::new(context.budget)?);
                continue;
            }
        }
        if advance_old {
            window.old_at = window
                .old_at
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        if advance_edit {
            source.consume(context.budget)?;
        }
    }
    window.finish(anchors, pages, context, spool)
}

pub(super) fn prepare(
    prior: MapRoot,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
) -> Result<(), IndexError> {
    let result = (|| {
        spool.begin_term_anchors(context.budget)?;
        let mut anchors = Anchors::new(context.budget)?;
        let mut pages = None;
        let mut source = TermSource::new(spool.top_cursor());
        source.load(context.budget)?;
        while let Some(edit) = source.pending.as_ref() {
            let mut window = Window::new(prior, edit.key(), context)?;
            rewrite_window(
                &mut window,
                &mut source,
                &mut anchors,
                &mut pages,
                context,
                spool,
            )?;
        }
        if let Some(pages) = pages.as_mut() {
            pages.flush(&mut anchors, context, spool)?;
        }
        anchors.flush(spool, context.budget)?;
        spool.seal_term_anchors()?;
        Ok(())
    })();
    if result.is_err() {
        spool.invalidate_term_anchors();
    }
    result
}
