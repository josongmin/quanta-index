//! Bounded query pages retain the existing canonical edit and its key custody.

use std::collections::VecDeque;

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, OrdinalMapEdit, WorkBudget, WorkUsage,
};
use rusqlite::{Row, Statement, ToSql};

const PAGE: usize = 64;

fn sql(error: &rusqlite::Error) -> IndexError {
    crate::storage_error(error, "scratch edit page")
}

#[derive(Default)]
pub struct MapEditPage {
    rows: VecDeque<OrdinalMapEdit>,
    memory: Option<MemoryLease>,
    complete: bool,
    malformed: Option<IndexError>,
}

impl MapEditPage {
    pub fn read(
        statement: &mut Statement<'_>,
        parameters: &[&dyn ToSql],
        query_bytes: u64,
        budget: &mut WorkBudget,
        mut decode: impl FnMut(&Row<'_>, &mut WorkBudget) -> Result<OrdinalMapEdit, IndexError>,
    ) -> Result<Self, IndexError> {
        if query_bytes < 4096 {
            return Err(IndexError::InvalidInput(
                "scratch query fee below one page".into(),
            ));
        }
        // Preserve the original per-query envelope. Rows and backing below
        // have additional charges; batching removes executions, not their fee.
        budget.charge(WorkUsage {
            reads: 1,
            bytes: query_bytes,
            ..WorkUsage::default()
        })?;
        let mut input = statement.query(parameters).map_err(|error| sql(&error))?;
        let mut page = Self::default();
        while let Some(row) = input.next().map_err(|error| sql(&error))? {
            // The original query read includes its first row; every additional
            // row retains a read charge even though no new seek is performed.
            if !page.rows.is_empty() {
                budget.charge(WorkUsage {
                    reads: 1,
                    ..WorkUsage::default()
                })?;
            }
            if page.rows.len() == PAGE {
                return Err(IndexError::ResourceLimited);
            }
            let edit = match decode(row, budget) {
                Ok(edit) => edit,
                Err(error @ IndexError::CorruptObject(_)) if !page.rows.is_empty() => {
                    // Keep valid predecessors observable at their original
                    // positions. Never advance past or hide the malformed row.
                    page.malformed = Some(error);
                    return Ok(page);
                }
                Err(error) => return Err(error),
            };
            if page.rows.capacity() == 0 {
                let bytes = PAGE
                    .checked_mul(std::mem::size_of::<OrdinalMapEdit>())
                    .ok_or(IndexError::ResourceLimited)?;
                let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    bytes,
                    ..WorkUsage::default()
                })?;
                let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
                page.rows
                    .try_reserve_exact(PAGE)
                    .map_err(|_error| IndexError::ResourceLimited)?;
                page.memory = memory;
            }
            page.rows.push_back(edit);
        }
        page.complete = page.rows.len() < PAGE;
        Ok(page)
    }

    pub fn pop(&mut self) -> Result<Option<OrdinalMapEdit>, IndexError> {
        if let Some(row) = self.rows.pop_front() {
            return Ok(Some(row));
        }
        self.malformed
            .as_ref()
            .map_or_else(|| Ok(None), |error| Err(error.clone()))
    }

    #[must_use]
    pub fn complete(&self) -> bool {
        self.complete
    }
}

