//! One real SQL execution for a bounded batch of the existing edit IR.

use std::cell::Cell;

use quanta_index_core::index::{
    IndexError, MapEdit, MapValue, MemoryLifetime, ScratchAllocation, WorkBudget, WorkUsage,
};
use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticContextV1, DiagnosticLaneV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};
use rusqlite::{Connection, limits::Limit, types::ValueRef};

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
        row,
        budget,
    )
}

/// Maximum opt-in width supported by this actual SQLite connection. A lower
/// run-time variable limit keeps posting callers on their existing 64-row path.
pub fn max_supported_wide_edit_batch_rows(db: &Connection) -> Result<usize, IndexError> {
    let limit = db
        .limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER)
        .map_err(|error| sql(&error))?;
    const WIDE_PARAMETERS: usize = MAX_WIDE_WRITE_BATCH_ROWS * 4;
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
        row,
        budget,
    )
}

struct EditBatch<'a> {
    prefix: &'static str,
    query_bytes: u64,
    reservations: &'a [u64],
    max_rows: usize,
}

fn write_map_edit_fields_batch_inner<'a>(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    batch: EditBatch<'_>,
    row: impl Fn(usize) -> Result<(ValueRef<'a>, MapEditFields<'a>, u64), IndexError>,
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
    let sql_bytes = count
        .checked_mul("(?,?,?,?)".len())
        .and_then(|bytes| bytes.checked_add(count.checked_sub(1)?))
        .and_then(|bytes| bytes.checked_add(prefix.len()))
        .ok_or(IndexError::ResourceLimited)?;
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
    let capacity_span = diagnostic.as_ref().and_then(|context| {
        context
            .enter(
                DiagnosticStageV1::NativeTextPostingEdit,
                DiagnosticLaneV1(34),
            )
            .ok()
    });
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
        let expected = edit.expected.map(map_value).transpose()?;
        let next = edit.next.map(map_value).transpose()?;
        statement
            .raw_bind_parameter(
                first.checked_add(2).ok_or(IndexError::ResourceLimited)?,
                expected,
            )
            .map_err(|error| sql(&error))?;
        statement
            .raw_bind_parameter(
                first.checked_add(3).ok_or(IndexError::ResourceLimited)?,
                next,
            )
            .map_err(|error| sql(&error))?;
    }
    budget.checkpoint()?;
    let execute_span = diagnostic.as_ref().and_then(|context| {
        context
            .enter(
                DiagnosticStageV1::NativeTextPostingEdit,
                DiagnosticLaneV1(35),
            )
            .ok()
    });
    let executed = statement
        .raw_execute()
        .map_err(|error| sql(&error))
        .and_then(|changed| {
            if changed == count {
                Ok(())
            } else {
                Err(IndexError::BackendViolation(
                    "scratch batch changed unexpected row count".into(),
                ))
            }
        });
    if let Some(span) = execute_span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(&executed));
    }
    executed?;
    budget.checkpoint()?;
    statement.clear_bindings();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sampled_batch_ordinal;

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
