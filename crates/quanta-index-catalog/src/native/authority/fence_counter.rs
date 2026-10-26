//! One canonical global counter, with conditional transaction-local reuse.

use quanta_index_core::index::{IndexError, WorkBudget, WorkUsage};
use rusqlite::{Connection, Transaction, params};

use super::{cached_execute, cached_query_row, exactly_one, fields_digest, sql, uint};

#[derive(Clone, Copy)]
struct FenceRow {
    next: i64,
    checksum: [u8; 32],
}

impl FenceRow {
    fn read(connection: &Connection) -> Result<Self, IndexError> {
        let (next, checksum): (i64, Option<Vec<u8>>) = cached_query_row(
            connection,
            "SELECT CASE WHEN typeof(next)='integer' THEN next END,
                    CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
             FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).map_err(|error| sql(&error))?;
        let checksum: [u8; 32] = checksum
            .ok_or_else(|| IndexError::Storage("native counter checksum shape invalid".into()))?
            .try_into()
            .map_err(|_bytes| {
                IndexError::Storage("native counter checksum shape invalid".into())
            })?;
        if checksum.as_slice()
            != fields_digest(b"qidx.native.counter.v1", &("attempt_fence", next))?
        {
            return Err(IndexError::Storage("native counter digest mismatch".into()));
        }
        if next <= 0 {
            return Err(IndexError::Storage(
                "native counter must be positive".into(),
            ));
        }
        Ok(Self { next, checksum })
    }

    fn allocate(&mut self) -> Result<u64, IndexError> {
        let successor = self
            .next
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let successor_checksum: [u8; 32] =
            fields_digest(b"qidx.native.counter.v1", &("attempt_fence", successor))?
                .try_into()
                .map_err(|_bytes| {
                    IndexError::Storage("native counter checksum shape invalid".into())
                })?;
        let allocated = uint(self.next)?;
        self.next = successor;
        self.checksum = successor_checksum;
        Ok(allocated)
    }

    fn persist(&self, connection: &Connection, initial: &Self) -> Result<(), IndexError> {
        let changed = cached_execute(
            connection,
            "UPDATE native_counter_v1 SET next=?1,row_digest=?2
             WHERE name='attempt_fence' AND next=?3 AND row_digest=?4",
            params![
                self.next,
                self.checksum.as_slice(),
                initial.next,
                initial.checksum.as_slice()
            ],
        )
        .map_err(|error| sql(&error))?;
        exactly_one(changed, "advance fence counter")
    }

    fn advance(&mut self, connection: &Connection) -> Result<u64, IndexError> {
        let initial = *self;
        let allocated = self.allocate()?;
        self.persist(connection, &initial)?;
        Ok(allocated)
    }
}

pub(super) fn next(connection: &Connection) -> Result<u64, IndexError> {
    FenceRow::read(connection)?.advance(connection)
}

/// The caller's admitted input count bounds this transaction's sequence.
/// It is created after child adoption and never escapes managed reservation.
pub(super) struct ReservationSequence<'tx, 'conn> {
    transaction: &'tx Transaction<'conn>,
    initial: Option<FenceRow>,
    row: Option<FenceRow>,
    remaining: usize,
    refused: bool,
}

impl<'tx, 'conn> ReservationSequence<'tx, 'conn> {
    pub(super) fn run<T>(
        transaction: &'tx Transaction<'conn>,
        limit: usize,
        budget: &mut WorkBudget,
        work: impl FnOnce(&mut Self, &mut WorkBudget) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        let mut sequence = Self {
            transaction,
            initial: None,
            row: None,
            remaining: limit,
            refused: false,
        };
        sequence.ensure_open()?;
        let output = match work(&mut sequence, budget) {
            Ok(output) => output,
            Err(error) => {
                sequence.refuse();
                return Err(error);
            }
        };
        if let Err(error) = budget.checkpoint() {
            sequence.refuse();
            return Err(error);
        }
        sequence.ensure_open()?;
        if let (Some(initial), Some(row)) = (sequence.initial, sequence.row)
            && let Err(error) = row.persist(sequence.transaction, &initial)
        {
            sequence.refuse();
            return Err(error);
        }
        Ok(output)
    }

    pub(super) fn transaction(&self) -> &'tx Transaction<'conn> {
        self.transaction
    }

    pub(super) fn ensure_open(&mut self) -> Result<(), IndexError> {
        if self.refused {
            return Err(IndexError::BackendViolation(
                "fence sequence already refused".into(),
            ));
        }
        if self.transaction.is_autocommit() {
            self.refused = true;
            return Err(IndexError::BackendViolation(
                "fence sequence requires a transaction".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn refuse(&mut self) {
        self.refused = true;
    }

    pub(super) fn next(&mut self, budget: &mut WorkBudget) -> Result<u64, IndexError> {
        self.ensure_open()?;
        let result = self.next_inner(budget);
        if result.is_err() {
            self.refused = true;
        }
        result
    }

    fn next_inner(&mut self, budget: &mut WorkBudget) -> Result<u64, IndexError> {
        budget.checkpoint()?;
        if self.remaining == 0 {
            return Err(IndexError::BackendViolation(
                "fence sequence exceeded admitted inputs".into(),
            ));
        }
        if self.row.is_none() {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let row = FenceRow::read(self.transaction)?;
            self.initial = Some(row);
            self.row = Some(row);
        }
        let row = self
            .row
            .as_mut()
            .ok_or_else(|| IndexError::BackendViolation("fence sequence lacks its row".into()))?;
        let allocated = row.allocate()?;
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(allocated)
    }
}
