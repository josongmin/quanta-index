//! One real SQL execution for a bounded batch of the existing edit IR.

use std::cell::Cell;

use quanta_index_core::index::{
    IndexError, MapEdit, MapValue, MemoryLifetime, OrdinalMapEdit, ScratchAllocation, WorkBudget,
    WorkUsage,
};
use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1,
    DiagnosticLaneV1, DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1,
    DiagnosticTraceErrorV1,
};
use rusqlite::{Connection, PagerCacheStatus, limits::Limit, types::ValueRef};

use crate::{
    MAX_WIDE_WRITE_BATCH_ROWS, MAX_WRITE_BATCH_ROWS, map_value, reserve_for_write_batch_with_limit,
    storage_error,
};

fn sql(error: &rusqlite::Error) -> IndexError {
    storage_error(error, "scratch edit batch")
}

thread_local! {
    // The sampled prefix resets for a new trace capture on this build thread.
    static WIDE_POSTING_BATCH: Cell<(Option<DiagnosticCaptureIdV1>, u64)> =
        const { Cell::new((None, 0)) };
}

fn sampled_wide_posting_context(
    budget: &WorkBudget,
    max_rows: usize,
    prefix: &str,
) -> Option<DiagnosticContextV1> {
    if max_rows != MAX_WIDE_WRITE_BATCH_ROWS || prefix != "INSERT INTO posting(term,k,p,n) VALUES "
    {
        return None;
    }
    let context = budget.request().diagnostic_context()?;
    let capture = context.capture_id();
    let selected = WIDE_POSTING_BATCH.with(|slot| {
        let (prior_capture, prior_count) = slot.get();
        let count = if prior_capture == Some(capture) {
            prior_count.saturating_add(1)
        } else {
            1
        };
        slot.set((Some(capture), count));
        sampled_batch_ordinal(count)
    });
    selected.then(|| context.clone())
}

fn sampled_batch_ordinal(ordinal: u64) -> bool {
    ordinal <= 8 || ordinal.is_power_of_two()
}

/// Full posting flush phases. Connection-scope `SqliteWork` lane 22 is a sibling;
/// its duration may overlap these phases, so sibling durations must not be added.
struct PostingPhases {
    context: Option<DiagnosticContextV1>,
    active: Option<DiagnosticSpanGuardV1>,
}

impl PostingPhases {
    fn new(budget: &WorkBudget) -> Self {
        let context = budget.request().diagnostic_context().cloned();
        let active = context
            .as_ref()
            .and_then(|context| enter_posting_span(context, DiagnosticLaneV1(36)));
        Self { context, active }
    }

    fn advance(&mut self, lane: u16) {
        if let Some(active) = self.active.take() {
            let _finished = active.finish(DiagnosticTerminalV1::Ok);
        }
        self.active = self
            .context
            .as_ref()
            .and_then(|context| enter_posting_span(context, DiagnosticLaneV1(lane)));
    }

    fn sampled_child(&self, sampled: &DiagnosticContextV1) -> DiagnosticContextV1 {
        self.active
            .as_ref()
            .map_or_else(|| sampled.clone(), |active| sampled.child_context(active))
    }

    fn record_execute_rows(&self, changed: usize) {
        let (Some(context), Some(active)) = (&self.context, &self.active) else {
            return;
        };
        let Ok(rows) = u64::try_from(changed) else {
            context.health(DiagnosticHealthCodeV1::CounterOverflow);
            return;
        };
        context.counter(active, DiagnosticCounterV1::SqliteRows, rows);
        context.counter(active, DiagnosticCounterV1::SqliteBatches, 1);
    }

    fn finish(mut self, result: &Result<(), IndexError>) {
        if let Some(active) = self.active.take() {
            let _finished = active.finish(DiagnosticTerminalV1::from_result(result));
        }
    }
}

fn posting_span_health(error: DiagnosticTraceErrorV1) -> DiagnosticHealthCodeV1 {
    match error {
        DiagnosticTraceErrorV1::SpanIdentityExhausted => DiagnosticHealthCodeV1::CounterOverflow,
        DiagnosticTraceErrorV1::ClockRegressed => DiagnosticHealthCodeV1::ClockRegressed,
        DiagnosticTraceErrorV1::ForeignCaptureSpan => DiagnosticHealthCodeV1::ForeignCaptureSpan,
        DiagnosticTraceErrorV1::MissingParentSpan => DiagnosticHealthCodeV1::MissingParentSpan,
    }
}

fn enter_posting_span(
    context: &DiagnosticContextV1,
    lane: DiagnosticLaneV1,
) -> Option<DiagnosticSpanGuardV1> {
    match context.enter(DiagnosticStageV1::NativeTextPostingEdit, lane) {
        Ok(span) => Some(span),
        Err(error) => {
            context.health(posting_span_health(error));
            None
        }
    }
}

