//! Pre-admitted, attempt-local recording for one physical occurrence installation.

use quanta_index_core::index::{
    AttemptRef, IndexError, MemoryCreditLimits, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget,
    WorkUsage, ports::MAX_OBJECT_INSTALL_BATCH,
};

use super::{TextSpool, reference_bytes, sql};

const ROWS_PER_PROGRAM: usize = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
const MAX_PROGRAMS: usize = MAX_OBJECT_INSTALL_BATCH.div_ceil(ROWS_PER_PROGRAM);
const ROW_RESERVATION_BYTES: u64 = 128;
const INSERT_PREFIX: &str = "INSERT OR IGNORE INTO confirmed_occurrence(ref) VALUES ";

fn program_bytes(references: usize) -> Result<usize, IndexError> {
    if references == 0 || references > MAX_OBJECT_INSTALL_BATCH {
        return Err(IndexError::ResourceLimited);
    }
    let mut remaining = references;
    let mut total = 0usize;
    while remaining != 0 {
        let rows = remaining.min(ROWS_PER_PROGRAM);
        let values = rows
            .checked_mul("(?)".len())
            .and_then(|bytes| bytes.checked_add(rows.saturating_sub(1)))
            .ok_or(IndexError::ResourceLimited)?;
        total = total
            .checked_add(INSERT_PREFIX.len())
            .and_then(|bytes| bytes.checked_add(values))
            .ok_or(IndexError::ResourceLimited)?;
        remaining -= rows;
    }
    Ok(total)
}

pub(super) fn confirmation_program_memory_bound(
    maximum_refs: usize,
) -> Result<MemoryCreditLimits, IndexError> {
    Ok(MemoryCreditLimits {
        temporary_data_bytes: u64::try_from(program_bytes(maximum_refs)?)
            .map_err(|_error| IndexError::ResourceLimited)?,
        retained_data_bytes: 0,
        max_live_temporary_leases: 1,
        max_live_retained_leases: 0,
    })
}

/// Optional original backing for the maximum union receipt. The retained
/// buffers are moved into the ordinary record; no release/re-reserve occurs.
pub(in crate::native) struct ConfirmedOccurrencePrograms {
    programs: [Option<String>; MAX_PROGRAMS],
    memory: Option<MemoryLease>,
}

impl ConfirmedOccurrencePrograms {
    pub(in crate::native) fn try_new(budget: &WorkBudget) -> Result<Option<Self>, IndexError> {
        let values = ROWS_PER_PROGRAM
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_sub(1))
            .ok_or(IndexError::ResourceLimited)?;
        let capacity = INSERT_PREFIX
            .len()
            .checked_add(values)
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = capacity
            .checked_mul(MAX_PROGRAMS)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .ok_or(IndexError::ResourceLimited)?;
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let mut programs = std::array::from_fn(|_| None);
        for slot in &mut programs {
            budget.checkpoint()?;
            let mut program = String::new();
            if program.try_reserve_exact(capacity).is_err() || program.capacity() > capacity {
                return Ok(None);
            }
            *slot = Some(program);
        }
        Ok(Some(Self {
            programs,
            memory: Some(memory),
        }))
    }
}

/// An exact packet receipt prepared before durable installation.
///
/// The caller reserves `scratch_bytes` together with
/// its other scratch demands before installation and calls `finish` only after
/// every returned immutable identity matches the packet.
#[must_use]
pub(super) struct ConfirmedOccurrenceRecord<'a> {
    spool: &'a TextSpool,
    references: [ObjectRef; MAX_OBJECT_INSTALL_BATCH],
    len: usize,
    attempt: AttemptRef,
    // SQL strings die before their original temporary allocation grant.
    programs: [Option<String>; MAX_PROGRAMS],
    _program_memory: Option<MemoryLease>,
}

impl<'a> ConfirmedOccurrenceRecord<'a> {
    pub(super) fn prepare(
        spool: &'a TextSpool,
        references: &[ObjectRef],
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        Self::prepare_with_programs(spool, references, attempt, None, budget)
    }

