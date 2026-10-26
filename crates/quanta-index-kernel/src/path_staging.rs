//! Data-only custody for source path rows, pages and temporary lookup keys.

use std::{ops::Deref, sync::Arc};

use quanta_index_core::index::{
    IndexError, MAX_SOURCE_PATH_BYTES, MemoryLease, MemoryLifetime, OwnerKey, WorkBudget,
};

pub(crate) const PAGE_ROWS: usize = 128;

pub(crate) use quanta_index_core::index::SourcePathKey as PathKey;

pub(crate) struct PathRowData {
    pub(crate) path: String,
    pub(crate) leaving: Option<OwnerKey>,
    pub(crate) incoming: Option<OwnerKey>,
    pub(crate) expected: Option<quanta_index_core::index::MapValue>,
    pub(crate) next: Option<quanta_index_core::index::MapValue>,
    _memory: Option<MemoryLease>,
}

#[derive(Clone)]
pub(crate) struct PathRow(Arc<PathRowData>);

impl Deref for PathRow {
    type Target = PathRowData;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PathRow {
    pub(crate) fn copy(
        path: &str,
        leaving: Option<OwnerKey>,
        incoming: Option<OwnerKey>,
        expected: Option<quanta_index_core::index::MapValue>,
        next: Option<quanta_index_core::index::MapValue>,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        if path.len() > MAX_SOURCE_PATH_BYTES {
            return Err(IndexError::CorruptObject(
                "source path row exceeds bound".into(),
            ));
        }
        let bytes = path
            .len()
            .checked_add(std::mem::size_of::<PathRowData>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[usize; 2]>()))
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        let mut copied = String::new();
        copied
            .try_reserve_exact(path.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        copied.push_str(path);
        budget.checkpoint()?;
        Ok(Self(Arc::new(PathRowData {
            path: copied,
            leaving,
            incoming,
            expected,
            next,
            _memory: memory,
        })))
    }
}

#[derive(Default)]
pub(crate) struct PathPage {
    rows: Vec<PathRow>,
    memory: Option<MemoryLease>,
}

impl PathPage {
    pub(crate) fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let bytes = PAGE_ROWS
            .checked_mul(std::mem::size_of::<PathRow>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(PAGE_ROWS)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self { rows, memory })
    }

    pub(crate) fn push(&mut self, row: PathRow) -> Result<(), IndexError> {
        if self.rows.capacity() < PAGE_ROWS {
            return Err(IndexError::BackendViolation(
                "source path page has no admitted backing".into(),
            ));
        }
        if self.rows.len() == PAGE_ROWS {
            return Err(IndexError::BackendViolation(
                "source path page exceeds row bound".into(),
            ));
        }
        self.rows.push(row);
        Ok(())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn as_slice(&self) -> &[PathRow] {
        &self.rows
    }
}

pub(crate) struct PathPageCursor {
    rows: std::vec::IntoIter<PathRow>,
    _memory: Option<MemoryLease>,
}

impl Default for PathPageCursor {
    fn default() -> Self {
        Self {
            rows: Vec::new().into_iter(),
            _memory: None,
        }
    }
}

impl PathPageCursor {
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.as_slice().is_empty()
    }
}

impl Iterator for PathPageCursor {
    type Item = PathRow;

    fn next(&mut self) -> Option<Self::Item> {
        self.rows.next()
    }
}

impl IntoIterator for PathPage {
    type Item = PathRow;
    type IntoIter = PathPageCursor;

