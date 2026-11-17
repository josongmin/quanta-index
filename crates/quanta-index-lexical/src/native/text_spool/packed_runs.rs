//! One bounded, disposable posting edit stream for a Text prepare.
//!
//! SQL stores packets, not one row per `(term, owner)`. The packet payload is
//! private to this scratch namespace and never becomes a published format.

use super::*;
use quanta_index_core::index::{MemoryCreditLimits, MemoryLifetime};
use sha2::{Digest as ShaDigest, Sha256};
use std::cmp::Ordering;
use std::collections::HashMap;

mod cold_run;

use cold_run::ColdRun;

const PACKET_BYTES: usize = 128 * 1024;
const PACKET_HEADER: usize = 42;
const EDIT_HEADER: usize = 50;
const EDIT_MAGIC: &[u8; 8] = b"QITXPR02";
const GROUPED_EDIT_MAGIC: &[u8; 8] = b"QITXPR03";
const REPEATED_TERM: u16 = 1 << 15;
const TERM_MAGIC: &[u8; 8] = b"QITXTM01";
const FAN_IN: usize = 64;
const DIRECT_FAN_IN: usize = 512;
const MAX_EDIT_ROWS: usize = 1_772;
const MAX_REFERENCE_IDS: usize = MAX_EDIT_ROWS * 2;
const PACKET_WRITER_MEMORY: usize = PACKET_BYTES + MAX_EDIT_ROWS * 512;

fn corrupt(message: &'static str) -> IndexError {
    IndexError::CorruptObject(format!("posting scratch packet: {message}"))
}

fn width(value: usize) -> Result<u64, IndexError> {
    u64::try_from(value).map_err(|_error| IndexError::ResourceLimited)
}

fn measured_copy(bytes: usize, budget: &mut WorkBudget) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        bytes: width(bytes)?,
        ..WorkUsage::default()
    })
}

/// Attribute accepted packet work without changing admission or execution.
/// Lanes 36/37 separate sorting from body recopy; lane 38 includes SQL outcome.
fn observe_packet_work<T>(
    budget: &mut WorkBudget,
    lane: u16,
    operation: impl FnOnce(&mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let Some(diagnostic) = budget.request().diagnostic_context().cloned() else {
        return operation(budget);
    };
    let span = match diagnostic.enter(
        quanta_index_core::DiagnosticStageV1::NativeTextPostingEdit,
        quanta_index_core::DiagnosticLaneV1(lane),
    ) {
        Ok(span) => span,
        Err(error) => {
            diagnostic.health(match error {
                quanta_index_core::DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    quanta_index_core::DiagnosticHealthCodeV1::CounterOverflow
                }
                quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed => {
                    quanta_index_core::DiagnosticHealthCodeV1::ClockRegressed
                }
                quanta_index_core::DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    quanta_index_core::DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                quanta_index_core::DiagnosticTraceErrorV1::MissingParentSpan => {
                    quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan
                }
            });
            return operation(budget);
        }
    };
    let before = budget.usage();
    let child = diagnostic.child_context(&span);
    let result = {
        let mut scoped = budget.scoped_diagnostic_context(Some(child));
        let result = operation(&mut scoped);
        scoped.record_diagnostic_usage_delta(before, &span);
        result
    };
    let _finished = span.finish(quanta_index_core::DiagnosticTerminalV1::from_result(
        &result,
    ));
    result
}

fn term_page_digest(terms: &TermPage) -> Result<[u8; 32], IndexError> {
    let mut digest = Sha256::new();
    digest.update(width(terms.len())?.to_be_bytes());
    digest.update([u8::from(terms.case_ranges)]);
    for term in terms.iter() {
        digest.update(width(term.as_slice().len())?.to_be_bytes());
        digest.update(term.as_slice());
    }
    Ok(digest.finalize().into())
}

impl TermPage {
    fn count_digest(&self, budget: &mut WorkBudget) -> Result<[u8; 32], IndexError> {
        const DOMAIN: &[u8] = b"qidx.native.text.term-count-page.v1\0";
        let counts = self
            .counts
            .as_ref()
            .ok_or_else(|| corrupt("count data absent"))?;
        let bytes = self.keys.iter().try_fold(DOMAIN.len() + 9, |total, key| {
            total
                .checked_add(24)
                .and_then(|n| n.checked_add(key.as_slice().len()))
                .ok_or(IndexError::ResourceLimited)
        })?;
        // This new bounded integrity pass is real work; only the eliminated
        // SQL read and packet-body copy disappear from the old path.
        measured_copy(bytes, budget)?;
        let mut digest = Sha256::new();
        digest.update(DOMAIN);
        digest.update(width(self.keys.len())?.to_be_bytes());
        digest.update([u8::from(self.case_ranges)]);
        for (slot, key) in self.keys.iter().enumerate() {
            budget.checkpoint()?;
            let delta = counts.deltas.get(slot).ok_or(IndexError::ResourceLimited)?;
            digest.update(width(key.as_slice().len())?.to_be_bytes());
            digest.update(key.as_slice());
            digest.update(delta.added.to_be_bytes());
            digest.update(delta.removed.to_be_bytes());
        }
        Ok(digest.finalize().into())
    }

    pub(super) fn seal_counts(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        let digest = self.count_digest(budget)?;
        self.counts
            .as_mut()
            .ok_or(IndexError::ResourceLimited)?
            .digest = Some(digest);
        Ok(())
    }

    pub(super) fn check_counts(
        &self,
        spool: &TextSpool,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        spool.postings.check_page_source(spool)?;
        let counts = self
            .counts
            .as_ref()
            .ok_or_else(|| corrupt("count data absent"))?;
        let source = spool.postings.page_source.borrow();
        if source
            .as_ref()
            .is_none_or(|current| !Rc::ptr_eq(current, &counts.source))
        {
            return Err(corrupt("count data belongs to another scratch source"));
        }
        if counts.digest != Some(self.count_digest(budget)?) {
            return Err(corrupt("count page key or delta seal differs"));
        }
        Ok(())
    }
}

impl DensePostingPrepassProof {
    pub(in crate::native) fn check_page(
        &self,
        spool: &TextSpool,
        terms: &TermPage,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        spool.check_posting_usable()?;
        if self.page_digest != term_page_digest(terms)? {
            return Err(corrupt("cardinality term page differs"));
        }
        self.check_epoch(spool, budget)
    }

    pub(in crate::native) fn check_epoch(
        &self,
        spool: &TextSpool,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        spool.check_posting_usable()?;
        spool.check_posting_epoch(self.changes)
    }
}

pub(super) struct PostingRow {
    pub term: Vec<u8>,
    pub owner: [u8; 32],
    pub expected: Option<MapValue>,
    pub next: Option<MapValue>,
    _memory: Option<MemoryLease>,
}

impl PostingRow {
    pub(super) fn ordinal(
        &self,
        root: u32,
        budget: &WorkBudget,
    ) -> Result<OrdinalMapEdit, IndexError> {
        RowView::from(self).ordinal(root, budget)
    }
}

#[derive(Clone, Copy)]
pub(super) struct RowView<'a> {
    pub(super) term: &'a [u8],
    pub(super) owner: [u8; 32],
    expected: Option<MapValue>,
    next: Option<MapValue>,
}

impl<'a> From<&'a PostingRow> for RowView<'a> {
    fn from(row: &'a PostingRow) -> Self {
        Self {
            term: &row.term,
            owner: row.owner,
            expected: row.expected,
            next: row.next,
        }
    }
}

impl RowView<'_> {
    fn check(self) -> Result<(), IndexError> {
        check_term_key(self.term)
    }

    fn into_owned(self, budget: &mut WorkBudget) -> Result<PostingRow, IndexError> {
        let memory = budget.reserve_memory(width(self.term.len())?, MemoryLifetime::Temporary)?;
        measured_copy(self.term.len(), budget)?;
        Ok(PostingRow {
            term: self.term.to_vec(),
            owner: self.owner,
            expected: self.expected,
            next: self.next,
            _memory: memory,
        })
    }

    pub(super) fn ordinal(
        self,
        root: u32,
        budget: &WorkBudget,
    ) -> Result<OrdinalMapEdit, IndexError> {
        OrdinalMapEdit::copy_key(root, &self.owner, self.expected, self.next, budget)
    }
}

pub(super) struct PostingRuns {
    pending: RefCell<Option<InitialRun>>,
    cold: bool,
    last_cold_owner: Cell<Option<[u8; 32]>>,
    phase: Cell<i64>,
    runs: Cell<i64>,
    sealed: Cell<bool>,
    poisoned: Cell<bool>,
    effect_epoch: Cell<u64>,
    page_source: RefCell<Option<Rc<PostingPageSource>>>,
}

impl PostingRuns {
    pub(super) fn new(cold: bool) -> Self {
        Self {
            pending: RefCell::new(None),
            cold,
            last_cold_owner: Cell::new(None),
            phase: Cell::new(0),
            runs: Cell::new(0),
            sealed: Cell::new(false),
            poisoned: Cell::new(false),
            effect_epoch: Cell::new(0),
            page_source: RefCell::new(None),
        }
    }

    pub(super) fn check_page_source(&self, spool: &TextSpool) -> Result<(), IndexError> {
        if self.sealed.get()
            && let Some(source) = self.page_source.borrow().as_ref()
            && spool.db.change_count()? != source.expected_changes.get()
        {
            return Err(corrupt("sealed posting scratch source changed"));
        }
        Ok(())
    }