    pub(super) fn prepare_with_programs(
        spool: &'a TextSpool,
        references: &[ObjectRef],
        attempt: AttemptRef,
        backing: Option<ConfirmedOccurrencePrograms>,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        spool.check_confirmed_attempt(attempt)?;
        if references.is_empty() || references.len() > MAX_OBJECT_INSTALL_BATCH {
            return Err(IndexError::ResourceLimited);
        }
        for (slot, reference) in references.iter().enumerate() {
            budget.checkpoint()?;
            if references
                .get(..slot)
                .is_some_and(|earlier| earlier.contains(reference))
            {
                return Err(IndexError::BackendViolation(
                    "confirmed occurrence packet repeats an identity".into(),
                ));
            }
            let _wire = reference_bytes(reference)?;
        }
        let rows = u64::try_from(references.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: rows,
            bytes: rows
                .checked_mul(ROW_RESERVATION_BYTES)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;

        // Census and fund every variable SQL byte before creating a String or
        // asking SQLite to compile any of the bounded programs.
        let program_bytes = program_bytes(references.len())?;
        let prepaid = backing.is_some();
        let (program_memory, mut programs) = if let Some(backing) = backing {
            (backing.memory, backing.programs)
        } else {
            (
                budget.reserve_memory(
                    u64::try_from(program_bytes).map_err(|_error| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?,
                std::array::from_fn(|_| None),
            )
        };
        for (slot, chunk) in references.chunks(ROWS_PER_PROGRAM).enumerate() {
            budget.checkpoint()?;
            let bytes = INSERT_PREFIX
                .len()
                .checked_add(
                    chunk
                        .len()
                        .checked_mul("(?)".len())
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .and_then(|bytes| bytes.checked_add(chunk.len().saturating_sub(1)))
                .ok_or(IndexError::ResourceLimited)?;
            let mut program = programs
                .get_mut(slot)
                .and_then(Option::take)
                .unwrap_or_default();
            if prepaid {
                if program.capacity() < bytes {
                    return Err(IndexError::BackendViolation(
                        "prepaid confirmation SQL backing differs".into(),
                    ));
                }
            } else {
                program
                    .try_reserve_exact(bytes)
                    .map_err(|_error| IndexError::ResourceLimited)?;
            }
            program.clear();
            program.push_str(INSERT_PREFIX);
            for index in 0..chunk.len() {
                if index != 0 {
                    program.push(',');
                }
                program.push_str("(?)");
            }
            if program.len() != bytes {
                return Err(IndexError::BackendViolation(
                    "confirmed occurrence SQL program changed its census".into(),
                ));
            }
            // Compilation and its SQL allocation occur before durable object
            // installation. No SQLite scope survives the preparation call.
            let io_scope = spool.db.scoped(budget)?;
            drop(
                io_scope
                    .prepare_cached(&program)
                    .map_err(|error| sql(&error))?,
            );
            *programs.get_mut(slot).ok_or(IndexError::ResourceLimited)? = Some(program);
        }
        let first = *references.first().ok_or(IndexError::ResourceLimited)?;
        let mut owned = [first; MAX_OBJECT_INSTALL_BATCH];
        owned
            .get_mut(..references.len())
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(references);
        Ok(Self {
            spool,
            references: owned,
            len: references.len(),
            attempt,
            programs,
            _program_memory: program_memory,
        })
    }

    pub(super) fn scratch_bytes(&self) -> Result<u64, IndexError> {
        u64::try_from(self.len)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(ROW_RESERVATION_BYTES)
            .ok_or(IndexError::ResourceLimited)
    }

    pub(super) fn finish(self, budget: &WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.spool.check_confirmed_attempt(self.attempt)?;
        let exact = self.references.get(..self.len).ok_or_else(|| {
            IndexError::BackendViolation("confirmed occurrence receipt length changed".into())
        })?;
        for (slot, references) in exact.chunks(ROWS_PER_PROGRAM).enumerate() {
            budget.checkpoint()?;
            let program = self
                .programs
                .get(slot)
                .and_then(Option::as_deref)
                .ok_or_else(|| {
                    IndexError::BackendViolation(
                        "confirmed occurrence SQL program is missing".into(),
                    )
                })?;
            let first = reference_bytes(references.first().ok_or(IndexError::ResourceLimited)?)?;
            let mut values = [first; ROWS_PER_PROGRAM];
            for (value, reference) in values.iter_mut().zip(references) {
                *value = reference_bytes(reference)?;
            }
            let io_scope = self.spool.db.scoped(budget)?;
            let mut statement = io_scope
                .prepare_cached(program)
                .map_err(|error| sql(&error))?;
            statement.clear_bindings();
            let result = (|| {
                for (index, value) in values.iter().take(references.len()).enumerate() {
                    budget.checkpoint()?;
                    statement
                        .raw_bind_parameter(
                            index.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                            value,
                        )
                        .map_err(|error| sql(&error))?;
                }
                budget.checkpoint()?;
                statement.raw_execute().map_err(|error| sql(&error))
            })();
            statement.clear_bindings();
            let changed = result?;
            if changed > references.len() {
                return Err(IndexError::BackendViolation(
                    "confirmed occurrence SQL inserted excess rows".into(),
                ));
            }
        }
        budget.checkpoint()
    }
}
