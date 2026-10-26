//! One real SQL execution for a bounded batch of the existing edit IR.

use quanta_index_core::index::{
    IndexError, MapEdit, MemoryLifetime, ScratchAllocation, WorkBudget, WorkUsage,
};
use rusqlite::{Connection, types::ValueRef};

use crate::{MAX_WRITE_BATCH_ROWS, map_value, reserve_for_write_batch, storage_error};

fn sql(error: &rusqlite::Error) -> IndexError {
    storage_error(error, "scratch edit batch")
}

/// Write one bounded edit batch with bound parameters.
///
/// `prefix` is the adapter's constant INSERT table/column clause, ending in VALUES.
/// Rows must stay immutable for this call.
/// Original query fees, every logical write, actual fields and physical row
/// reservations are separate. Each row also declares its secondary-index work.
/// Any error poisons this disposable attempt.
pub fn write_map_edit_batch<'a>(
    db: &Connection,
    allocation: &dyn ScratchAllocation,
    prefix: &'static str,
    query_bytes: u64,
    reservations: &[u64],
    row: impl Fn(usize) -> Result<(ValueRef<'a>, &'a MapEdit, u64), IndexError>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let count = reservations.len();
    if count == 0 || count > MAX_WRITE_BATCH_ROWS || query_bytes < 4096 {
        return Err(IndexError::InvalidInput(
            "invalid scratch edit batch".into(),
        ));
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
                u64::try_from(value.encoded()?.as_slice()?.len())
                    .map_err(|_error| IndexError::ResourceLimited)
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
    reserve_for_write_batch(db, allocation, reservations, budget)?;
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
                edit.key.as_slice(),
            )
            .map_err(|error| sql(&error))?;
        let expected = edit.expected.as_ref().map(map_value).transpose()?;
        let next = edit.next.as_ref().map(map_value).transpose()?;
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
    if statement.raw_execute().map_err(|error| sql(&error))? != count {
        return Err(IndexError::BackendViolation(
            "scratch batch changed unexpected row count".into(),
        ));
    }
    budget.checkpoint()?;
    statement.clear_bindings();
    Ok(())
}