    fn into_iter(self) -> Self::IntoIter {
        PathPageCursor {
            rows: self.rows.into_iter(),
            _memory: self.memory,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};

    use super::*;

    pub(crate) struct Ledger {
        cap: u64,
        live: AtomicU64,
        temporary: AtomicU64,
        retained: AtomicU64,
    }

    impl Ledger {
        pub(crate) fn live(&self) -> u64 {
            self.live.load(Ordering::SeqCst)
        }
    }

    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            let _previous = self.live.fetch_sub(bytes, Ordering::SeqCst);
        }
    }

    struct Scope(Arc<Ledger>);

    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            let next = self
                .0
                .live()
                .checked_add(bytes)
                .filter(|next| *next <= self.0.cap)
                .ok_or(IndexError::ResourceLimited)?;
            self.0.live.store(next, Ordering::SeqCst);
            match lifetime {
                MemoryLifetime::Temporary => {
                    let _old = self.0.temporary.fetch_add(1, Ordering::SeqCst);
                }
                MemoryLifetime::Retained => {
                    let _old = self.0.retained.fetch_add(1, Ordering::SeqCst);
                }
            }
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    pub(crate) fn budget(cap: u64) -> Result<(WorkBudget, Arc<Ledger>), IndexError> {
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let ledger = Arc::new(Ledger {
            cap,
            live: AtomicU64::new(0),
            temporary: AtomicU64::new(0),
            retained: AtomicU64::new(0),
        });
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        Ok((work, ledger))
    }

    fn require(value: bool) -> Result<(), IndexError> {
        if value {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "source path custody oracle mismatch".into(),
            ))
        }
    }

    #[test]
    fn row_page_and_lookup_key_refuse_floor_before_backing_birth() -> Result<(), IndexError> {
        let (work, ledger) = budget(0)?;
        require(matches!(
            PathRow::copy("a", None, None, None, None, &work),
            Err(IndexError::ResourceLimited)
        ))?;
        require(matches!(
            PathPage::new(&work),
            Err(IndexError::ResourceLimited)
        ))?;
        require(matches!(
            PathKey::copy("a", &work),
            Err(IndexError::ResourceLimited)
        ))?;
        require(ledger.live() == 0)
    }

    #[test]
    fn shared_row_and_cursor_position_keep_exactly_one_retained_charge_after_eof()
    -> Result<(), IndexError> {
        let (work, ledger) = budget(u64::MAX)?;
        let row = PathRow::copy("a", None, None, None, None, &work)?;
        let bytes = ledger.live();
        let position = row.clone();
        require(Arc::ptr_eq(&row.0, &position.0) && ledger.live() == bytes)?;
        drop(row);
        let mut page = PathPage::new(&work)?;
        page.push(position)?;
        let with_page = ledger.live();
        let mut cursor = page.into_iter();
        let escaped = cursor.next().ok_or(IndexError::Conflict)?;
        require(cursor.next().is_none() && ledger.live() == with_page)?;
        drop(cursor);
        require(escaped.path == "a" && ledger.live() == bytes)?;
        require(
            ledger.retained.load(Ordering::SeqCst) == 1
                && ledger.temporary.load(Ordering::SeqCst) == 1,
        )?;
        drop(escaped);
        require(ledger.live() == 0)
    }

    #[test]
    fn fixed_page_limit_no_unadmitted_default_growth_and_binary_key_gold() -> Result<(), IndexError>
    {
        let (work, ledger) = budget(u64::MAX)?;
        let row = PathRow::copy("a", None, None, None, None, &work)?;
        let mut empty = PathPage::default();
        require(matches!(
            empty.push(row.clone()),
            Err(IndexError::BackendViolation(_))
        ))?;
        let mut page = PathPage::new(&work)?;
        for _entry in 0..128 {
            page.push(row.clone())?;
        }
        require(matches!(
            page.push(row.clone()),
            Err(IndexError::BackendViolation(_))
        ))?;
        require(page.as_slice().len() == 128)?;
        drop(page);
        drop(row);
        require(ledger.live() == 0)?;
        let key = PathKey::copy("é.rs", &work)?;
        require(key.as_slice() == b"path\0\xc3\xa9.rs" && ledger.live() == 10)?;
        drop(key);
        require(ledger.live() == 0)
    }
}