pub fn read_map_edit_budgeted(
    row: &Row<'_>,
    key_bound: usize,
    root: u32,
    budget: &mut WorkBudget,
) -> Result<OrdinalMapEdit, IndexError> {
    let key = crate::bounded_blob(row.get_ref(0).map_err(|error| sql(&error))?, key_bound)?;
    budget.charge(WorkUsage {
        bytes: OrdinalMapEdit::allocation_bytes(key.len())?,
        ..WorkUsage::default()
    })?;
    crate::read_map_edit(row, key_bound, root, budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    struct Ledger {
        live: AtomicU64,
        denied: Option<MemoryLifetime>,
    }
    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
        }
    }
    struct Scope(Arc<Ledger>);
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if bytes == 0 || self.0.denied == Some(lifetime) {
                return Err(IndexError::ResourceLimited);
            }
            let _prior = self.0.live.fetch_add(bytes, Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }
    fn work(
        bytes: u64,
        denied: Option<MemoryLifetime>,
    ) -> Result<(WorkBudget, Arc<Ledger>), IndexError> {
        let ledger = Arc::new(Ledger {
            live: AtomicU64::new(0),
            denied,
        });
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 0,
                bytes,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?,
        );
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        Ok((work, ledger))
    }
    fn fixture(malformed: bool) -> Result<rusqlite::Connection, IndexError> {
        let db = rusqlite::Connection::open_in_memory().map_err(|error| sql(&error))?;
        db.execute_batch("CREATE TABLE edits(k BLOB PRIMARY KEY,p BLOB,n BLOB) WITHOUT ROWID;
            INSERT INTO edits VALUES(x'0061',NULL,x'000101010101010101010101010101010101010101010101010101010101010101');").map_err(|error| sql(&error))?;
        if malformed {
            db.execute_batch("INSERT INTO edits VALUES(x'0062',NULL,zeroblob(31));")
                .map_err(|error| sql(&error))?;
        } else {
            db.execute_batch("INSERT INTO edits SELECT x'0062',p,n FROM edits WHERE k=x'0061';")
                .map_err(|error| sql(&error))?;
        }
        Ok(db)
    }
    #[test]
    fn page_and_shared_position_keep_real_grants_until_last_owner() -> Result<(), IndexError> {
        let db = fixture(false)?;
        let mut statement = db
            .prepare("SELECT k,p,n FROM edits ORDER BY k LIMIT 64")
            .map_err(|error| sql(&error))?;
        let (mut work, ledger) = work(1_000_000, None)?;
        let mut page = MapEditPage::read(&mut statement, &[], 4096, &mut work, |row, budget| {
            read_map_edit_budgeted(row, 1024, 0, budget)
        })?;
        let first = page.pop()?.ok_or(IndexError::ResourceLimited)?;
        let position = first.clone();
        drop(first);
        drop(page);
        if ledger.live.load(Ordering::SeqCst) != OrdinalMapEdit::allocation_bytes(2)? {
            return Err(IndexError::BackendViolation(
                "shared edit lost or duplicated key custody".into(),
            ));
        }
        drop(position);
        if ledger.live.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "edit page leaked a grant".into(),
            ));
        }
        Ok(())
    }
    #[test]
    fn page_and_row_admission_refusal_refunds_every_prior_allocation() -> Result<(), IndexError> {
        for denied in [MemoryLifetime::Temporary, MemoryLifetime::Retained] {
            let db = fixture(false)?;
            let mut statement = db
                .prepare("SELECT k,p,n FROM edits ORDER BY k LIMIT 64")
                .map_err(|error| sql(&error))?;
            let (mut work, ledger) = work(1_000_000, Some(denied))?;
            if !matches!(
                MapEditPage::read(&mut statement, &[], 4096, &mut work, |row, budget| {
                    read_map_edit_budgeted(row, 1024, 0, budget)
                }),
                Err(IndexError::ResourceLimited)
            ) || ledger.live.load(Ordering::SeqCst) != 0
            {
                return Err(IndexError::BackendViolation(
                    "denied edit page retained backing".into(),
                ));
            }
        }
        Ok(())
    }
    #[test]
    fn original_query_fee_and_row_byte_refusal_precede_owned_data() -> Result<(), IndexError> {
        let db = fixture(false)?;
        let mut statement = db
            .prepare("SELECT k,p,n FROM edits ORDER BY k LIMIT 64")
            .map_err(|error| sql(&error))?;
        let (mut work, ledger) = work(4096, None)?;
        if !matches!(
            MapEditPage::read(&mut statement, &[], 4096, &mut work, |row, budget| {
                read_map_edit_budgeted(row, 1024, 0, budget)
            }),
            Err(IndexError::ResourceLimited)
        ) || work.usage().bytes != 4096
            || ledger.live.load(Ordering::SeqCst) != 0
        {
            return Err(IndexError::BackendViolation(
                "query or row funding was waived".into(),
            ));
        }
        Ok(())
    }
    #[test]
    fn later_malformed_row_remains_a_repeated_error_after_valid_predecessor()
    -> Result<(), IndexError> {
        let db = fixture(true)?;
        let mut statement = db
            .prepare("SELECT k,p,n FROM edits ORDER BY k LIMIT 64")
            .map_err(|error| sql(&error))?;
        let (mut work, ledger) = work(1_000_000, None)?;
        let mut page = MapEditPage::read(&mut statement, &[], 4096, &mut work, |row, budget| {
            read_map_edit_budgeted(row, 1024, 0, budget)
        })?;
        let first = page.pop()?.ok_or(IndexError::ResourceLimited)?;
        if first.edit.key.as_slice() != b"\0a" || page.complete() {
            return Err(IndexError::BackendViolation(
                "malformed page hid a row or became EOF".into(),
            ));
        }
        for _attempt in 0..2 {
            if !matches!(page.pop(), Err(IndexError::CorruptObject(_))) {
                return Err(IndexError::BackendViolation(
                    "malformed edit was skipped".into(),
                ));
            }
        }
        drop(first);
        drop(page);
        if ledger.live.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "malformed page leaked a grant".into(),
            ));
        }
        Ok(())
    }
}