/// Record a sampled successful posting execution and its optional pager packet.
///
/// Missing or regressed pager snapshots are diagnostic-only.
/// `SQLite` does not define the write counter after a failed
/// SQL write, so that path emits no counters or additional health events.
fn record_posting_execute(
    context: &DiagnosticContextV1,
    span: &DiagnosticSpanGuardV1,
    before: Option<rusqlite::Result<PagerCacheStatus>>,
    after: Option<rusqlite::Result<PagerCacheStatus>>,
    changed: Option<usize>,
) {
    let Some(changed) = changed else {
        return;
    };
    let Ok(rows) = u64::try_from(changed) else {
        context.health(DiagnosticHealthCodeV1::CounterOverflow);
        return;
    };
    context.counter(span, DiagnosticCounterV1::SqliteRows, rows);
    context.counter(span, DiagnosticCounterV1::SqliteBatches, 1);
    let (Some(Ok(before)), Some(Ok(after))) = (before, after) else {
        context.health(DiagnosticHealthCodeV1::CounterUnavailable);
        return;
    };
    let (Some(hit), Some(miss), Some(write), Some(spill)) = (
        after.hit.checked_sub(before.hit),
        after.miss.checked_sub(before.miss),
        after.write.checked_sub(before.write),
        after.spill.checked_sub(before.spill),
    ) else {
        context.health(DiagnosticHealthCodeV1::CounterRegressed);
        return;
    };
    for (key, value) in [
        (DiagnosticCounterV1::SqliteCacheHits, hit),
        (DiagnosticCounterV1::SqliteCacheMisses, miss),
        (DiagnosticCounterV1::SqliteCacheWrites, write),
        (DiagnosticCounterV1::SqliteCacheSpills, spill),
    ] {
        context.counter(span, key, value);
    }
}

/// Fields borrowed only while binding one scratch row. A caller with an
/// inline key need not allocate an owned `MapEdit` for SQL.
#[derive(Clone, Copy)]
pub struct MapEditFields<'a> {
    pub key: &'a [u8],
    pub expected: Option<&'a MapValue>,
    pub next: Option<&'a MapValue>,
}

impl<'a> From<&'a MapEdit> for MapEditFields<'a> {
    fn from(edit: &'a MapEdit) -> Self {
        Self {
            key: &edit.key,
            expected: edit.expected.as_ref(),
            next: edit.next.as_ref(),
        }
    }
}

#[derive(Clone, Copy)]
enum EditValueRef<'a> {
    Decoded(&'a MapValue),
    CanonicalWire(&'a [u8]),
}

impl EditValueRef<'_> {
    fn encoded_len(self) -> Result<usize, IndexError> {
        match self {
            Self::Decoded(value) => value.encoded_len(),
            Self::CanonicalWire(wire) => Ok(wire.len()),
        }
    }
}

#[derive(Clone, Copy)]
struct EditFields<'a> {
    key: &'a [u8],
    expected: Option<EditValueRef<'a>>,
    next: Option<EditValueRef<'a>>,
}

impl<'a> From<MapEditFields<'a>> for EditFields<'a> {
    fn from(fields: MapEditFields<'a>) -> Self {
        Self {
            key: fields.key,
            expected: fields.expected.map(EditValueRef::Decoded),
            next: fields.next.map(EditValueRef::Decoded),
        }
    }
}

fn ordinal_fields(edit: &OrdinalMapEdit) -> EditFields<'_> {
    EditFields {
        key: edit.key(),
        expected: edit.expected_wire().map(EditValueRef::CanonicalWire),
        next: edit.next_wire().map(EditValueRef::CanonicalWire),
    }
}

/// Write a bounded batch of the canonical owned edits through the same binder.
pub fn write_map_edit_batch<'a>(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    prefix: &'static str,
    query_bytes: u64,
    reservations: &[u64],
    row: impl Fn(usize) -> Result<(ValueRef<'a>, &'a MapEdit, u64), IndexError>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    write_map_edit_fields_batch(
        db,
        allocation,
        prefix,
        query_bytes,
        reservations,
        |slot| row(slot).map(|(partition, edit, index)| (partition, edit.into(), index)),
        budget,
    )
}

fn map_edit_sql_bytes(prefix: &str, count: usize) -> Result<usize, IndexError> {
    count
        .checked_mul("(?,?,?,?)".len())
        .and_then(|bytes| bytes.checked_add(count.saturating_sub(1)))
        .and_then(|bytes| bytes.checked_add(prefix.len()))
        .ok_or(IndexError::ResourceLimited)
}

/// Tracked SQL String backing for one ordinary edit packet using the same
/// length calculation as `write_map_edit_batch`. Caller-owned rows and `SQLite`
/// internal memory are outside this bound.
pub fn map_edit_sql_memory_bound(
    prefix: &'static str,
    maximum_rows: usize,
) -> Result<quanta_index_core::index::MemoryCreditLimits, IndexError> {
    if maximum_rows == 0 || maximum_rows > MAX_WRITE_BATCH_ROWS {
        return Err(IndexError::ResourceLimited);
    }
    Ok(quanta_index_core::index::MemoryCreditLimits {
        temporary_data_bytes: u64::try_from(map_edit_sql_bytes(prefix, maximum_rows)?)
            .map_err(|_error| IndexError::ResourceLimited)?,
        retained_data_bytes: 0,
        max_live_temporary_leases: 1,
        max_live_retained_leases: 0,
    })
}

/// Write one bounded edit batch with bound parameters.
///
/// `prefix` is the adapter's constant INSERT table/column clause, ending in VALUES.
/// Rows must stay immutable for this call.
/// Original query fees, every logical write, actual fields and physical row
/// reservations are separate. Each row also declares its secondary-index work.
/// Any error poisons this disposable attempt.
pub fn write_map_edit_fields_batch<'a>(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    prefix: &'static str,
    query_bytes: u64,
    reservations: &[u64],
    row: impl Fn(usize) -> Result<(ValueRef<'a>, MapEditFields<'a>, u64), IndexError>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    write_map_edit_fields_batch_inner(
        db,
        allocation,
        EditBatch {
            prefix,
            query_bytes,
            reservations,
            max_rows: MAX_WRITE_BATCH_ROWS,
        },
        |slot| row(slot).map(|(partition, fields, index)| (partition, fields.into(), index)),
        budget,
    )
}

