//! Pre-admitted, attempt-local recording for one physical occurrence installation.

use quanta_index_core::index::{
    AttemptRef, IndexError, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget, WorkUsage,
    ports::MAX_OBJECT_INSTALL_BATCH,
};

use super::{TextSpool, reference_bytes, sql};

const ROWS_PER_PROGRAM: usize = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
const MAX_PROGRAMS: usize = MAX_OBJECT_INSTALL_BATCH.div_ceil(ROWS_PER_PROGRAM);
const ROW_RESERVATION_BYTES: u64 = 128;
const INSERT_PREFIX: &str = "INSERT OR IGNORE INTO confirmed_occurrence(ref) VALUES ";

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
        let program_bytes =
            references
                .chunks(ROWS_PER_PROGRAM)
                .try_fold(0usize, |total, chunk| {
                    let values = chunk
                        .len()
                        .checked_mul("(?)".len())
                        .and_then(|bytes| bytes.checked_add(chunk.len().saturating_sub(1)))
                        .ok_or(IndexError::ResourceLimited)?;
                    let bytes = INSERT_PREFIX
                        .len()
                        .checked_add(values)
                        .ok_or(IndexError::ResourceLimited)?;
                    total.checked_add(bytes).ok_or(IndexError::ResourceLimited)
                })?;
        let program_memory = budget.reserve_memory(
            u64::try_from(program_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut programs: [Option<String>; MAX_PROGRAMS] = std::array::from_fn(|_| None);
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
            let mut program = String::new();
            program
                .try_reserve_exact(bytes)
                .map_err(|_error| IndexError::ResourceLimited)?;
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