    pub(super) fn page_source(
        &self,
        spool: &TextSpool,
        budget: &WorkBudget,
    ) -> Result<Rc<PostingPageSource>, IndexError> {
        self.check_page_source(spool)?;
        if let Some(source) = self.page_source.borrow().as_ref() {
            return Ok(Rc::clone(source));
        }
        // Include the Rc strong/weak counters before allocating its backing.
        let bytes = std::mem::size_of::<PostingPageSource>()
            .checked_add(2 * std::mem::size_of::<usize>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(width(bytes)?, MemoryLifetime::Temporary)?;
        let source = Rc::new(PostingPageSource {
            expected_changes: Cell::new(spool.db.change_count()?),
            _memory: memory,
        });
        *self.page_source.borrow_mut() = Some(Rc::clone(&source));
        Ok(source)
    }

    pub(super) fn accept_top_changes(&self, spool: &TextSpool) -> Result<(), IndexError> {
        if let Some(source) = self.page_source.borrow().as_ref() {
            source.expected_changes.set(spool.db.change_count()?);
        }
        Ok(())
    }

    fn mark_physical_effect(&self) -> Result<(), IndexError> {
        let next = self
            .effect_epoch
            .get()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.effect_epoch.set(next);
        Ok(())
    }

    pub(super) fn check(&self) -> Result<(), IndexError> {
        if self.poisoned.get() {
            return Err(IndexError::BackendViolation(
                "posting scratch packet was abandoned".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn append<T>(
        &self,
        spool: &TextSpool,
        rows: &[T],
        fields: impl for<'a> Fn(&'a T) -> (&'a [u8], quanta_index_scratch_sqlite::MapEditFields<'a>),
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let before_effect = self.effect_epoch.get();
        let mut pending_changed = false;
        let result = (|| {
            spool.check_posting_usable()?;
            if self.sealed.get() {
                return Err(IndexError::BackendViolation(
                    "posting scratch was sealed".into(),
                ));
            }
            if rows.is_empty()
                || rows.len() > quanta_index_scratch_sqlite::MAX_WIDE_WRITE_BATCH_ROWS
            {
                return Err(IndexError::InvalidInput(
                    "invalid posting scratch edit batch".into(),
                ));
            }
            // Preserve the original logical edit admission. Packet insertion
            // below separately charges each real SQL row and its physical
            // bytes; removed SQL text/bind construction is no longer work.
            let mut logical_bytes = 0_u64;
            let previous_owner = self.last_cold_owner.get();
            let mut last_owner = previous_owner.as_ref().map(|owner| owner.as_slice());
            for row in rows {
                budget.checkpoint()?;
                let (term, edit) = fields(row);
                check_term_key(term)?;
                if edit.key.len() != 32 {
                    return Err(corrupt("owner width"));
                }
                if self.cold {
                    if edit.expected.is_some() || !matches!(edit.next, Some(MapValue::Object(_))) {
                        return Err(corrupt("cold run requires object insertions"));
                    }
                    if last_owner.is_some_and(|previous| previous > edit.key) {
                        return Err(corrupt("cold posting owners are unordered"));
                    }
                    last_owner = Some(edit.key);
                }
                let old = edit
                    .expected
                    .map_or(Ok(1_usize), |value| value.encoded_len())?;
                let next = edit.next.map_or(Ok(1_usize), |value| value.encoded_len())?;
                let fields_bytes = term
                    .len()
                    .checked_add(edit.key.len())
                    .and_then(|bytes| bytes.checked_add(64))
                    .and_then(|bytes| bytes.checked_add(old))
                    .and_then(|bytes| bytes.checked_add(next))
                    .ok_or(IndexError::ResourceLimited)?;
                logical_bytes = logical_bytes
                    .checked_add(width(fields_bytes)?)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            budget.charge(WorkUsage {
                writes: width(rows.len())?,
                bytes: logical_bytes,
                ..WorkUsage::default()
            })?;
            // The source token belongs to the producing spool's Original,
            // never a later reader whose cursor may have already returned.
            let _source = self.page_source(spool, budget)?;
            for row in rows {
                budget.checkpoint()?;
                let (term, edit) = fields(row);
                check_term_key(term)?;
                let owner: [u8; 32] = edit
                    .key
                    .try_into()
                    .map_err(|_error| corrupt("owner width"))?;
                let row = RowView {
                    term,
                    owner,
                    expected: edit.expected.copied(),
                    next: edit.next.copied(),
                };
                let layout = {
                    let mut pending = self.pending.borrow_mut();
                    if pending.is_none() {
                        *pending = Some(InitialRun::new(self.cold, budget)?);
                    }
                    pending
                        .as_ref()
                        .ok_or(IndexError::ResourceLimited)?
                        .plan_row(row)?
                };
                if layout.is_none() {
                    // A pre-write refusal leaves the previous packet intact.
                    self.flush_pending(spool, budget).inspect_err(|error| {
                        pending_changed |=
                            !matches!(error, IndexError::ResourceLimited | IndexError::Cancelled);
                    })?;
                }
                let mut pending = self.pending.borrow_mut();
                let writer = pending.as_mut().ok_or(IndexError::ResourceLimited)?;
                let layout = match layout {
                    Some(layout) => layout,
                    None => writer.plan_row(row)?.ok_or(IndexError::ResourceLimited)?,
                };
                writer.push(row, layout, budget, &mut pending_changed)?;
            }
            if let Some(owner) = last_owner {
                self.last_cold_owner
                    .set(Some(owner.try_into().map_err(|_| corrupt("owner width"))?));
            }
            Ok(())
        })();
        if result.is_err() && (pending_changed || self.effect_epoch.get() != before_effect) {
            self.poisoned.set(true);
            spool.inventory_poisoned.set(true);
        }
        result
    }

    fn flush_pending(&self, spool: &TextSpool, budget: &mut WorkBudget) -> Result<(), IndexError> {
        let mut pending = self.pending.borrow_mut();
        let Some(writer) = pending.as_mut() else {
            return budget.checkpoint();
        };
        if writer.packet().count == 0 {
            return budget.checkpoint();
        }
        let phase = self.phase.get();
        let run = self.runs.get();
        writer.finish(spool, phase, run, budget)?;
        self.runs
            .set(run.checked_add(1).ok_or(IndexError::ResourceLimited)?);
        Ok(())
    }

    pub(super) fn seal(
        &self,
        spool: &TextSpool,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let before_effect = self.effect_epoch.get();
        let result = (|| {
            self.check()?;
            if self.sealed.get() {
                return budget.checkpoint();
            }
            self.flush_pending(spool, budget)?;
            // The initial run writer is empty after a successful flush.
            // Refund its backing before funding merge readers and packets.
            drop(self.pending.borrow_mut().take());
            let mut phase = self.phase.get();
            let mut runs = self.runs.get();
            let mut term_packets_written = false;
            // A single admitted heap merge avoids writing and reading a full
            // intermediate phase. Failure to obtain optional Original credit
            // keeps the existing 64-way path and does not record a refusal.
            if usize::try_from(runs)
                .is_ok_and(|count| (FAN_IN + 1..=DIRECT_FAN_IN).contains(&count))
            {
                let count = usize::try_from(runs).map_err(|_error| IndexError::ResourceLimited)?;
                let limits = direct_merge_credit(count)?;
                if let Some(mut credit) = budget.try_scoped_memory_credit(limits)? {
                    let next = phase.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    merge_group(
                        spool,
                        phase,
                        0..runs,
                        next,
                        0,
                        true,
                        DIRECT_FAN_IN,
                        &mut credit,
                    )?;
                    phase = next;
                    runs = 1;
                    term_packets_written = true;
                }
            }
            while runs > 1 {
                budget.checkpoint()?;
                let final_merge =
                    runs <= i64::try_from(FAN_IN).map_err(|_error| IndexError::ResourceLimited)?;
                let next = phase.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                let mut output = 0_i64;
                let mut first = 0_i64;
                while first < runs {
                    let end = first
                        .saturating_add(
                            i64::try_from(FAN_IN).map_err(|_error| IndexError::ResourceLimited)?,
                        )
                        .min(runs);
                    merge_group(
                        spool,
                        phase,
                        first..end,
                        next,
                        output,
                        final_merge,
                        FAN_IN,
                        budget,
                    )?;
                    output = output.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    first = end;
                }
                phase = next;
                runs = output;
                if final_merge {
                    term_packets_written = true;
                }
            }
            self.phase.set(phase);
            self.runs.set(runs);
            if runs != 0 && !term_packets_written {
                build_term_packets(spool, phase, budget)?;
            }
            if let Some(source) = self.page_source.borrow().as_ref() {
                source.expected_changes.set(spool.db.change_count()?);
            }
            self.sealed.set(true);
            Ok(())
        })();
        if let Err(error) = &result {
            let retryable_before_effect = self.effect_epoch.get() == before_effect
                && matches!(error, IndexError::ResourceLimited | IndexError::Cancelled);
            if !retryable_before_effect {
                self.poisoned.set(true);
                spool.inventory_poisoned.set(true);
            }
        }
        result
    }

    pub(super) fn final_phase(&self) -> Result<Option<i64>, IndexError> {
        self.check()?;
        if !self.sealed.get() {
            return Err(IndexError::BackendViolation(
                "posting scratch not sealed".into(),
            ));
        }
        Ok((self.runs.get() == 1).then_some(self.phase.get()))
    }
}

enum InitialRun {
    General(PacketWriter),
    Cold(ColdRun),
}

impl InitialRun {
    fn new(cold: bool, budget: &WorkBudget) -> Result<Self, IndexError> {
        if cold {
            Ok(Self::Cold(ColdRun::new(budget)?))
        } else {
            Ok(Self::General(PacketWriter::new(
                budget,
                MemoryLifetime::Retained,
            )?))
        }
    }

    fn packet(&self) -> &PacketWriter {
        match self {
            Self::General(writer) => writer,
            Self::Cold(writer) => &writer.packet,
        }
    }

    fn plan_row(&self, row: RowView<'_>) -> Result<Option<RowLayout>, IndexError> {
        match self {
            Self::General(writer) => writer.plan_row(row),
            Self::Cold(writer) => writer.plan_row(row),
        }
    }

    fn push(
        &mut self,
        row: RowView<'_>,
        layout: RowLayout,
        budget: &mut WorkBudget,
        pending_changed: &mut bool,
    ) -> Result<(), IndexError> {
        let before = self.packet().backing_lengths();
        let result = match self {
            Self::General(writer) => writer.push_unordered(row, layout, budget),
            Self::Cold(writer) => writer.push(row, layout, budget),
        };
        *pending_changed |= result.is_ok() || before != self.packet().backing_lengths();
        result
    }

    fn finish(
        &mut self,
        spool: &TextSpool,
        phase: i64,
        run: i64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        match self {
            Self::General(writer) => writer.sort_and_finish(spool, phase, run, budget),
            Self::Cold(writer) => writer.finish(spool, phase, run, budget),
        }
    }
}

struct PacketWriter {
    bytes: Vec<u8>,
    count: u16,
    first: Option<RowOffset>,
    last: Option<RowOffset>,
    page: i64,
    offsets: Vec<RowOffset>,
    sort_work: usize,
    owners: Vec<[u8; 32]>,
    references: Vec<ObjectRef>,
    owner_ids: HashMap<[u8; 32], u16>,
    reference_ids: HashMap<ObjectRef, u16>,
    _memory: Option<MemoryLease>,
}

#[derive(Clone, Copy)]
struct RowLayout {
    row_len: usize,
    dictionary_bytes: usize,
}

#[derive(Clone, Copy)]
struct RowOffset {
    start: usize,
    end: usize,
    term_start: usize,
    term_end: usize,
    owner_id: u16,
}

impl PacketWriter {
    fn backing_lengths(&self) -> (usize, usize, usize) {
        (self.bytes.len(), self.owners.len(), self.references.len())
    }

    fn fence(&self, row: RowOffset) -> (&[u8], [u8; 32]) {
        (
            &self.bytes[row.term_start..row.term_end],
            self.owners[usize::from(row.owner_id)],
        )
    }

    fn new(budget: &WorkBudget, lifetime: MemoryLifetime) -> Result<Self, IndexError> {
        // The bounded payload, row offsets, dictionaries and their hash
        // indices share one Original grant.
        let memory = budget.reserve_memory(width(PACKET_WRITER_MEMORY)?, lifetime)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(PACKET_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        bytes.resize(EDIT_HEADER, 0);
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(MAX_EDIT_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut references = Vec::new();
        references
            .try_reserve_exact(MAX_REFERENCE_IDS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut offsets = Vec::new();
        offsets
            .try_reserve_exact(MAX_EDIT_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut owner_ids = HashMap::new();
        owner_ids
            .try_reserve(MAX_EDIT_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let mut reference_ids = HashMap::new();
        reference_ids
            .try_reserve(MAX_REFERENCE_IDS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            bytes,
            count: 0,
            first: None,
            last: None,
            page: 0,
            offsets,
            sort_work: 0,
            owners,
            references,
            owner_ids,
            reference_ids,
            _memory: memory,
        })
    }

    fn value_len(value: Option<MapValue>) -> Result<usize, IndexError> {
        match value {
            None => Ok(1),
            Some(MapValue::Object(_)) => Ok(3),
            Some(value @ MapValue::Inline(_)) => value
                .encoded_len()?
                .checked_add(3)
                .ok_or(IndexError::ResourceLimited),
        }
    }

    fn new_dictionary_bytes(&self, row: RowView<'_>) -> Result<usize, IndexError> {
        let mut count = usize::from(!self.owner_ids.contains_key(&row.owner));
        let prior = match row.expected {
            Some(MapValue::Object(reference)) => Some(reference),
            _ => None,
        };
        let next = match row.next {
            Some(MapValue::Object(reference)) => Some(reference),
            _ => None,
        };
        if prior.is_some_and(|reference| !self.reference_ids.contains_key(&reference)) {
            count += 1;
        }
        if next.is_some_and(|reference| !self.reference_ids.contains_key(&reference))
            && next != prior
        {
            count += 1;
        }
        count.checked_mul(32).ok_or(IndexError::ResourceLimited)
    }

    fn compact_row_len(row: RowView<'_>) -> Result<usize, IndexError> {
        let old = Self::value_len(row.expected)?;
        let next = Self::value_len(row.next)?;
        2usize
            .checked_add(row.term.len())
            .and_then(|bytes| bytes.checked_add(2))
            .and_then(|bytes| bytes.checked_add(old))
            .and_then(|bytes| bytes.checked_add(next))
            .ok_or(IndexError::ResourceLimited)
    }

    fn plan_row(&self, row: RowView<'_>) -> Result<Option<RowLayout>, IndexError> {
        self.plan_row_at(row, self.bytes.len())
    }

    fn plan_row_at(
        &self,
        row: RowView<'_>,
        body_end: usize,
    ) -> Result<Option<RowLayout>, IndexError> {
        if self.count == u16::MAX || usize::from(self.count) == MAX_EDIT_ROWS {
            return Ok(None);
        }
        let row_len = Self::compact_row_len(row)?;
        let dictionary_bytes = self.new_dictionary_bytes(row)?;
        let used = body_end
            .checked_add(
                self.owners
                    .len()
                    .checked_mul(32)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .and_then(|bytes| bytes.checked_add(self.references.len().checked_mul(32)?))
            .and_then(|bytes| bytes.checked_add(row_len))
            .and_then(|bytes| bytes.checked_add(dictionary_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        Ok((used <= PACKET_BYTES).then_some(RowLayout {
            row_len,
            dictionary_bytes,
        }))
    }

    fn can_push(&self, row: RowView<'_>) -> Result<bool, IndexError> {
        Ok(self.plan_row(row)?.is_some())
    }

    fn reference_id(&mut self, reference: ObjectRef) -> Result<u16, IndexError> {
        if let Some(id) = self.reference_ids.get(&reference) {
            return Ok(*id);
        }
        let id =
            u16::try_from(self.references.len()).map_err(|_error| IndexError::ResourceLimited)?;
        self.references.push(reference);
        if self.reference_ids.insert(reference, id).is_some() {
            return Err(corrupt("reference dictionary identity changed"));
        }
        Ok(id)
    }

    fn owner_id(&mut self, owner: [u8; 32]) -> Result<u16, IndexError> {
        if let Some(id) = self.owner_ids.get(&owner) {
            return Ok(*id);
        }
        let id = u16::try_from(self.owners.len()).map_err(|_error| IndexError::ResourceLimited)?;
        self.owners.push(owner);
        if self.owner_ids.insert(owner, id).is_some() {
            return Err(corrupt("owner dictionary identity changed"));
        }
        Ok(id)
    }

    fn put_reference_id(bytes: &mut Vec<u8>, id: u16) {
        bytes.push(1);
        bytes.extend_from_slice(&id.to_be_bytes());
    }

    fn put_row_key(
        bytes: &mut Vec<u8>,
        term: &[u8],
        owner_id: u16,
    ) -> Result<RowOffset, IndexError> {
        let start = bytes.len();
        let length = u16::try_from(term.len()).map_err(|_error| IndexError::ResourceLimited)?;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(term);
        bytes.extend_from_slice(&owner_id.to_be_bytes());
        Ok(RowOffset {
            start,
            end: bytes.len(),
            term_start: start + 2,
            term_end: start + 2 + term.len(),
            owner_id,
        })
    }

    fn put_value(&mut self, value: Option<MapValue>) -> Result<(), IndexError> {
        match value {
            None => self.bytes.push(0),
            Some(MapValue::Object(reference)) => {
                let id = self.reference_id(reference)?;
                Self::put_reference_id(&mut self.bytes, id);
            }
            Some(value @ MapValue::Inline(_)) => {
                let encoded = value.encoded()?;
                let wire = encoded.as_slice()?;
                let length =
                    u16::try_from(wire.len()).map_err(|_error| IndexError::ResourceLimited)?;
                self.bytes.push(2);
                self.bytes.extend_from_slice(&length.to_be_bytes());
                self.bytes.extend_from_slice(wire);
            }
        }
        Ok(())
    }

    fn push(&mut self, row: RowView<'_>, budget: &mut WorkBudget) -> Result<(), IndexError> {
        self.push_inner(row, budget, false, None)
    }

    fn push_unordered(
        &mut self,
        row: RowView<'_>,
        layout: RowLayout,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.push_inner(row, budget, true, Some(layout))
    }

    fn push_inner(
        &mut self,
        row: RowView<'_>,
        budget: &mut WorkBudget,
        unordered: bool,
        layout: Option<RowLayout>,
    ) -> Result<(), IndexError> {
        row.check()?;
        let layout = match layout {
            Some(layout) => layout,
            None => self.plan_row(row)?.ok_or(IndexError::ResourceLimited)?,
        };
        if !unordered
            && self.last.is_some_and(|last| {
                let (term, owner) = self.fence(last);
                term.cmp(row.term).then(owner.cmp(&row.owner)) != Ordering::Less
            })
        {
            return Err(corrupt("packet rows are unordered"));
        }
        let row_len = layout.row_len;
        let dictionary_bytes = layout.dictionary_bytes;
        let copied_dictionary = dictionary_bytes
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        let copy = row_len
            .checked_add(copied_dictionary)
            .ok_or(IndexError::ResourceLimited)?;
        measured_copy(copy, budget)?;
        let owner_id = self.owner_id(row.owner)?;
        let mut offset = Self::put_row_key(&mut self.bytes, row.term, owner_id)?;
        self.put_value(row.expected)?;
        self.put_value(row.next)?;
        offset.end = self.bytes.len();
        if unordered {
            self.offsets.push(offset);
            let old = row.expected.map_or(Ok(0), |value| value.encoded_len())?;
            let next = row.next.map_or(Ok(0), |value| value.encoded_len())?;
            self.sort_work = self
                .sort_work
                .checked_add(
                    2usize
                        .checked_add(row.term.len())
                        .and_then(|bytes| bytes.checked_add(32 + 2 + old + 2 + next))
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
        } else {
            let _first = self.first.get_or_insert(offset);
            self.last = Some(offset);
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }

    fn sort_and_finish(
        &mut self,
        spool: &TextSpool,
        phase: i64,
        run: i64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.offsets.len() != usize::from(self.count) {
            return Err(corrupt("pending row count"));
        }
        if self.page == 0 && self.pending_is_ordered(budget)? {
            return self.finish_ordered_pending(spool, phase, run, budget);
        }
        observe_packet_work(budget, 38, |budget| {
            self.finish_unordered(spool, phase, run, budget)
        })
    }

    fn finish_ordered_pending(
        &mut self,
        spool: &TextSpool,
        phase: i64,
        run: i64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let first = *self.offsets.first().ok_or(IndexError::ResourceLimited)?;
        let last = *self.offsets.last().ok_or(IndexError::ResourceLimited)?;
        let body_len = self.bytes.len();
        let header: [u8; EDIT_HEADER] = self.bytes[..EDIT_HEADER]
            .try_into()
            .map_err(|_error| corrupt("pending packet header"))?;
        let prior = (self.first, self.last, self.page);
        self.first = Some(first);
        self.last = Some(last);
        if let Err(error) = self.finish(spool, phase, run, budget) {
            // A pre-effect refusal may leave the dictionaries and seal in the
            // buffer. Restore the pending rows so the next attempt can seal.
            self.bytes.truncate(body_len);
            self.bytes[..EDIT_HEADER].copy_from_slice(&header);
            (self.first, self.last, self.page) = prior;
            return Err(error);
        }
        self.page = 0;
        Ok(())
    }

    fn finish_unordered(
        &mut self,
        spool: &TextSpool,
        phase: i64,
        run: i64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        observe_packet_work(budget, 36, |budget| {
            budget.charge(WorkUsage {
                bytes: width(self.sort_work)?,
                ..WorkUsage::default()
            })?;
            let bytes = &self.bytes;
            let owners = &self.owners;
            self.offsets.sort_unstable_by(|left, right| {
                bytes[left.term_start..left.term_end]
                    .cmp(&bytes[right.term_start..right.term_end])
                    .then(
                        owners[usize::from(left.owner_id)]
                            .cmp(&owners[usize::from(right.owner_id)]),
                    )
            });
            for pair in self.offsets.windows(2) {
                let left = &pair[0];
                let right = &pair[1];
                if bytes[left.term_start..left.term_end]
                    .cmp(&bytes[right.term_start..right.term_end])
                    .then(
                        owners[usize::from(left.owner_id)]
                            .cmp(&owners[usize::from(right.owner_id)]),
                    )
                    != Ordering::Less
                {
                    return Err(corrupt("duplicate posting key in initial run"));
                }
            }
            Ok(())
        })?;
        let _sort_memory =
            budget.reserve_memory(width(PACKET_BYTES)?, MemoryLifetime::Temporary)?;
        let mut ordered = Vec::new();
        ordered
            .try_reserve_exact(PACKET_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        ordered.resize(EDIT_HEADER, 0);
        observe_packet_work(budget, 37, |budget| {
            measured_copy(
                self.bytes
                    .len()
                    .checked_sub(EDIT_HEADER)
                    .ok_or(IndexError::ResourceLimited)?,
                budget,
            )?;
            for row in &self.offsets {
                ordered.extend_from_slice(&self.bytes[row.start..row.end]);
            }
            Ok(())
        })?;
        let first = *self.offsets.first().ok_or(IndexError::ResourceLimited)?;
        let last = *self.offsets.last().ok_or(IndexError::ResourceLimited)?;
        finalize_compact_packet(
            &mut ordered,
            self.count,
            &self.owners,
            &self.references,
            budget,
        )?;
        spool.insert_posting_packet(
            phase,
            run,
            self.page,
            PostingPacketFences {
                first: self.fence(first),
                last: self.fence(last),
            },
            &ordered,
            budget,
        )?;
        // Each sorted Pending packet is a separate phase-0 run. The next
        // run starts at page zero; merged writers retain their own page count.
        self.page = 0;
        self.clear_rows();
        Ok(())
    }

    fn pending_is_ordered(&self, budget: &WorkBudget) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        let mut previous = None;
        let mut end = EDIT_HEADER;
        for row in &self.offsets {
            budget.checkpoint()?;
            // A failed fallback may have sorted offsets while leaving the
            // payload in insertion order. Only physical row order can reuse
            // the existing packet body without copying it.
            if row.start != end
                || previous
                    .is_some_and(|prior| self.fence(prior).cmp(&self.fence(*row)) != Ordering::Less)
            {
                return Ok(false);
            }
            previous = Some(*row);
            end = row.end;
        }
        Ok(previous.is_some() && end == self.bytes.len())
    }

    fn finish(
        &mut self,
        spool: &TextSpool,
        phase: i64,
        run: i64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.count == 0 {
            return budget.checkpoint();
        }
        finalize_compact_packet(
            &mut self.bytes,
            self.count,
            &self.owners,
            &self.references,
            budget,
        )?;
        let first = self.first.ok_or(IndexError::ResourceLimited)?;
        let last = self.last.ok_or(IndexError::ResourceLimited)?;
        spool.insert_posting_packet(
            phase,
            run,
            self.page,
            PostingPacketFences {
                first: self.fence(first),
                last: self.fence(last),
            },
            &self.bytes,
            budget,
        )?;
        self.page = self
            .page
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.clear_rows();
        Ok(())
    }

    fn clear_rows(&mut self) {
        self.bytes.truncate(EDIT_HEADER);
        self.count = 0;
        self.first = None;
        self.last = None;
        self.owners.clear();
        self.references.clear();
        self.owner_ids.clear();
        self.reference_ids.clear();
        self.offsets.clear();
        self.sort_work = 0;
    }
}

fn finalize_compact_packet(
    bytes: &mut Vec<u8>,
    count: u16,
    owners: &[[u8; 32]],
    references: &[ObjectRef],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let row_end = u32::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let owner_count = u16::try_from(owners.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let reference_count =
        u16::try_from(references.len()).map_err(|_error| IndexError::ResourceLimited)?;
    let dictionary_bytes = owners
        .len()
        .checked_add(references.len())
        .and_then(|count| count.checked_mul(32))
        .ok_or(IndexError::ResourceLimited)?;
    if bytes
        .len()
        .checked_add(dictionary_bytes)
        .ok_or(IndexError::ResourceLimited)?
        > PACKET_BYTES
    {
        return Err(IndexError::ResourceLimited);
    }
    measured_copy(dictionary_bytes, budget)?;
    bytes[42..46].copy_from_slice(&row_end.to_be_bytes());
    bytes[46..48].copy_from_slice(&owner_count.to_be_bytes());
    bytes[48..50].copy_from_slice(&reference_count.to_be_bytes());
    for owner in owners {
        bytes.extend_from_slice(owner);
    }
    for reference in references {
        bytes.extend_from_slice(&reference.0.0);
    }
    bytes[..8].copy_from_slice(EDIT_MAGIC);
    bytes[8..10].copy_from_slice(&count.to_be_bytes());
    let digest = Sha256::digest(&bytes[42..]);
    bytes[10..42].copy_from_slice(&digest);
    Ok(())
}

struct Packet {
    bytes: Vec<u8>,
    row_end: usize,
    references_begin: usize,
    last_offset: usize,
    offset: usize,
    remaining: u16,
    grouped_terms: bool,
    reusable: bool,
    _memory: Option<MemoryLease>,
}

struct PostingPacketFences<'a> {
    first: (&'a [u8], [u8; 32]),
    last: (&'a [u8], [u8; 32]),
}

impl Packet {
    fn parts(&self) -> (&[u8], &[u8], &[u8]) {
        (
            &self.bytes[..self.row_end],
            &self.bytes[self.row_end..self.references_begin],
            &self.bytes[self.references_begin..],
        )
    }
    fn parse(
        bytes: Vec<u8>,
        memory: Option<MemoryLease>,
        fences: Option<PostingPacketFences<'_>>,
        start: Option<&[u8]>,
        allow_grouped_terms: bool,
    ) -> Result<Self, IndexError> {
        let grouped_terms = bytes.get(..8) == Some(GROUPED_EDIT_MAGIC.as_slice());
        let known_magic =
            bytes.get(..8) == Some(EDIT_MAGIC.as_slice()) || (allow_grouped_terms && grouped_terms);
        if bytes.len() < EDIT_HEADER || bytes.len() > PACKET_BYTES || !known_magic {
            return Err(corrupt("packet header or size"));
        }
        let count = u16::from_be_bytes(bytes[8..10].try_into().map_err(|_error| corrupt("count"))?);
        let row_end = usize::try_from(u32::from_be_bytes(
            bytes[42..46]
                .try_into()
                .map_err(|_error| corrupt("row end"))?,
        ))
        .map_err(|_error| IndexError::ResourceLimited)?;
        let owner_count = usize::from(u16::from_be_bytes(
            bytes[46..48]
                .try_into()
                .map_err(|_error| corrupt("owners"))?,
        ));
        let reference_count = usize::from(u16::from_be_bytes(
            bytes[48..50]
                .try_into()
                .map_err(|_error| corrupt("references"))?,
        ));
        let references_begin = row_end
            .checked_add(
                owner_count
                    .checked_mul(32)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        let end = references_begin
            .checked_add(
                reference_count
                    .checked_mul(32)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        if count == 0
            || count as usize > MAX_EDIT_ROWS
            || row_end < EDIT_HEADER
            || end != bytes.len()
            || Sha256::digest(&bytes[42..]).as_slice() != &bytes[10..42]
        {
            return Err(corrupt("packet digest or dictionary layout"));
        }
        let mut packet = Self {
            bytes,
            row_end,
            references_begin,
            last_offset: EDIT_HEADER,
            offset: EDIT_HEADER,
            remaining: count,
            grouped_terms,
            reusable: false,
            _memory: memory,
        };
        let mut offset = EDIT_HEADER;
        let mut last_offset = EDIT_HEADER;
        let mut skipped = 0_u16;
        let mut first_selected = EDIT_HEADER;
        let mut first = None;
        let mut prior: Option<(&[u8], [u8; 32])> = None;
        let mut literal_start = None;
        for _ in 0..count {
            let decoded = packet.decode_row(offset)?;
            let RowView { term, owner, .. } = decoded.view;
            if decoded.term_start == offset + 2 {
                literal_start = Some(decoded.term_start);
            } else if literal_start != Some(decoded.term_start) {
                return Err(corrupt("packet repeat points outside its term group"));
            }
            if prior.is_some_and(|(key, before)| {
                key.cmp(term).then(before.cmp(&owner)) != Ordering::Less
            }) {
                return Err(corrupt("packet order"));
            }
            let _first = first.get_or_insert((term, owner));
            if start.is_some_and(|lower| term < lower) {
                skipped = skipped.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                first_selected = decoded.end;
            }
            prior = Some((term, owner));
            last_offset = offset;
            offset = decoded.end;
        }
        if offset != row_end {
            return Err(corrupt("packet trailing rows"));
        }
        if fences.is_some_and(|fences| first != Some(fences.first) || prior != Some(fences.last)) {
            return Err(corrupt("packet SQL fences differ from checked payload"));
        }
        packet.last_offset = last_offset;
        packet.offset = first_selected;
        packet.remaining = count
            .checked_sub(skipped)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(packet)
    }

    fn last_key(&self) -> Result<(&[u8], [u8; 32]), IndexError> {
        let RowView { term, owner, .. } = self.decode_row(self.last_offset)?.view;
        Ok((term, owner))
    }

    fn reposition(&mut self, start: &[u8], budget: &WorkBudget) -> Result<(), IndexError> {
        let count = u16::from_be_bytes(
            self.bytes[8..10]
                .try_into()
                .map_err(|_error| corrupt("checked packet count"))?,
        );
        let mut offset = EDIT_HEADER;
        let mut remaining = count;
        while remaining != 0 {
            budget.checkpoint()?;
            let decoded = self.decode_row(offset)?;
            if decoded.view.term >= start {
                break;
            }
            offset = decoded.end;
            remaining -= 1;
        }
        self.offset = offset;
        self.remaining = remaining;
        Ok(())
    }

    fn next_row<T>(
        &mut self,
        budget: &mut WorkBudget,
        project: impl FnOnce(DecodedPacketRow<'_>, &mut WorkBudget) -> Result<T, IndexError>,
    ) -> Result<Option<T>, IndexError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        budget.checkpoint()?;
        let decoded = self.decode_row(self.offset)?;
        let end = decoded.end;
        // The caller's validation and output admission are part of consuming
        // a row. A failed projection leaves its exact position available.
        let projected = project(decoded, budget)?;
        self.offset = end;
        self.remaining -= 1;
        Ok(Some(projected))
    }

    fn decode_row(&self, offset: usize) -> Result<DecodedPacketRow<'_>, IndexError> {
        let (rows, owners, references) = self.parts();
        decode_compact_row(rows, owners, references, offset, self.grouped_terms)
    }
}

fn read_u16(bytes: &[u8], offset: &mut usize) -> Result<usize, IndexError> {
    let end = offset.checked_add(2).ok_or(IndexError::ResourceLimited)?;
    let raw = bytes
        .get(*offset..end)
        .ok_or_else(|| corrupt("truncated length"))?;
    *offset = end;
    Ok(usize::from(u16::from_be_bytes(
        raw.try_into().map_err(|_error| corrupt("length"))?,
    )))
}

fn decode_compact_value(
    rows: &[u8],
    references: &[u8],
    offset: &mut usize,
) -> Result<Option<MapValue>, IndexError> {
    let tag = *rows
        .get(*offset)
        .ok_or_else(|| corrupt("truncated value tag"))?;
    *offset = offset.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    match tag {
        0 => Ok(None),
        1 => {
            let id = read_u16(rows, offset)?;
            let begin = id.checked_mul(32).ok_or(IndexError::ResourceLimited)?;
            let end = begin.checked_add(32).ok_or(IndexError::ResourceLimited)?;
            let digest: [u8; 32] = references
                .get(begin..end)
                .ok_or_else(|| corrupt("reference dictionary id"))?
                .try_into()
                .map_err(|_error| corrupt("reference dictionary width"))?;
            Ok(Some(MapValue::Object(ObjectRef(Digest(digest)))))
        }
        2 => {
            let length = read_u16(rows, offset)?;
            let end = offset
                .checked_add(length)
                .ok_or(IndexError::ResourceLimited)?;
            let raw = rows
                .get(*offset..end)
                .ok_or_else(|| corrupt("truncated inline value"))?;
            let value = MapValue::decode(raw)?;
            if !matches!(value, MapValue::Inline(_)) {
                return Err(corrupt("noncanonical inline tag"));
            }
            *offset = end;
            Ok(Some(value))
        }
        _ => Err(corrupt("unknown value tag")),
    }
}

struct DecodedPacketRow<'a> {
    view: RowView<'a>,
    term_start: usize,
    end: usize,
}

fn decode_compact_row<'a>(
    rows: &'a [u8],
    owners: &[u8],
    references: &[u8],
    begin: usize,
    grouped_terms: bool,
) -> Result<DecodedPacketRow<'a>, IndexError> {
    let mut offset = begin;
    let encoded = read_u16(rows, &mut offset)?;
    let (term_start, term) = if encoded & usize::from(REPEATED_TERM) != 0 {
        let distance = encoded & usize::from(REPEATED_TERM - 1);
        if !grouped_terms || distance == 0 {
            return Err(corrupt("invalid repeated term marker"));
        }
        let mut literal = begin
            .checked_sub(distance)
            .filter(|&offset| offset >= EDIT_HEADER)
            .ok_or_else(|| corrupt("repeat points outside packet rows"))?;
        let length = read_u16(rows, &mut literal)?;
        let term = rows
            .get(literal..literal + length)
            .filter(|_| literal + length <= begin)
            .ok_or_else(|| corrupt("invalid repeated term literal"))?;
        (literal, term)
    } else {
        let start = offset;
        offset += encoded;
        (
            start,
            rows.get(start..offset)
                .ok_or_else(|| corrupt("truncated term"))?,
        )
    };
    check_term_key(term)?;
    let id = read_u16(rows, &mut offset)?;
    let begin = id.checked_mul(32).ok_or(IndexError::ResourceLimited)?;
    let end = begin.checked_add(32).ok_or(IndexError::ResourceLimited)?;
    let owner: [u8; 32] = owners
        .get(begin..end)
        .ok_or_else(|| corrupt("owner dictionary id"))?
        .try_into()
        .map_err(|_error| corrupt("owner dictionary width"))?;
    let expected = decode_compact_value(rows, references, &mut offset)?;
    let next = decode_compact_value(rows, references, &mut offset)?;
    Ok(DecodedPacketRow {
        view: RowView {
            term,
            owner,
            expected,
            next,
        },
        term_start,
        end: offset,
    })
}

// A head owns no term bytes. Its offsets remain valid while its checked Packet
// is retained by the reader; fill may replace that Packet only after head is
// consumed.
struct RunHead {
    term_start: usize,
    term_end: usize,
    owner: [u8; 32],
    expected: Option<MapValue>,
    next: Option<MapValue>,
}

struct RunReader<'a> {
    spool: &'a TextSpool,
    phase: i64,
    run: i64,
    page: i64,
    packet: Option<Packet>,
    head: Option<RunHead>,
}

impl<'a> RunReader<'a> {
    fn new(spool: &'a TextSpool, phase: i64, run: i64) -> Self {
        Self {
            spool,
            phase,
            run,
            page: 0,
            packet: None,
            head: None,
        }
    }

    fn head_parts(&self) -> Result<(&[u8], &RunHead), IndexError> {
        let head = self.head.as_ref().ok_or(IndexError::ResourceLimited)?;
        let packet = self.packet.as_ref().ok_or(IndexError::ResourceLimited)?;
        let term = packet
            .bytes
            .get(head.term_start..head.term_end)
            .ok_or_else(|| corrupt("run head outside checked packet"))?;
        Ok((term, head))
    }

    fn head_view(&self) -> Result<RowView<'_>, IndexError> {
        let (term, head) = self.head_parts()?;
        Ok(RowView {
            term,
            owner: head.owner,
            expected: head.expected,
            next: head.next,
        })
    }

    fn fill(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.head.is_some() {
            return Ok(());
        }
        loop {
            if let Some(packet) = self.packet.as_mut() {
                if let Some(head) = packet.next_row(budget, |decoded, _budget| {
                    let row = decoded.view;
                    Ok(RunHead {
                        term_start: decoded.term_start,
                        term_end: decoded.term_start + row.term.len(),
                        owner: row.owner,
                        expected: row.expected,
                        next: row.next,
                    })
                })? {
                    self.head = Some(head);
                    return Ok(());
                }
                self.packet = None;
            }
            let Some(packet) = self
                .spool
                .read_posting_packet(self.phase, self.run, self.page, None, None, budget)?
            else {
                return Ok(());
            };
            self.page = self
                .page
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            self.packet = Some(packet);
        }
    }
}

// The one previous key is bounded by the same term-key contract as the packet.
// Repeated owners of a term only update the fixed-width owner witness.
struct PreviousKey {
    term: PostingTermKey,
    owner: [u8; 32],
}

impl PreviousKey {
    fn term(&self) -> &[u8] {
        self.term.as_slice()
    }

    fn admit_next_term(
        prior: Option<&Self>,
        row: RowView<'_>,
        budget: &mut WorkBudget,
        unordered: &'static str,
    ) -> Result<Option<PostingTermKey>, IndexError> {
        if let Some(previous) = prior {
            match previous.term().cmp(row.term) {
                Ordering::Less => {}
                Ordering::Equal if previous.owner < row.owner => return Ok(None),
                _ => return Err(corrupt(unordered)),
            }
        }
        measured_copy(row.term.len(), budget)?;
        PostingTermKey::copy(row.term, budget).map(Some)
    }

    fn advance(prior: &mut Option<Self>, term: Option<PostingTermKey>, owner: [u8; 32]) {
        if let Some(term) = term {
            *prior = Some(Self { term, owner });
        } else if let Some(previous) = prior {
            previous.owner = owner;
        }
    }
}

fn direct_merge_credit(runs: usize) -> Result<MemoryCreditLimits, IndexError> {
    if runs < 2 || runs > DIRECT_FAN_IN {
        return Err(IndexError::ResourceLimited);
    }
    // Every reader retains at most one checked 128 KiB packet and one
    // decoded <=1024-byte head. The reader slab includes the existing
    // conservative head grant. The writer, term writer, prior row, and heap
    // stay inside the same temporary child credit until merge_group returns.
    let per_reader = std::mem::size_of::<RunReader<'_>>()
        .checked_add(2 * 1024)
        .and_then(|bytes| bytes.checked_add(PACKET_BYTES))
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<usize>()))
        .ok_or(IndexError::ResourceLimited)?;
    let writer = PACKET_BYTES
        .checked_add(2048)
        .and_then(|bytes| bytes.checked_add(MAX_EDIT_ROWS * 512))
        .ok_or(IndexError::ResourceLimited)?;
    let total = runs
        .checked_mul(per_reader)
        .and_then(|bytes| bytes.checked_add(writer))
        .and_then(|bytes| bytes.checked_add(PACKET_BYTES + 2048 + 2 * 1024))
        .ok_or(IndexError::ResourceLimited)?;
    let slots = runs
        .checked_mul(2)
        .and_then(|count| count.checked_add(16))
        .ok_or(IndexError::ResourceLimited)?;
    Ok(MemoryCreditLimits {
        temporary_data_bytes: width(total)?,
        retained_data_bytes: 0,
        max_live_temporary_leases: u32::try_from(slots)
            .map_err(|_error| IndexError::ResourceLimited)?,
        max_live_retained_leases: 0,
    })
}

struct MergeHeadHeap {
    slots: Vec<usize>,
    _memory: Option<MemoryLease>,
}

impl MergeHeadHeap {
    fn new(capacity: usize, budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(
            width(
                capacity
                    .checked_mul(std::mem::size_of::<usize>())
                    .ok_or(IndexError::ResourceLimited)?,
            )?,
            MemoryLifetime::Temporary,
        )?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if slots.capacity() > capacity {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            slots,
            _memory: memory,
        })
    }

    fn compare(
        readers: &[RunReader<'_>],
        left: usize,
        right: usize,
    ) -> Result<Ordering, IndexError> {
        let (left_term, left_head) = readers
            .get(left)
            .ok_or(IndexError::ResourceLimited)?
            .head_parts()?;
        let (right_term, right_head) = readers
            .get(right)
            .ok_or(IndexError::ResourceLimited)?
            .head_parts()?;
        Ok(left_term
            .cmp(right_term)
            .then(left_head.owner.cmp(&right_head.owner))
            .then(left.cmp(&right)))
    }

    fn push(&mut self, readers: &[RunReader<'_>], slot: usize) -> Result<(), IndexError> {
        if self.slots.len() == self.slots.capacity() {
            return Err(IndexError::ResourceLimited);
        }
        self.slots.push(slot);
        let mut child = self.slots.len() - 1;
        while child != 0 {
            let parent = (child - 1) / 2;
            if Self::compare(readers, self.slots[child], self.slots[parent])? != Ordering::Less {
                break;
            }
            self.slots.swap(child, parent);
            child = parent;
        }
        Ok(())
    }

    fn pop(&mut self, readers: &[RunReader<'_>]) -> Result<Option<usize>, IndexError> {
        let Some(last) = self.slots.pop() else {
            return Ok(None);
        };
        if self.slots.is_empty() {
            return Ok(Some(last));
        }
        let first = std::mem::replace(&mut self.slots[0], last);
        let mut parent = 0_usize;
        loop {
            let left = parent
                .checked_mul(2)
                .and_then(|slot| slot.checked_add(1))
                .ok_or(IndexError::ResourceLimited)?;
            if left >= self.slots.len() {
                break;
            }
            let right = left.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            let mut child = left;
            if right < self.slots.len()
                && Self::compare(readers, self.slots[right], self.slots[left])? == Ordering::Less
            {
                child = right;
            }
            if Self::compare(readers, self.slots[child], self.slots[parent])? != Ordering::Less {
                break;
            }
            self.slots.swap(child, parent);
            parent = child;
        }
        Ok(Some(first))
    }
}

fn merge_group(
    spool: &TextSpool,
    phase: i64,
    runs: std::ops::Range<i64>,
    next: i64,
    output: i64,
    final_merge: bool,
    maximum_runs: usize,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let len =
        usize::try_from(runs.end - runs.start).map_err(|_error| IndexError::ResourceLimited)?;
    if len == 0 || len > maximum_runs {
        return Err(IndexError::ResourceLimited);
    }
    let _heads = budget.reserve_memory(
        width(
            len.checked_mul(std::mem::size_of::<RunReader<'_>>() + 1024)
                .ok_or(IndexError::ResourceLimited)?,
        )?,
        MemoryLifetime::Temporary,
    )?;
    let mut readers = Vec::new();
    readers
        .try_reserve_exact(len)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for run in runs {
        readers.push(RunReader::new(spool, phase, run));
    }
    for reader in &mut readers {
        reader.fill(budget)?;
    }
    let mut heap = MergeHeadHeap::new(len, budget)?;
    for slot in 0..len {
        if readers
            .get(slot)
            .ok_or(IndexError::ResourceLimited)?
            .head
            .is_some()
        {
            heap.push(&readers, slot)?;
        }
    }
    let mut writer = PacketWriter::new(budget, MemoryLifetime::Temporary)?;
    let mut terms = if final_merge {
        Some(TermAccumulator::new(budget)?)
    } else {
        None
    };
    let mut prior: Option<PreviousKey> = None;
    while let Some(slot) = heap.pop(&readers)? {
        budget.checkpoint()?;
        {
            let row = readers
                .get(slot)
                .ok_or(IndexError::ResourceLimited)?
                .head_view()?;
            let term = PreviousKey::admit_next_term(
                prior.as_ref(),
                row,
                budget,
                "duplicate or unordered merge key",
            )?;
            if !writer.can_push(row)? {
                writer.finish(spool, next, output, budget)?;
            }
            writer.push(row, budget)?;
            if let Some(terms) = terms.as_mut() {
                let completed = prior.as_ref().filter(|_| term.is_some());
                terms.observe(spool, completed, row, budget)?;
            }
            PreviousKey::advance(&mut prior, term, row.owner);
        }
        let reader = readers.get_mut(slot).ok_or(IndexError::ResourceLimited)?;
        reader.head = None;
        reader.fill(budget)?;
        if reader.head.is_some() {
            heap.push(&readers, slot)?;
        }
    }
    writer.finish(spool, next, output, budget)?;
    if let Some(terms) = terms {
        terms.finish(spool, prior.as_ref(), budget)?;
    }
    Ok(())
}

struct TermAccumulator {
    writer: TermPacketWriter,
    added: u64,
    removed: u64,
}

impl TermAccumulator {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        Ok(Self {
            writer: TermPacketWriter::new(budget)?,
            added: 0,
            removed: 0,
        })
    }

    fn observe(
        &mut self,
        spool: &TextSpool,
        completed: Option<&PreviousKey>,
        row: RowView<'_>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if let Some(prior) = completed {
            self.writer
                .push(spool, prior.term(), self.added, self.removed, budget)?;
            self.added = 0;
            self.removed = 0;
        }
        if row.expected.is_none() && row.next.is_some() {
            self.added = self
                .added
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        if row.expected.is_some() && row.next.is_none() {
            self.removed = self
                .removed
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        Ok(())
    }

    fn finish(
        mut self,
        spool: &TextSpool,
        prior: Option<&PreviousKey>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if let Some(prior) = prior {
            self.writer
                .push(spool, prior.term(), self.added, self.removed, budget)?;
        }
        self.writer.finish(spool, budget)
    }
}

fn build_term_packets(
    spool: &TextSpool,
    phase: i64,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let mut reader = RunReader::new(spool, phase, 0);
    let mut terms = TermAccumulator::new(budget)?;
    let mut prior: Option<PreviousKey> = None;
    loop {
        reader.fill(budget)?;
        if reader.head.is_none() {
            break;
        }
        let row = reader.head_view()?;
        let term = PreviousKey::admit_next_term(
            prior.as_ref(),
            row,
            budget,
            "duplicate or unordered single-run key",
        )?;
        let completed = prior.as_ref().filter(|_| term.is_some());
        terms.observe(spool, completed, row, budget)?;
        PreviousKey::advance(&mut prior, term, row.owner);
        reader.head = None;
    }
    terms.finish(spool, prior.as_ref(), budget)
}

struct TermPacketWriter {
    bytes: Vec<u8>,
    count: u16,
    first: Option<std::ops::Range<usize>>,
    last: Option<std::ops::Range<usize>>,
    sequence: i64,
    _memory: Option<MemoryLease>,
}

impl TermPacketWriter {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(width(PACKET_BYTES)?, MemoryLifetime::Temporary)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(PACKET_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        bytes.resize(PACKET_HEADER, 0);
        Ok(Self {
            bytes,
            count: 0,
            first: None,
            last: None,
            sequence: 0,
            _memory: memory,
        })
    }
    fn push(
        &mut self,
        spool: &TextSpool,
        term: &[u8],
        added: u64,
        removed: u64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        check_term_key(term)?;
        let length = 2usize
            .checked_add(term.len())
            .and_then(|bytes| bytes.checked_add(16))
            .ok_or(IndexError::ResourceLimited)?;
        if self.count == 64
            || self
                .bytes
                .len()
                .checked_add(length)
                .ok_or(IndexError::ResourceLimited)?
                > PACKET_BYTES
        {
            self.finish(spool, budget)?;
        }
        if self
            .last
            .as_ref()
            .is_some_and(|last| &self.bytes[last.clone()] >= term)
        {
            return Err(corrupt("term metadata order"));
        }
        measured_copy(length, budget)?;
        self.bytes.extend_from_slice(
            &u16::try_from(term.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        let start = self.bytes.len();
        self.bytes.extend_from_slice(term);
        let range = start..self.bytes.len();
        let _first = self.first.get_or_insert_with(|| range.clone());
        self.last = Some(range);
        self.bytes.extend_from_slice(&added.to_be_bytes());
        self.bytes.extend_from_slice(&removed.to_be_bytes());
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }
    fn finish(&mut self, spool: &TextSpool, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.count == 0 {
            return budget.checkpoint();
        }
        self.bytes[..8].copy_from_slice(TERM_MAGIC);
        self.bytes[8..10].copy_from_slice(&self.count.to_be_bytes());
        let digest = Sha256::digest(&self.bytes[PACKET_HEADER..]);
        self.bytes[10..42].copy_from_slice(&digest);
        let first = self.first.clone().ok_or(IndexError::ResourceLimited)?;
        let last = self.last.clone().ok_or(IndexError::ResourceLimited)?;
        spool.insert_term_packet(
            self.sequence,
            &self.bytes[first],
            &self.bytes[last],
            &self.bytes,
            budget,
        )?;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.bytes.truncate(PACKET_HEADER);
        self.count = 0;
        self.first = None;
        self.last = None;
        Ok(())
    }
}

struct TermPacket<'row> {
    bytes: &'row [u8],
    offset: usize,
    remaining: u16,
}

impl<'row> TermPacket<'row> {
    fn parse(bytes: &'row [u8], first: &[u8], last: &[u8]) -> Result<Self, IndexError> {
        if bytes.len() < PACKET_HEADER
            || bytes.len() > PACKET_BYTES
            || bytes.get(..8) != Some(TERM_MAGIC.as_slice())
        {
            return Err(corrupt("term packet header"));
        }
        let count = u16::from_be_bytes(
            bytes[8..10]
                .try_into()
                .map_err(|_error| corrupt("term count"))?,
        );
        if count == 0
            || count > 64
            || Sha256::digest(&bytes[PACKET_HEADER..]).as_slice() != &bytes[10..42]
        {
            return Err(corrupt("term packet digest or count"));
        }
        let mut offset = PACKET_HEADER;
        let mut prior: Option<&[u8]> = None;
        for ordinal in 0..count {
            let (term, _, _, end) = decode_term(bytes, offset)?;
            if ordinal == 0 && term != first {
                return Err(corrupt("term SQL fences differ"));
            }
            if prior.is_some_and(|key| key >= term) {
                return Err(corrupt("term packet order"));
            }
            prior = Some(term);
            offset = end;
        }
        if offset != bytes.len() {
            return Err(corrupt("term packet trailing bytes"));
        }
        if prior != Some(last) {
            return Err(corrupt("term SQL fences differ"));
        }
        Ok(Self {
            bytes,
            offset: PACKET_HEADER,
            remaining: count,
        })
    }
    fn next(&mut self) -> Result<Option<(&[u8], u64, u64)>, IndexError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let (term, added, removed, end) = decode_term(self.bytes, self.offset)?;
        self.offset = end;
        self.remaining -= 1;
        Ok(Some((term, added, removed)))
    }
}

fn decode_term(bytes: &[u8], begin: usize) -> Result<(&[u8], u64, u64, usize), IndexError> {
    let mut offset = begin;
    let length = read_u16(bytes, &mut offset)?;
    let end = offset
        .checked_add(length)
        .ok_or(IndexError::ResourceLimited)?;
    let term = bytes
        .get(offset..end)
        .ok_or_else(|| corrupt("truncated term metadata"))?;
    check_term_key(term)?;
    offset = end;
    let end = offset.checked_add(16).ok_or(IndexError::ResourceLimited)?;
    let pair = bytes
        .get(offset..end)
        .ok_or_else(|| corrupt("truncated term counts"))?;
    let added = u64::from_be_bytes(
        pair[..8]
            .try_into()
            .map_err(|_error| corrupt("added count"))?,
    );
    let removed = u64::from_be_bytes(
        pair[8..]
            .try_into()
            .map_err(|_error| corrupt("removed count"))?,
    );
    Ok((term, added, removed, end))
}

impl TextSpool {
    fn insert_posting_packet(
        &self,
        phase: i64,
        run: i64,
        page: i64,
        fences: PostingPacketFences<'_>,
        payload: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if payload.is_empty() || payload.len() > PACKET_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let incoming = width(payload.len())?
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(4096 + 2 * 1024 + 64))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: 1,
            bytes: incoming,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        self.postings.mark_physical_effect()?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &db,
            self.allocation.as_ref(),
            incoming,
            budget,
        )?;
        let changed = db.execute(
            "INSERT INTO posting_packet(phase,run,page,first_term,first_owner,last_term,last_owner,payload)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![phase, run, page, fences.first.0, fences.first.1.as_slice(), fences.last.0, fences.last.1.as_slice(), payload],
        ).map_err(|error| sql(&error))?;
        if changed != 1 {
            return Err(IndexError::BackendViolation(
                "posting packet insertion count".into(),
            ));
        }
        Ok(())
    }

    fn read_posting_packet(
        &self,
        phase: i64,
        run: i64,
        page: i64,
        start: Option<&[u8]>,
        cache: Option<&PostingPacketCache>,
        budget: &mut WorkBudget,
    ) -> Result<Option<Packet>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        let mut statement = db
            .prepare_cached(
                "SELECT first_term,first_owner,last_term,last_owner,payload FROM posting_packet
             WHERE phase=?1 AND run=?2 AND page=?3",
            )
            .map_err(|error| sql(&error))?;
        let mut rows = statement
            .query(params![phase, run, page])
            .map_err(|error| sql(&error))?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            return Ok(None);
        };
        let first_term = quanta_index_scratch_sqlite::bounded_blob(
            row.get_ref(0).map_err(|error| sql(&error))?,
            1024,
        )?;
        let first_owner = quanta_index_scratch_sqlite::fixed_blob::<32>(
            row.get_ref(1).map_err(|error| sql(&error))?,
        )?;
        let last_term = quanta_index_scratch_sqlite::bounded_blob(
            row.get_ref(2).map_err(|error| sql(&error))?,
            1024,
        )?;
        let last_owner = quanta_index_scratch_sqlite::fixed_blob::<32>(
            row.get_ref(3).map_err(|error| sql(&error))?,
        )?;
        let bytes = quanta_index_scratch_sqlite::bounded_blob(
            row.get_ref(4).map_err(|error| sql(&error))?,
            PACKET_BYTES,
        )?;
        measured_copy(bytes.len(), budget)?;
        let payload_bytes = width(bytes.len())?;
        let spare = if cache.is_some() {
            budget.try_reserve_memory_for_optimization(payload_bytes, MemoryLifetime::Temporary)?
        } else {
            None
        };
        let reusable = spare.is_some();
        let memory = if reusable {
            spare
        } else {
            if let Some(cache) = cache {
                // Optional backing must not compete with this mandatory
                // page when the Original scope is already under pressure.
                cache.clear();
            }
            budget.reserve_memory(payload_bytes, MemoryLifetime::Temporary)?
        };
        let mut packet = Packet::parse(
            bytes.to_vec(),
            memory,
            Some(PostingPacketFences {
                first: (first_term, first_owner),
                last: (last_term, last_owner),
            }),
            start,
            phase == 0 && run > 0 && page == 0 && self.postings.cold,
        )?;
        if rows.next().map_err(|error| sql(&error))?.is_some() {
            return Err(corrupt("duplicate packet identity"));
        }
        packet.reusable = reusable;
        Ok(Some(packet))
    }
}

impl TextSpool {
    fn insert_term_packet(
        &self,
        sequence: i64,
        first: &[u8],
        last: &[u8],
        payload: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if payload.len() > PACKET_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let incoming = width(payload.len())?
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(4096 + 2048))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: 1,
            bytes: incoming,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        self.postings.mark_physical_effect()?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &db,
            self.allocation.as_ref(),
            incoming,
            budget,
        )?;
        let changed=db.execute("INSERT INTO posting_term_packet(seq,first_term,last_term,payload) VALUES(?1,?2,?3,?4)",params![sequence,first,last,payload]).map_err(|error|sql(&error))?;
        if changed != 1 {
            return Err(IndexError::BackendViolation(
                "term packet insertion count".into(),
            ));
        }
        Ok(())
    }

    /// Consume validated metadata while its SQLite row is live. The callback
    /// may only prepare local data; the query must reach EOF before it escapes.
    fn with_term_packet_at<T>(
        &self,
        after: &[u8],
        inclusive: bool,
        budget: &mut WorkBudget,
        consume: impl for<'row> FnOnce(TermPacket<'row>, &mut WorkBudget) -> Result<T, IndexError>,
    ) -> Result<Option<T>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        let query = if inclusive {
            "SELECT first_term,last_term,payload FROM posting_term_packet WHERE last_term>=?1 ORDER BY last_term LIMIT 1"
        } else {
            "SELECT first_term,last_term,payload FROM posting_term_packet WHERE last_term>?1 ORDER BY last_term LIMIT 1"
        };
        let mut statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
        let mut rows = statement
            .query(params![after])
            .map_err(|error| sql(&error))?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            return Ok(None);
        };
        let first = quanta_index_scratch_sqlite::bounded_blob(
            row.get_ref(0).map_err(|error| sql(&error))?,
            1024,
        )?;
        let last = quanta_index_scratch_sqlite::bounded_blob(
            row.get_ref(1).map_err(|error| sql(&error))?,
            1024,
        )?;
        let raw = quanta_index_scratch_sqlite::bounded_blob(
            row.get_ref(2).map_err(|error| sql(&error))?,
            PACKET_BYTES,
        )?;
        let packet = TermPacket::parse(raw, first, last)?;
        if last < after || (!inclusive && last == after) {
            return Err(corrupt("posting term packet did not advance"));
        }
        let result = consume(packet, budget);
        if rows.next().map_err(|error| sql(&error))?.is_some() {
            return Err(corrupt("term query repeated packet"));
        }
        result.map(Some)
    }

    pub(super) fn packed_term_page(
        &self,
        after: Option<&[u8]>,
        surface: Option<u8>,
        budget: &mut WorkBudget,
    ) -> Result<TermPage, IndexError> {
        let mut result = self.read_packed_term_page(after, surface, budget)?;
        if !result.is_empty() {
            result.seal_counts(budget)?;
        }
        Ok(result)
    }

    fn read_packed_term_page(
        &self,
        after: Option<&[u8]>,
        surface: Option<u8>,
        budget: &mut WorkBudget,
    ) -> Result<TermPage, IndexError> {
        self.check_posting_usable()?;
        self.postings.seal(self, budget)?;
        self.postings.check_page_source(self)?;
        let mut result = TermPage::new();
        let _cursor_memory = budget.reserve_memory(1024, MemoryLifetime::Temporary)?;
        let mut cursor = [0_u8; 1024];
        let mut cursor_len = 0_usize;
        if let Some(after) = after {
            check_term_key(after)?;
            measured_copy(after.len(), budget)?;
            cursor[..after.len()].copy_from_slice(after);
            cursor_len = after.len();
        }
        if let Some(surface) = surface {
            if surface > 1 {
                return Err(IndexError::InvalidInput(
                    "invalid posting case surface".into(),
                ));
            }
            if cursor_len == 0 {
                cursor[0] = surface;
                cursor_len = 1;
            } else if cursor[0] != surface {
                return Err(corrupt("case cursor surface differs"));
            }
        }
        loop {
            budget.checkpoint()?;
            let lower = &cursor[..cursor_len];
            let before = result.len();
            let complete =
                self.with_term_packet_at(lower, false, budget, |mut packet, budget| {
                    while let Some((term, added, removed)) = packet.next()? {
                        if term <= lower {
                            continue;
                        }
                        if let Some(surface) = surface {
                            if term[0] < surface {
                                continue;
                            }
                            if term[0] > surface {
                                return Ok(true);
                            }
                        }
                        if result.is_empty() {
                            let source = self.postings.page_source(self, budget)?;
                            result = TermPage::counted(source, budget)?;
                        }
                        result.push_counted(term, PostingDelta { added, removed }, budget)?;
                        if result.len() == PAGE {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                })?;
            if complete != Some(false) {
                break;
            }
            if result.len() == before {
                return Err(corrupt("posting term packet did not advance"));
            }
            let last = result.last().ok_or(IndexError::ResourceLimited)?.as_slice();
            measured_copy(last.len(), budget)?;
            cursor[..last.len()].copy_from_slice(last);
            cursor_len = last.len();
        }
        Ok(result)
    }

    pub(super) fn packed_cardinalities(
        &self,
        terms: &TermPage,
        budget: &mut WorkBudget,
    ) -> Result<PostingCardinalities, IndexError> {
        self.postings.seal(self, budget)?;
        self.postings.check_page_source(self)?;
        if terms.is_empty()
            || terms.len() > PAGE
            || terms
                .windows(2)
                .any(|pair| matches!(pair,[left,right] if left.as_slice()>=right.as_slice()))
        {
            return Err(corrupt("cardinality term page size or order"));
        }
        let memory = budget.reserve_memory(
            width(std::mem::size_of::<[PostingDelta; PAGE]>())?,
            MemoryLifetime::Temporary,
        )?;
        for _ in terms.iter() {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
        }
        if let Some(counts) = terms.counts.as_ref() {
            terms.check_counts(self, budget)?;
            return Ok(PostingCardinalities {
                deltas: counts.deltas,
                len: terms.len(),
                page_digest: term_page_digest(terms)?,
                changes: self.db.change_count()?,
                _memory: memory,
            });
        }
        let mut deltas = [PostingDelta::default(); PAGE];
        let cut = terms.case_cut()?.unwrap_or(terms.len());
        for (start, end) in [(0, cut), (cut, terms.len())] {
            if start == end {
                continue;
            }
            let first = terms
                .get(start)
                .ok_or(IndexError::ResourceLimited)?
                .as_slice();
            let mut slot = start;
            while slot < end {
                let inclusive = slot == start;
                let lower = if inclusive {
                    first
                } else {
                    terms
                        .get(slot - 1)
                        .ok_or(IndexError::ResourceLimited)?
                        .as_slice()
                };
                let before = slot;
                self.with_term_packet_at(lower, inclusive, budget, |mut packet, budget| {
                    while slot < end {
                        budget.checkpoint()?;
                        let Some((term, added, removed)) = packet.next()? else {
                            break;
                        };
                        if slot == start && term < first {
                            continue;
                        }
                        let expected = terms.get(slot).ok_or(IndexError::ResourceLimited)?;
                        if term != expected.as_slice() {
                            return Err(corrupt("cardinality key differs"));
                        }
                        deltas[slot] = PostingDelta { added, removed };
                        slot += 1;
                    }
                    Ok(())
                })?
                .ok_or_else(|| corrupt("cardinality metadata missing"))?;
                if slot == before {
                    return Err(corrupt("cardinality packet did not advance"));
                }
            }
        }
        Ok(PostingCardinalities {
            deltas,
            len: terms.len(),
            page_digest: term_page_digest(terms)?,
            changes: self.db.change_count()?,
            _memory: memory,
        })
    }
}

struct RememberedPacket {
    source: Rc<PostingPageSource>,
    phase: i64,
    page: i64,
    packet: Packet,
}

/// Two slots for the last checked packets of the two cold posting surfaces.
/// Park only Original-funded payloads; a live cursor owns its checked packet.
#[derive(Default)]
pub(super) struct PostingPacketCache {
    slots: RefCell<[Option<RememberedPacket>; 2]>,
}

impl PostingPacketCache {
    fn clear(&self) {
        let parked = self.slots.replace([None, None]);
        drop(parked);
    }

    fn take(
        &self,
        source: &Rc<PostingPageSource>,
        phase: i64,
        page: i64,
        start: Option<&[u8]>,
        budget: &WorkBudget,
    ) -> Result<Option<Packet>, IndexError> {
        let mut slots = self.slots.borrow_mut();
        for slot in slots.iter_mut() {
            if let Some(saved) = slot.as_mut()
                && Rc::ptr_eq(&saved.source, source)
                && saved.phase == phase
                && saved.page == page
            {
                saved.packet.reposition(start.unwrap_or_default(), budget)?;
                return Ok(slot.take().map(|saved| saved.packet));
            }
        }
        Ok(None)
    }
}

struct PacketReuse<'a> {
    cache: &'a PostingPacketCache,
    source: Rc<PostingPageSource>,
    surface: usize,
}

fn posting_surface(key: &[u8]) -> Result<usize, IndexError> {
    match key.first() {
        Some(0) => Ok(0),
        Some(1) => Ok(1),
        _ => Err(corrupt("invalid packet reuse surface")),
    }
}

pub(super) struct PackedRangeCursor<'a> {
    spool: &'a TextSpool,
    phase: Option<i64>,
    db_changes: u64,
    start: Vec<u8>,
    next_page: Option<i64>,
    packet: Option<Packet>,
    reuse: Option<PacketReuse<'a>>,
    initialized: bool,
    _key_memory: Option<MemoryLease>,
    _prior_memory: Option<MemoryLease>,
    prior_term: [u8; 1024],
    prior_len: usize,
    prior_owner: [u8; 32],
}

impl<'a> PackedRangeCursor<'a> {
    pub(super) fn new(
        spool: &'a TextSpool,
        start: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        Self::new_with_packets(spool, start, None, budget)
    }

    pub(super) fn new_with_packets(
        spool: &'a TextSpool,
        start: &[u8],
        cache: Option<&'a PostingPacketCache>,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        spool.check_posting_usable()?;
        spool.postings.seal(spool, budget)?;
        let phase = spool.postings.final_phase()?;
        let reuse = if let Some(cache) = cache
            && phase.is_some()
        {
            Some(PacketReuse {
                cache,
                source: spool.postings.page_source(spool, budget)?,
                surface: posting_surface(start)?,
            })
        } else {
            None
        };
        let db_changes = spool.db.change_count()?;
        if start.len() > 1024 {
            return Err(IndexError::ResourceLimited);
        }
        let key_memory = budget.reserve_memory(width(start.len())?, MemoryLifetime::Temporary)?;
        let prior_memory = budget.reserve_memory(1056, MemoryLifetime::Temporary)?;
        measured_copy(start.len(), budget)?;
        Ok(Self {
            spool,
            phase,
            db_changes,
            start: start.to_vec(),
            next_page: None,
            packet: None,
            reuse,
            initialized: false,
            _key_memory: key_memory,
            _prior_memory: prior_memory,
            prior_term: [0; 1024],
            prior_len: 0,
            prior_owner: [0; 32],
        })
    }

    pub(super) fn remember_packet(&mut self) {
        if let Some(reuse) = self.reuse.as_ref()
            && let Some(phase) = self.phase
            && let Some(page) = self.next_page.and_then(|next| next.checked_sub(1))
            && self.packet.as_ref().is_some_and(|packet| packet.reusable)
            && let Some(packet) = self.packet.take()
        {
            let displaced =
                reuse.cache.slots.borrow_mut()[reuse.surface].replace(RememberedPacket {
                    source: Rc::clone(&reuse.source),
                    phase,
                    page,
                    packet,
                });
            drop(displaced);
        }
    }

    fn check_epoch(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        self.spool.check_posting_usable()?;
        budget.checkpoint()?;
        self.spool.check_posting_epoch(self.db_changes)
    }

    /// Seek a disjoint range with the same reader. Only an exact SQL page
    /// match in this epoch may reuse the resident, fully checked payload.
    pub(super) fn reposition(
        &mut self,
        start: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.check_epoch(budget)?;
        check_term_key(start)?;
        let new_surface = self
            .reuse
            .as_ref()
            .map(|_| posting_surface(start))
            .transpose()?;
        let page = self
            .phase
            .map(|phase| self.spool.seek_final_posting_page(phase, start, budget))
            .transpose()?
            .flatten();
        let reuse = page.is_some()
            && self.next_page.and_then(|next| next.checked_sub(1)) == page
            && self.packet.is_some();
        if reuse {
            self.packet
                .as_mut()
                .ok_or_else(|| corrupt("resident posting packet absent"))?
                .reposition(start, budget)?;
        } else {
            let key_memory =
                budget.reserve_memory(width(start.len())?, MemoryLifetime::Temporary)?;
            measured_copy(start.len(), budget)?;
            self.start = start.to_vec();
            self._key_memory = key_memory;
            self.remember_packet();
            self.packet = None;
            self.next_page = page;
        }
        if let Some((reuse, surface)) = self.reuse.as_mut().zip(new_surface) {
            reuse.surface = surface;
        }
        self.prior_len = 0;
        self.initialized = true;
        Ok(())
    }

    pub(super) fn next(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<PostingRow>, IndexError> {
        self.next_row(budget, |row, budget| row.into_owned(budget))
    }

    pub(super) fn next_row<T>(
        &mut self,
        budget: &mut WorkBudget,
        mut project: impl FnMut(RowView<'_>, &mut WorkBudget) -> Result<T, IndexError>,
    ) -> Result<Option<T>, IndexError> {
        self.check_epoch(budget)?;
        if !self.initialized {
            self.next_page = match self.phase {
                Some(phase) => self
                    .spool
                    .seek_final_posting_page(phase, &self.start, budget)?,
                None => None,
            };
            self.initialized = true;
        }
        loop {
            if let Some(packet) = self.packet.as_mut() {
                // Packet::parse checks every row's order. The next packet
                // only has to prove its first key follows the previous tail.
                let first_in_packet = packet.offset == EDIT_HEADER;
                if let Some(row) = packet.next_row(budget, |decoded, budget| {
                    let row = decoded.view;
                    if first_in_packet
                        && self.prior_len != 0
                        && self.prior_term[..self.prior_len]
                            .cmp(row.term)
                            .then(self.prior_owner.cmp(&row.owner))
                            != Ordering::Less
                    {
                        return Err(corrupt("final packet stream is unordered"));
                    }
                    project(row, budget)
                })? {
                    return Ok(Some(row));
                }
                // Keep the authenticated page on a refusal so a retry can
                // recover the same boundary before opening the next page.
                let (term, owner) = packet.last_key()?;
                measured_copy(
                    term.len()
                        .checked_add(owner.len())
                        .ok_or(IndexError::ResourceLimited)?,
                    budget,
                )?;
                self.prior_term[..term.len()].copy_from_slice(term);
                self.prior_len = term.len();
                self.prior_owner = owner;
                self.remember_packet();
                self.packet = None;
            }
            let Some(page) = self.next_page else {
                return Ok(None);
            };
            let phase = self.phase.ok_or(IndexError::ResourceLimited)?;
            // Before the first row, position within the authenticated packet
            // without materializing earlier rows. Refusal does not advance it.
            let lower_bound = (self.prior_len == 0).then_some(self.start.as_slice());
            let remembered = if let Some(reuse) = self.reuse.as_ref() {
                let packet = reuse
                    .cache
                    .take(&reuse.source, phase, page, lower_bound, budget)?;
                if packet.is_none() {
                    // A miss replaces this surface's slot. The active
                    // packet and the other surface never need a third grant.
                    let displaced = reuse.cache.slots.borrow_mut()[reuse.surface].take();
                    drop(displaced);
                }
                packet
            } else {
                None
            };
            self.packet = if remembered.is_some() {
                remembered
            } else {
                self.spool.read_posting_packet(
                    phase,
                    0,
                    page,
                    lower_bound,
                    self.reuse.as_ref().map(|reuse| reuse.cache),
                    budget,
                )?
            };
            self.next_page = if self.packet.is_some() {
                Some(page.checked_add(1).ok_or(IndexError::ResourceLimited)?)
            } else {
                None
            };
        }
    }
}

impl TextSpool {
    fn seek_final_posting_page(
        &self,
        phase: i64,
        start: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<i64>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        db.prepare_cached(
            "SELECT page FROM posting_packet INDEXED BY posting_packet_seek
             WHERE phase=?1 AND run=0 AND last_term>=?2
             ORDER BY last_term,last_owner LIMIT 1",
        )
        .map_err(|error| sql(&error))?
        .query_row(params![phase, start], |row| row.get(0))
        .optional()
        .map_err(|error| sql(&error))
    }
}

impl TextSpool {
    pub(super) fn check_posting_epoch(&self, changes: u64) -> Result<(), IndexError> {
        if self.db.change_count()? != changes {
            return Err(IndexError::BackendViolation(
                "posting scratch changed during packet read".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn packed_root_cursors<'a>(
        &'a self,
        terms: &TermPage,
        eligible: &[bool],
        proof: Option<&DensePostingPrepassProof>,
        budget: &mut WorkBudget,
    ) -> Result<PostingRootCursors<'a>, IndexError> {
        self.check_posting_usable()?;
        if terms.is_empty() || terms.len() > PAGE || eligible.len() != terms.len() {
            return Err(IndexError::InvalidInput(
                "sparse posting roots differ from term page".into(),
            ));
        }
        if eligible.iter().filter(|flag| **flag).count() < 2 {
            return PostingRootCursors::fallback(self, budget);
        }
        // The authenticated sparse prepass owns a packet cursor. Constructing
        // the shared fallback actor before that cursor is released retains two
        // independent 1024-byte key witnesses at once under the same Original.
        let mut offsets = [0_usize; PAGE];
        let mut lengths = [0_u8; PAGE];
        let mut keys: [Option<PreparedKey>; PAGE] = std::array::from_fn(|_| None);
        let mut selected_count = 0_usize;
        for (slot, selected) in eligible.iter().copied().enumerate() {
            if selected {
                offsets[slot] = selected_count
                    .checked_mul(8)
                    .ok_or(IndexError::ResourceLimited)?;
                selected_count = selected_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        let capacity = selected_count
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)?;
        let dense = if let Some(proof) = proof {
            proof.lower_bound > width(capacity)?
                && proof.page_digest == term_page_digest(terms)?
                && self.db.change_count()? == proof.changes
        } else {
            false
        };
        if dense {
            return PostingRootCursors::fallback(self, budget);
        }
        let bytes = capacity
            .checked_mul(std::mem::size_of::<Option<OrdinalMapEdit>>())
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: width(bytes)?,
            ..WorkUsage::default()
        })?;
        let memory = budget.reserve_memory(width(bytes)?, MemoryLifetime::Temporary)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(capacity)
            .map_err(|_error| IndexError::ResourceLimited)?;
        rows.resize_with(capacity, || None);
        let mut seen = [false; PAGE];
        let mut scanned = 0usize;
        let cut = terms.case_cut()?.unwrap_or(terms.len());
        let mut dense_fallback = false;
        let mut range: Option<PackedRangeCursor<'_>> = None;
        'scan: for (start, end) in [(0, cut), (cut, terms.len())] {
            if start == end {
                continue;
            }
            let first = terms.get(start).ok_or(IndexError::ResourceLimited)?;
            let last = terms.get(end - 1).ok_or(IndexError::ResourceLimited)?;
            if let Some(cursor) = range.as_mut() {
                cursor.reposition(first.as_slice(), budget)?;
            } else {
                range = Some(PackedRangeCursor::new(self, first.as_slice(), budget)?);
            }
            let cursor = range.as_mut().ok_or(IndexError::ResourceLimited)?;
            while let Some(row) = cursor.next(budget)? {
                budget.checkpoint()?;
                if row.term.as_slice() > last.as_slice() {
                    break;
                }
                scanned = scanned.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                // The original sparse prepass stopped after 513 rows. A dense
                // page falls back to the same authenticated per-root cursor.
                if scanned > capacity {
                    dense_fallback = true;
                    break 'scan;
                }
                let slot = terms
                    .binary_search_by(|key| key.as_slice().cmp(row.term.as_slice()))
                    .map_err(|_error| corrupt("sparse posting term outside selected page"))?;
                seen[slot] = true;
                if eligible[slot] && lengths[slot] < 8 {
                    let index = offsets[slot]
                        .checked_add(usize::from(lengths[slot]))
                        .ok_or(IndexError::ResourceLimited)?;
                    rows[index] = Some(row.ordinal(0, budget)?);
                    lengths[slot] += 1;
                } else if eligible[slot] {
                    lengths[slot] = 9;
                }
            }
        }
        let changes = range
            .as_ref()
            .ok_or(IndexError::ResourceLimited)?
            .db_changes;
        self.check_posting_epoch(changes)?;
        drop(range);
        if dense_fallback {
            drop(rows);
            drop(memory);
            return PostingRootCursors::fallback(self, budget);
        }
        for slot in 0..terms.len() {
            if !seen[slot] {
                return Err(corrupt("sparse posting selected term missing"));
            }
            if eligible[slot] && (1..=8).contains(&lengths[slot]) {
                keys[slot] = Some(PreparedKey::copy(
                    terms
                        .get(slot)
                        .ok_or(IndexError::ResourceLimited)?
                        .as_slice(),
                    budget,
                )?);
            } else {
                lengths[slot] = 0;
            }
        }
        let mut result = PostingRootCursors::fallback(self, budget)?;
        result.prepared_changes = Some(changes);
        result.shared.borrow_mut().db_changes = Some(changes);
        result.keys = keys;
        result.lengths = lengths;
        result.offsets = offsets;
        result.rows = rows;
        result._row_memory = memory;
        Ok(result)
    }
}

#[cfg(test)]
mod dictionary_capacity_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn reference(number: u32) -> ObjectRef {
        let mut bytes = [0_u8; 32];
        bytes[..4].copy_from_slice(&number.to_be_bytes());
        ObjectRef(Digest(bytes))
    }

    #[test]
    fn grouped_packet_rejects_references_outside_its_current_literal() -> Result<(), IndexError> {
        let mut budget = WorkBudget::new(
            quanta_index_core::index::WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 4_096,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(10),
        );
        let mut bytes = vec![0; EDIT_HEADER];
        for term in [b"\0a", b"\0b"] {
            let _key = PacketWriter::put_row_key(&mut bytes, term, 0)?;
            bytes.extend_from_slice(&[0, 1, 0, 0]);
        }
        let repeat_offset = bytes.len();
        bytes.extend_from_slice(&(REPEATED_TERM | 10).to_be_bytes());
        bytes.extend_from_slice(&[0, 1, 0, 1, 0, 0]);
        finalize_compact_packet(
            &mut bytes,
            3,
            &[[1; 32], [2; 32]],
            &[reference(0)],
            &mut budget,
        )?;
        bytes[..8].copy_from_slice(GROUPED_EDIT_MAGIC);
        assert!(matches!(
            Packet::parse(bytes.clone(), None, None, None, false),
            Err(IndexError::CorruptObject(_))
        ));
        let packet = Packet::parse(bytes.clone(), None, None, None, true)?;
        assert_eq!(packet.last_key()?, (b"\0b".as_slice(), [2; 32]));

        for bad_distance in [0, 9, 20, 21] {
            bytes[repeat_offset..repeat_offset + 2]
                .copy_from_slice(&(REPEATED_TERM | bad_distance).to_be_bytes());
            let digest = Sha256::digest(&bytes[42..]);
            bytes[10..42].copy_from_slice(&digest);
            assert!(matches!(
                Packet::parse(bytes.clone(), None, None, None, true),
                Err(IndexError::CorruptObject(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn sorted_posting_writer_charges_one_copy_per_row_with_exact_packet_fences()
    -> Result<(), IndexError> {
        let term = b"\0posting";
        let row_bytes = 2 + term.len() + 2 + 1 + 3;
        let dictionary_copy_bytes = 2 * (2 * 32 + 32);
        let bytes = width(row_bytes * 2 + dictionary_copy_bytes)?;
        let mut budget = WorkBudget::new(
            quanta_index_core::index::WorkLimits {
                reads: 0,
                writes: 0,
                bytes,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(10),
        );
        let mut writer = PacketWriter::new(&budget, MemoryLifetime::Temporary)?;
        for owner in [[1; 32], [2; 32]] {
            writer.push(
                RowView {
                    term,
                    owner,
                    expected: None,
                    next: Some(MapValue::Object(reference(1))),
                },
                &mut budget,
            )?;
        }
        assert_eq!(budget.usage().bytes, bytes);
        assert_eq!(
            writer.fence(writer.first.ok_or(IndexError::Conflict)?),
            (term.as_slice(), [1; 32])
        );
        assert_eq!(
            writer.fence(writer.last.ok_or(IndexError::Conflict)?),
            (term.as_slice(), [2; 32])
        );
        Ok(())
    }

    #[test]
    fn two_distinct_references_per_row_never_grow_after_original_admission()
    -> Result<(), IndexError> {
        let mut budget = WorkBudget::new(
            quanta_index_core::index::WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 536_870_912,
                mutations: 1_000_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let mut writer = PacketWriter::new(&budget, MemoryLifetime::Temporary)?;
        let initial_references = writer.references.capacity();
        let initial_ids = writer.reference_ids.capacity();
        let term = b"\0distinct";
        for index in 0_u32..900 {
            let mut owner = [0_u8; 32];
            owner[..4].copy_from_slice(&index.to_be_bytes());
            let row = RowView {
                term,
                owner,
                expected: Some(MapValue::Object(reference(index * 2))),
                next: Some(MapValue::Object(reference(index * 2 + 1))),
            };
            assert!(writer.can_push(row)?);
            writer.push(row, &mut budget)?;
        }
        assert_eq!(writer.references.len(), 1_800);
        assert_eq!(writer.references.capacity(), initial_references);
        assert_eq!(writer.reference_ids.capacity(), initial_ids);
        finalize_compact_packet(
            &mut writer.bytes,
            writer.count,
            &writer.owners,
            &writer.references,
            &mut budget,
        )?;
        let raw = std::mem::take(&mut writer.bytes);
        let mut parsed = Packet::parse(raw, None, None, None, false)?;
        for index in 0_u32..900 {
            let row = parsed
                .next_row(&mut budget, |row, budget| row.view.into_owned(budget))?
                .ok_or(IndexError::ResourceLimited)?;
            let mut expected_owner = [0_u8; 32];
            expected_owner[..4].copy_from_slice(&index.to_be_bytes());
            assert_eq!(row.term.as_slice(), term.as_slice());
            assert_eq!(row.owner, expected_owner);
            assert_eq!(row.expected, Some(MapValue::Object(reference(index * 2))));
            assert_eq!(row.next, Some(MapValue::Object(reference(index * 2 + 1))));
        }
        assert!(parsed.next_row(&mut budget, |_, _| Ok(()))?.is_none());
        Ok(())
    }
}