/// Bind immutable cursor positions through the ordinary edit executor.
/// The retained position already owns canonical value wire bytes.
pub fn write_ordinal_map_edit_batch<'a>(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    prefix: &'static str,
    query_bytes: u64,
    reservations: &[u64],
    row: impl Fn(usize) -> Result<(ValueRef<'a>, &'a OrdinalMapEdit, u64), IndexError>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    write_map_edit_fields_batch_inner(
        db,
        allocation,
        EditBatch {
            prefix,
            query_bytes,
            reservations,
            max_rows: MAX_WRITE_BATCH_ROWS,
        },
        |slot| row(slot).map(|(partition, edit, index)| (partition, ordinal_fields(edit), index)),
        budget,
    )
}

/// Maximum opt-in width supported by this actual `SQLite` connection. A lower
/// run-time variable limit keeps posting callers on their existing 64-row path.
pub fn max_supported_wide_edit_batch_rows(db: &Connection) -> Result<usize, IndexError> {
    const WIDE_PARAMETERS: usize = MAX_WIDE_WRITE_BATCH_ROWS * 4;
    let limit = db
        .limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER)
        .map_err(|error| sql(&error))?;
    let wide_parameters =
        i32::try_from(WIDE_PARAMETERS).map_err(|_error| IndexError::ResourceLimited)?;
    Ok(if limit >= wide_parameters {
        MAX_WIDE_WRITE_BATCH_ROWS
    } else {
        MAX_WRITE_BATCH_ROWS
    })
}

/// Posting-only opt-in. It shares the ordinary binder and admission path; a
/// connection whose variable limit is too low refuses before reserving or SQL.
pub fn write_map_edit_fields_batch_wide<'a>(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    prefix: &'static str,
    query_bytes: u64,
    reservations: &[u64],
    row: impl Fn(usize) -> Result<(ValueRef<'a>, MapEditFields<'a>, u64), IndexError>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    write_map_edit_fields_batch_inner(
        db,
        allocation,
        EditBatch {
            prefix,
            query_bytes,
            reservations,
            max_rows: MAX_WIDE_WRITE_BATCH_ROWS,
        },
        |slot| row(slot).map(|(partition, fields, index)| (partition, fields.into(), index)),
        budget,
    )
}

#[derive(Clone, Copy)]
struct EditBatch<'a> {
    prefix: &'static str,
    query_bytes: u64,
    reservations: &'a [u64],
    max_rows: usize,
}

const POSTING_PREFIX: &str = "INSERT INTO posting(term,k,p,n) VALUES ";

#[derive(Clone, Copy)]
struct PostingSharing {
    // owner, expected and next are compared independently. Terms never share.
    fields: [bool; 3],
}

impl PostingSharing {
    fn count(self) -> usize {
        self.fields.into_iter().filter(|shared| *shared).count()
    }

    fn parameters(self, slot: usize, common: usize) -> Result<[usize; 4], IndexError> {
        let varying = 4_usize
            .checked_sub(common)
            .ok_or(IndexError::ResourceLimited)?;
        let mut next = slot
            .checked_mul(varying)
            .and_then(|offset| offset.checked_add(common))
            .ok_or(IndexError::ResourceLimited)?;
        let mut shared = 0_usize;
        let mut result = [0; 4];
        let [key, expected, replacement] = self.fields;
        for (parameter, common) in result.iter_mut().zip([false, key, expected, replacement]) {
            if common {
                shared = shared.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                *parameter = shared;
            } else {
                next = next.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                *parameter = next;
            }
        }
        Ok(result)
    }

    fn query_bytes(count: usize, common: usize) -> Result<usize, IndexError> {
        if count > MAX_WIDE_WRITE_BATCH_ROWS || common > 3 {
            return Err(IndexError::ResourceLimited);
        }
        let separators = count.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
        let mut bytes = POSTING_PREFIX
            .len()
            .checked_add(separators)
            .and_then(|bytes| {
                count
                    .checked_mul("(?,?,?,?)".len())
                    .and_then(|rows| bytes.checked_add(rows))
            })
            .ok_or(IndexError::ResourceLimited)?;
        if common == 0 {
            return Ok(bytes);
        }
        bytes = bytes
            .checked_add(
                count
                    .checked_mul(common)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        let varying = 4_usize
            .checked_sub(common)
            .ok_or(IndexError::ResourceLimited)?;
        let last = count
            .checked_mul(varying)
            .and_then(|total| total.checked_add(common))
            .ok_or(IndexError::ResourceLimited)?;
        let maximum = MAX_WIDE_WRITE_BATCH_ROWS
            .checked_mul(4)
            .ok_or(IndexError::ResourceLimited)?;
        if last > maximum {
            return Err(IndexError::ResourceLimited);
        }
        let mut first = common.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        let mut upper = 9_usize;
        let mut digits = 1_usize;
        while first <= last {
            let end = last.min(upper);
            let length = end
                .checked_sub(first)
                .and_then(|length| length.checked_add(1))
                .ok_or(IndexError::ResourceLimited)?;
            bytes = bytes
                .checked_add(
                    length
                        .checked_mul(digits)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
            if end == last {
                break;
            }
            first = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            upper = upper
                .checked_mul(10)
                .and_then(|bound| bound.checked_add(9))
                .ok_or(IndexError::ResourceLimited)?;
            digits = digits.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        Ok(bytes)
    }
}

/// One packet's admitted SQL shape. Numbered parameters are calculated once
/// and shared by rendering, ordinary bindings and value-ID bindings.
struct PostingSqlShape {
    fields: [bool; 3],
    common: usize,
    count: usize,
    query_bytes: usize,
    parameters: [[u16; 4]; MAX_WIDE_WRITE_BATCH_ROWS],
}

impl PostingSqlShape {
    fn new(
        sharing: PostingSharing,
        common: usize,
        count: usize,
        query_bytes: usize,
    ) -> Result<Self, IndexError> {
        if count == 0 || count > MAX_WIDE_WRITE_BATCH_ROWS {
            return Err(IndexError::ResourceLimited);
        }
        let maximum_parameter = MAX_WIDE_WRITE_BATCH_ROWS
            .checked_mul(4)
            .ok_or(IndexError::ResourceLimited)?;
        let mut parameters = [[0; 4]; MAX_WIDE_WRITE_BATCH_ROWS];
        for slot in 0..count {
            let numbers = sharing.parameters(slot, common)?;
            let row = parameters
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)?;
            for (target, number) in row.iter_mut().zip(numbers) {
                if number == 0 || number > maximum_parameter {
                    return Err(IndexError::ResourceLimited);
                }
                *target = u16::try_from(number).map_err(|_error| IndexError::ResourceLimited)?;
            }
        }
        Ok(Self {
            fields: sharing.fields,
            common,
            count,
            query_bytes,
            parameters,
        })
    }

    fn parameters(&self, slot: usize) -> Result<[usize; 4], IndexError> {
        if slot >= self.count {
            return Err(IndexError::ResourceLimited);
        }
        Ok((*self
            .parameters
            .get(slot)
            .ok_or(IndexError::ResourceLimited)?)
        .map(usize::from))
    }

    fn render_into(&self, query: &mut String, budget: &WorkBudget) -> Result<(), IndexError> {
        use std::fmt::Write as _;

        query.push_str(POSTING_PREFIX);
        for slot in 0..self.count {
            budget.checkpoint()?;
            if slot != 0 {
                query.push(',');
            }
            if self.common == 0 {
                query.push_str("(?,?,?,?)");
            } else {
                let [term, key, expected, next] = self.parameters(slot)?;
                write!(query, "(?{term},?{key},?{expected},?{next})")
                    .map_err(|_error| IndexError::ResourceLimited)?;
            }
        }
        if query.len() != self.query_bytes {
            return Err(IndexError::BackendViolation(
                "posting SQL differs from admitted length".into(),
            ));
        }
        Ok(())
    }
}

struct PostingValueBindings<'a> {
    values: [[Option<&'a MapValue>; MAX_WIDE_WRITE_BATCH_ROWS]; 2],
    recent_ids: [Option<(&'a MapValue, i64)>; 2],
}

fn posting_value_bindings_memory_bytes() -> Result<u64, IndexError> {
    u64::try_from(
        std::mem::size_of::<PostingValueBindings<'_>>()
            .checked_add(std::mem::size_of::<PostingSqlShape>())
            .ok_or(IndexError::ResourceLimited)?,
    )
    .map_err(|_error| IndexError::ResourceLimited)
}

fn posting_value_id<'a>(
    value: Option<&'a MapValue>,
    recent_ids: &mut [Option<(&'a MapValue, i64)>; 2],
    intern: &mut impl FnMut(&MapValue, &mut WorkBudget) -> Result<i64, IndexError>,
    budget: &mut WorkBudget,
) -> Result<Option<i64>, IndexError> {
    let Some(value) = value else {
        return Ok(None);
    };
    budget.checkpoint()?;
    let [first, second] = recent_ids;
    if let Some((prior, id)) = *first
        && prior == value
    {
        budget.checkpoint()?;
        return Ok(Some(id));
    }
    if let Some((prior, id)) = *second
        && prior == value
    {
        std::mem::swap(first, second);
        budget.checkpoint()?;
        return Ok(Some(id));
    }
    let id = intern(value, budget)?;
    budget.checkpoint()?;
    if id <= 0 {
        return Err(IndexError::BackendViolation(
            "posting value identity is not positive".into(),
        ));
    }
    *second = *first;
    *first = Some((value, id));
    Ok(Some(id))
}

/// Write canonical posting edits with shared bindings and private value IDs.
///
/// Original posting row fees and reservations remain unchanged. All row
/// callbacks, canonical value encodings and pure bindings complete before the
/// first `intern` call, preserving preexecute refusals without DB mutation.
/// The adapter owns exact value interning, its extra work/admission, and
/// disposal after an interning/write error; `journal_mode=OFF` is not rollback.
/// `before_execute` marks the start of a possible SQL effect even for all-NULL
/// values, which never invoke `intern`. It runs after all preexecute checks.
/// Two admitted, borrowed value IDs avoid repeated dictionary probes within a
/// mixed-owner packet; no identity is retained across calls.
/// `intern` must return the same positive ID for equal values within this call.
/// Rows must remain immutable until this call completes.
pub fn write_posting_edit_fields_batch<'a>(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    reservations: &[u64],
    row: impl Fn(usize) -> Result<(&'a [u8], MapEditFields<'a>), IndexError>,
    mut intern: impl FnMut(&MapValue, &mut WorkBudget) -> Result<i64, IndexError>,
    before_execute: impl FnOnce(),
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let mut phases = PostingPhases::new(budget);
    let result = (|| {
        let count = reservations.len();
        budget.checkpoint()?;
        if count == 0 || count > MAX_WIDE_WRITE_BATCH_ROWS {
            return Err(IndexError::InvalidInput(
                "invalid scratch edit batch".into(),
            ));
        }
        // Keep the existing 512-variable opt-in gate; compression cannot widen it.
        if count > MAX_WRITE_BATCH_ROWS && count > max_supported_wide_edit_batch_rows(db)? {
            return Err(IndexError::ResourceLimited);
        }
        let diagnostic =
            sampled_wide_posting_context(budget, MAX_WIDE_WRITE_BATCH_ROWS, POSTING_PREFIX);
        let (first_term, first) = row(0)?;
        let mut sharing = PostingSharing {
            fields: [count > 1; 3],
        };
        let mut bytes = 4096_u64;
        for slot in 0..count {
            budget.checkpoint()?;
            let (term, edit) = if slot == 0 {
                (first_term, first)
            } else {
                row(slot)?
            };
            let [key, expected, next] = &mut sharing.fields;
            *key = *key && edit.key == first.key;
            *expected = *expected && edit.expected == first.expected;
            *next = *next && edit.next == first.next;
            let fields = term
                .len()
                .checked_add(edit.key.len())
                .and_then(|length| length.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?;
            bytes = bytes
                .checked_add(u64::try_from(fields).map_err(|_error| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
            for value in [edit.expected, edit.next] {
                let length = value.map_or(Ok(1), |value| {
                    u64::try_from(value.encoded_len()?)
                        .map_err(|_error| IndexError::ResourceLimited)
                })?;
                bytes = bytes
                    .checked_add(length)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        let common = sharing.count();
        let query_bytes = PostingSharing::query_bytes(count, common)?;
        let original_query_bytes = PostingSharing::query_bytes(count, 0)?;
        // Preserve all logical fees; additionally account for longer numbered SQL.
        bytes = bytes
            .checked_add(
                u64::try_from(query_bytes.max(original_query_bytes))
                    .map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?,
            bytes,
            ..WorkUsage::default()
        })?;
        let _query_memory = budget.reserve_memory(
            u64::try_from(query_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let _value_bindings_memory = budget.reserve_memory(
            posting_value_bindings_memory_bytes()?,
            MemoryLifetime::Temporary,
        )?;
        let shape = PostingSqlShape::new(sharing, common, count, query_bytes)?;
        let mut value_bindings: PostingValueBindings<'a> = PostingValueBindings {
            values: [[None; MAX_WIDE_WRITE_BATCH_ROWS]; 2],
            recent_ids: [None; 2],
        };
        let mut query = String::new();
        query
            .try_reserve_exact(query_bytes)
            .map_err(|_error| IndexError::ResourceLimited)?;
        shape.render_into(&mut query, budget)?;
        phases.advance(37);
        let capacity_context = diagnostic
            .as_ref()
            .map(|context| phases.sampled_child(context));
        let capacity_span = capacity_context
            .as_ref()
            .and_then(|context| enter_posting_span(context, DiagnosticLaneV1(34)));
        let capacity = reserve_for_write_batch_with_limit(
            db,
            allocation,
            reservations,
            MAX_WIDE_WRITE_BATCH_ROWS,
            budget,
        );
        if let Some(span) = capacity_span {
            let _finished = span.finish(DiagnosticTerminalV1::from_result(&capacity));
        }
        capacity?;
        phases.advance(38);
        let mut statement = db.prepare_cached(&query).map_err(|error| sql(&error))?;
        let written = (|| {
            let [shared_key, shared_expected, shared_next] = shape.fields;
            let PostingValueBindings {
                values: [expected_values, next_values],
                recent_ids,
            } = &mut value_bindings;
            for slot in 0..count {
                budget.checkpoint()?;
                let (term, edit) = row(slot)?;
                let [term_parameter, key_parameter, ..] = shape.parameters(slot)?;
                statement
                    .raw_bind_parameter(term_parameter, term)
                    .map_err(|error| sql(&error))?;
                if slot == 0 || !shared_key {
                    statement
                        .raw_bind_parameter(key_parameter, edit.key)
                        .map_err(|error| sql(&error))?;
                }
                if slot == 0 || !shared_expected {
                    let _validated = edit.expected.map(map_value).transpose()?;
                    *expected_values
                        .get_mut(slot)
                        .ok_or(IndexError::ResourceLimited)? = edit.expected;
                }
                if slot == 0 || !shared_next {
                    let _validated = edit.next.map(map_value).transpose()?;
                    *next_values
                        .get_mut(slot)
                        .ok_or(IndexError::ResourceLimited)? = edit.next;
                }
            }
            // No input callback or encoding remains after this point. Interning
            // may mutate only this disposable scratch database, never publication.
            budget.checkpoint()?;
            for slot in 0..count {
                budget.checkpoint()?;
                let [_, _, expected_parameter, next_parameter] = shape.parameters(slot)?;
                if slot == 0 || !shared_expected {
                    let value = posting_value_id(
                        *expected_values
                            .get(slot)
                            .ok_or(IndexError::ResourceLimited)?,
                        recent_ids,
                        &mut intern,
                        budget,
                    )?;
                    statement
                        .raw_bind_parameter(expected_parameter, value)
                        .map_err(|error| sql(&error))?;
                }
                if slot == 0 || !shared_next {
                    let value = posting_value_id(
                        *next_values.get(slot).ok_or(IndexError::ResourceLimited)?,
                        recent_ids,
                        &mut intern,
                        budget,
                    )?;
                    statement
                        .raw_bind_parameter(next_parameter, value)
                        .map_err(|error| sql(&error))?;
                }
            }
            budget.checkpoint()?;
            phases.advance(39);
            let execute_context = diagnostic
                .as_ref()
                .map(|context| phases.sampled_child(context));
            let execute_span = execute_context
                .as_ref()
                .and_then(|context| enter_posting_span(context, DiagnosticLaneV1(35)));
            let pager_before = execute_span.as_ref().map(|_| db.pager_cache_status());
            before_execute();
            let executed = statement
                .raw_execute()
                .map_err(|error| sql(&error))
                .and_then(|changed| {
                    if changed == count {
                        Ok(changed)
                    } else {
                        Err(IndexError::BackendViolation(
                            "scratch batch changed unexpected row count".into(),
                        ))
                    }
                });
            if let Ok(changed) = &executed {
                phases.record_execute_rows(*changed);
            }
            if let Some(span) = execute_span {
                let (pager_after, changed) = match &executed {
                    Ok(changed) => (Some(db.pager_cache_status()), Some(*changed)),
                    Err(_primary) => (None, None),
                };
                if let Some(context) = execute_context.as_ref() {
                    record_posting_execute(context, &span, pager_before, pager_after, changed);
                }
                let _finished = span.finish(DiagnosticTerminalV1::from_result(&executed));
            }
            let _changed = executed?;
            budget.checkpoint()
        })();
        // CachedStatement::drop clears bindings through the vendored cache owner.
        // Drop it before _query_memory; manual clearing would scan every slot twice.
        drop(statement);
        written
    })();
    phases.finish(&result);
    result
}

fn write_map_edit_fields_batch_inner<'a>(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    batch: EditBatch<'_>,
    row: impl Fn(usize) -> Result<(ValueRef<'a>, EditFields<'a>, u64), IndexError>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let EditBatch {
        prefix,
        query_bytes,
        reservations,
        max_rows,
    } = batch;
    // Sample only the posting-wide production path. Ordinary batches and
    // requests without a trace context do no timing.
    let diagnostic = sampled_wide_posting_context(budget, max_rows, prefix);
    let count = reservations.len();
    if count == 0 || count > max_rows || query_bytes < 4096 {
        return Err(IndexError::InvalidInput(
            "invalid scratch edit batch".into(),
        ));
    }
    if count > MAX_WRITE_BATCH_ROWS && count > max_supported_wide_edit_batch_rows(db)? {
        return Err(IndexError::ResourceLimited);
    }
    let sql_bytes = map_edit_sql_bytes(prefix, count)?;
    let sql_bytes_u64 = u64::try_from(sql_bytes).map_err(|_error| IndexError::ResourceLimited)?;
    let mut bytes = query_bytes
        .checked_add(sql_bytes_u64)
        .ok_or(IndexError::ResourceLimited)?;
    for slot in 0..count {
        budget.checkpoint()?;
        let (partition, edit, index_bytes) = row(slot)?;
        let partition_bytes = match partition {
            ValueRef::Integer(_) => 8,
            ValueRef::Blob(value) => {
                u64::try_from(value.len()).map_err(|_error| IndexError::ResourceLimited)?
            }
            ValueRef::Null | ValueRef::Real(_) | ValueRef::Text(_) => {
                return Err(IndexError::InvalidInput(
                    "invalid scratch edit partition".into(),
                ));
            }
        };
        let mut fields = partition_bytes
            .checked_add(
                u64::try_from(edit.key.len()).map_err(|_error| IndexError::ResourceLimited)?,
            )
            .and_then(|bytes| bytes.checked_add(64))
            .and_then(|bytes| bytes.checked_add(index_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        for value in [edit.expected, edit.next] {
            let length = value.map_or(Ok(1), |value| {
                u64::try_from(value.encoded_len()?).map_err(|_error| IndexError::ResourceLimited)
            })?;
            fields = fields
                .checked_add(length)
                .ok_or(IndexError::ResourceLimited)?;
        }
        bytes = bytes
            .checked_add(fields)
            .ok_or(IndexError::ResourceLimited)?;
    }
    budget.charge(WorkUsage {
        writes: u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?,
        bytes,
        ..WorkUsage::default()
    })?;
    let _sql_memory = budget.reserve_memory(sql_bytes_u64, MemoryLifetime::Temporary)?;
    let mut query = String::new();
    query
        .try_reserve_exact(sql_bytes)
        .map_err(|_error| IndexError::ResourceLimited)?;
    query.push_str(prefix);
    for slot in 0..count {
        if slot != 0 {
            query.push(',');
        }
        query.push_str("(?,?,?,?)");
    }
    let capacity_span = diagnostic
        .as_ref()
        .and_then(|context| enter_posting_span(context, DiagnosticLaneV1(34)));
    let capacity =
        reserve_for_write_batch_with_limit(db, allocation, reservations, max_rows, budget);
    if let Some(span) = capacity_span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(&capacity));
    }
    capacity?;
    let mut statement = db.prepare_cached(&query).map_err(|error| sql(&error))?;
    statement.clear_bindings();
    for slot in 0..count {
        budget.checkpoint()?;
        let (partition, edit, _index_bytes) = row(slot)?;
        let first = slot
            .checked_mul(4)
            .and_then(|value| value.checked_add(1))
            .ok_or(IndexError::ResourceLimited)?;
        statement
            .raw_bind_parameter(first, rusqlite::types::ToSqlOutput::Borrowed(partition))
            .map_err(|error| sql(&error))?;
        statement
            .raw_bind_parameter(
                first.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                edit.key,
            )
            .map_err(|error| sql(&error))?;
        for (offset, value) in [(2usize, edit.expected), (3usize, edit.next)] {
            let parameter = first.checked_add(offset).ok_or(IndexError::ResourceLimited)?;
            match value {
                None => statement
                    .raw_bind_parameter(parameter, Option::<i64>::None)
                    .map_err(|error| sql(&error))?,
                Some(EditValueRef::Decoded(value)) => statement
                    .raw_bind_parameter(parameter, map_value(value)?)
                    .map_err(|error| sql(&error))?,
                Some(EditValueRef::CanonicalWire(wire)) => statement
                    .raw_bind_parameter(parameter, wire)
                    .map_err(|error| sql(&error))?,
            };
        }
    }
    budget.checkpoint()?;
    let execute_span = diagnostic
        .as_ref()
        .and_then(|context| enter_posting_span(context, DiagnosticLaneV1(35)));
    let pager_before = execute_span.as_ref().map(|_| db.pager_cache_status());
    let executed = statement
        .raw_execute()
        .map_err(|error| sql(&error))
        .and_then(|changed| {
            if changed == count {
                Ok(changed)
            } else {
                Err(IndexError::BackendViolation(
                    "scratch batch changed unexpected row count".into(),
                ))
            }
        });
    if let Some(span) = execute_span {
        let (pager_after, changed) = match &executed {
            Ok(changed) => (Some(db.pager_cache_status()), Some(*changed)),
            Err(_primary) => (None, None),
        };
        if let Some(context) = diagnostic.as_ref() {
            record_posting_execute(context, &span, pager_before, pager_after, changed);
        }
        let _finished = span.finish(DiagnosticTerminalV1::from_result(&executed));
    }
    let _changed = executed?;
    budget.checkpoint()?;
    statement.clear_bindings();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        POSTING_PREFIX, PostingSharing, PostingSqlShape, map_edit_sql_bytes,
        map_edit_sql_memory_bound, posting_span_health, record_posting_execute,
        sampled_batch_ordinal, sampled_wide_posting_context,
    };
    use quanta_index_core::index::{WorkBudget, WorkLimits};
    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
        DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticHealthCodeV1,
        DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticStageV1, DiagnosticTraceErrorV1,
    };
    use rusqlite::{Error, PagerCacheStatus};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    #[test]
    fn posting_span_admission_preserves_health_reason() {
        for (error, health) in [
            (
                DiagnosticTraceErrorV1::SpanIdentityExhausted,
                DiagnosticHealthCodeV1::CounterOverflow,
            ),
            (
                DiagnosticTraceErrorV1::ClockRegressed,
                DiagnosticHealthCodeV1::ClockRegressed,
            ),
            (
                DiagnosticTraceErrorV1::ForeignCaptureSpan,
                DiagnosticHealthCodeV1::ForeignCaptureSpan,
            ),
            (
                DiagnosticTraceErrorV1::MissingParentSpan,
                DiagnosticHealthCodeV1::MissingParentSpan,
            ),
        ] {
            assert_eq!(posting_span_health(error), health);
        }
    }

    #[test]
    fn posting_shape_uses_independent_parameter_oracle() -> Result<(), Box<dyn std::error::Error>> {
        use std::fmt::Write as _;

        let budget = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or("clock overflow")?,
        );
        for mask in 0_u8..8 {
            let fields = [mask & 1 != 0, mask & 2 != 0, mask & 4 != 0];
            let sharing = PostingSharing { fields };
            let common = sharing.count();
            for count in 1_usize..=super::MAX_WIDE_WRITE_BATCH_ROWS {
                let bytes = PostingSharing::query_bytes(count, common)?;
                let shape = PostingSqlShape::new(sharing, common, count, bytes)?;
                let mut shared = [None; 4];
                let mut ordinal = 0_usize;
                for column in 1_usize..4 {
                    if fields.get(column.checked_sub(1).ok_or("column underflow")?) == Some(&true) {
                        ordinal = ordinal.checked_add(1).ok_or("parameter overflow")?;
                        *shared.get_mut(column).ok_or("missing shared field")? = Some(ordinal);
                    }
                }
                let mut oracle = String::from(POSTING_PREFIX);
                for slot in 0..count {
                    if slot != 0 {
                        oracle.push(',');
                    }
                    let mut numbers = [0; 4];
                    for (column, number) in numbers.iter_mut().enumerate() {
                        *number = if let Some(shared_number) =
                            shared.get(column).ok_or("missing field")?
                        {
                            *shared_number
                        } else {
                            ordinal = ordinal.checked_add(1).ok_or("parameter overflow")?;
                            ordinal
                        };
                    }
                    if shape.parameters(slot)? != numbers {
                        return Err("posting parameter oracle mismatch".into());
                    }
                    if common == 0 {
                        oracle.push_str("(?,?,?,?)");
                    } else {
                        let [term, key, expected, next] = numbers;
                        write!(&mut oracle, "(?{term},?{key},?{expected},?{next})")?;
                    }
                }
                let mut rendered = String::new();
                shape.render_into(&mut rendered, &budget)?;
                if bytes != oracle.len() {
                    return Err("posting SQL length oracle mismatch".into());
                }
                if rendered != oracle {
                    return Err("posting SQL wire oracle mismatch".into());
                }
                if ordinal > 512 {
                    return Err("posting parameter limit exceeded".into());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn inventory_edit_sql_bound_matches_writer_allocation() {
        const PREFIX: &str = "INSERT INTO inventory(root,k,p,n) VALUES ";
        for count in [1, 2, 63, 64] {
            let bound = map_edit_sql_memory_bound(PREFIX, count).expect("valid packet");
            let actual = map_edit_sql_bytes(PREFIX, count).expect("writer SQL length");
            assert_eq!(u64::try_from(actual), Ok(bound.temporary_data_bytes));
            assert_eq!(bound.max_live_temporary_leases, 1);
        }
        assert!(map_edit_sql_memory_bound(PREFIX, 0).is_err());
        assert!(map_edit_sql_memory_bound(PREFIX, 65).is_err());
    }

    #[derive(Debug, Default)]
    struct TickClock(AtomicU64);

    impl DiagnosticClockV1 for TickClock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }

    #[derive(Debug, Default)]
    struct Events(Mutex<Vec<DiagnosticEmitV1>>);

    impl DiagnosticEventSinkV1 for Events {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            let Ok(mut rows) = self.0.lock() else {
                return false;
            };
            rows.push(event);
            true
        }
    }

    type Capture = (
        DiagnosticContextV1,
        Arc<Events>,
        quanta_index_core::DiagnosticSpanGuardV1,
        quanta_index_core::DiagnosticSpanGuardV1,
    );

    fn capture(id: u8) -> Result<Capture, Box<dyn std::error::Error>> {
        let events = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([id; 16]),
            Arc::new(TickClock::default()),
            events.clone(),
        );
        let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
        let child = context.child_context(&root);
        let span = child.enter(
            DiagnosticStageV1::NativeTextPostingEdit,
            DiagnosticLaneV1(35),
        )?;
        Ok((child, events, root, span))
    }

    #[test]
    fn off_posting_has_no_sampled_context() {
        let budget = WorkBudget::new(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            Instant::now() + Duration::from_secs(10),
        );
        assert!(sampled_wide_posting_context(&budget, 128, POSTING_PREFIX).is_none());
    }

    #[test]
    fn successful_posting_sample_emits_complete_pager_and_actual_row_packet()
    -> Result<(), Box<dyn std::error::Error>> {
        let (context, events, _root, span) = capture(31)?;
        let before = PagerCacheStatus {
            hit: 11,
            miss: 17,
            write: 23,
            spill: 3,
        };
        let after = PagerCacheStatus {
            hit: 18,
            miss: 19,
            write: 29,
            spill: 3,
        };
        record_posting_execute(&context, &span, Some(Ok(before)), Some(Ok(after)), Some(7));
        let rows = events
            .0
            .lock()
            .map_err(|error| format!("diagnostic event lock was poisoned: {error}"))?;
        let counters: Vec<_> = rows
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::Counter { key, value, .. } = event.kind {
                    Some((key, value))
                } else {
                    None
                }
            })
            .collect();
        let expected = [
            (DiagnosticCounterV1::SqliteRows, 7),
            (DiagnosticCounterV1::SqliteBatches, 1),
            (DiagnosticCounterV1::SqliteCacheHits, 7),
            (DiagnosticCounterV1::SqliteCacheMisses, 2),
            (DiagnosticCounterV1::SqliteCacheWrites, 6),
            (DiagnosticCounterV1::SqliteCacheSpills, 0),
        ];
        if counters != expected {
            return Err(format!("posting counters differ: {counters:?} != {expected:?}").into());
        }
        if rows
            .iter()
            .any(|event| matches!(event.kind, DiagnosticEventKindV1::Health { .. }))
        {
            return Err("successful posting emitted diagnostic health".into());
        }
        drop(rows);
        Ok(())
    }

    #[test]
    fn unavailable_or_regressed_pager_keeps_rows_but_failed_sql_emits_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        let before = PagerCacheStatus {
            hit: 10,
            miss: 10,
            write: 10,
            spill: 10,
        };
        for (id, first, last, changed, health) in [
            (
                1,
                None,
                Some(Ok(before)),
                Some(7),
                DiagnosticHealthCodeV1::CounterUnavailable,
            ),
            (
                2,
                Some(Err(Error::InvalidQuery)),
                Some(Ok(before)),
                Some(7),
                DiagnosticHealthCodeV1::CounterUnavailable,
            ),
            (
                3,
                Some(Ok(before)),
                Some(Err(Error::InvalidQuery)),
                Some(7),
                DiagnosticHealthCodeV1::CounterUnavailable,
            ),
            (
                4,
                Some(Ok(before)),
                None,
                None,
                DiagnosticHealthCodeV1::CounterUnavailable,
            ),
            (
                5,
                Some(Ok(before)),
                Some(Ok(PagerCacheStatus { spill: 9, ..before })),
                Some(7),
                DiagnosticHealthCodeV1::CounterRegressed,
            ),
        ] {
            let (context, events, _root, span) = capture(id)?;
            record_posting_execute(&context, &span, first, last, changed);
            let rows = events
                .0
                .lock()
                .map_err(|error| format!("diagnostic event lock was poisoned: {error}"))?;
            let counters: Vec<_> = rows
                .iter()
                .filter_map(|event| {
                    if let DiagnosticEventKindV1::Counter { key, value, .. } = event.kind {
                        Some((key, value))
                    } else {
                        None
                    }
                })
                .collect();
            if changed.is_some() {
                let expected = [
                    (DiagnosticCounterV1::SqliteRows, 7),
                    (DiagnosticCounterV1::SqliteBatches, 1),
                ];
                if counters != expected {
                    return Err(format!(
                        "partial posting counters differ: {counters:?} != {expected:?}"
                    )
                    .into());
                }
            } else if !counters.is_empty() {
                return Err(format!("failed posting emitted counters: {counters:?}").into());
            }
            let actual_health = rows
                .iter()
                .filter(|event| matches!(event.kind, DiagnosticEventKindV1::Health { code } if code == health))
                .count();
            let expected_health = usize::from(changed.is_some());
            if actual_health != expected_health {
                return Err(format!(
                    "posting health count differs: {actual_health} != {expected_health}"
                )
                .into());
            }
            drop(rows);
        }
        Ok(())
    }

    #[test]
    fn wide_batch_samples_are_bounded_and_cover_late_batches() {
        for ordinal in 1..=8 {
            assert!(sampled_batch_ordinal(ordinal));
        }
        for ordinal in [9, 15, 17, 63, 65, 75_723] {
            assert!(!sampled_batch_ordinal(ordinal));
        }
        for ordinal in [16, 32, 64, 65_536] {
            assert!(sampled_batch_ordinal(ordinal));
        }
        let sampled = (1..=75_723)
            .filter(|ordinal| sampled_batch_ordinal(*ordinal))
            .count();
        assert_eq!(sampled, 21);
    }
}
