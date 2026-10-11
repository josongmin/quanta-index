//! Private bounded disk sort for one native literal preparation. The catalog
//! owns installed objects; this disposable database never publishes a root.
use quanta_index_core::index::{
    Digest, IndexError, MapEdit, ObjectRef, OrdinalMapEdit, WorkBudget, WorkUsage,
    ports::MapEditCursor,
};
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};

pub(super) struct LiteralEditSpool {
    db: Connection,
    _file: tempfile::NamedTempFile,
}

fn sql(error: &rusqlite::Error) -> IndexError {
    if matches!(error, rusqlite::Error::SqliteFailure(failure, _) if matches!(failure.code, ErrorCode::DiskFull | ErrorCode::OutOfMemory))
    {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("literal scratch: {error}"))
    }
}
fn reference(bytes: Option<Vec<u8>>) -> Result<Option<ObjectRef>, IndexError> {
    bytes
        .map(|bytes| {
            bytes
                .try_into()
                .map(|value| ObjectRef(Digest(value)))
                .map_err(|_error| {
                    IndexError::CorruptObject("literal scratch reference length".into())
                })
        })
        .transpose()
}

impl LiteralEditSpool {
    pub(super) fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let file = tempfile::NamedTempFile::new()
            .map_err(|error| IndexError::Storage(format!("literal scratch create: {error}")))?;
        let db = Connection::open(file.path()).map_err(|error| sql(&error))?;
        db.execute_batch(
            "PRAGMA page_size=4096; PRAGMA cache_size=-2048;
            PRAGMA mmap_size=0; PRAGMA temp_store=FILE; PRAGMA journal_mode=OFF;
            PRAGMA synchronous=OFF; PRAGMA max_page_count=1048576;
            CREATE TABLE edits(surface INTEGER NOT NULL,k BLOB NOT NULL,p BLOB,n BLOB,
                PRIMARY KEY(surface,k)) WITHOUT ROWID;
            CREATE TABLE posting(g BLOB NOT NULL,k BLOB NOT NULL,p BLOB,n BLOB,
                PRIMARY KEY(g,k)) WITHOUT ROWID; BEGIN IMMEDIATE;",
        )
        .map_err(|error| sql(&error))?;
        let page_size: u32 = db
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        let max_pages: u32 = db
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        if page_size != 4096 || max_pages != 1_048_576 {
            return Err(IndexError::Storage(
                "literal scratch page limit was not installed".into(),
            ));
        }
        Ok(Self { db, _file: file })
    }

    pub(super) fn edit(
        &self,
        surface: u8,
        edit: &MapEdit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let inserted = self
            .db
            .execute(
                "INSERT INTO edits(surface,k,p,n) VALUES (?1,?2,?3,?4)",
                params![
                    surface,
                    edit.key,
                    edit.expected.map(|value| (value.0).0.to_vec()),
                    edit.next.map(|value| (value.0).0.to_vec())
                ],
            )
            .map_err(|error| sql(&error))?;
        if inserted != 1 {
            return Err(IndexError::BackendViolation(
                "edit spool insert affected an unexpected row count".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn posting(
        &self,
        gram: [u8; 3],
        edit: &MapEdit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if edit.expected.is_some() == edit.next.is_some() {
            return Err(IndexError::InvalidInput(
                "posting delta must add or remove membership".into(),
            ));
        }
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let inserted = self
            .db
            .execute(
                "INSERT INTO posting(g,k,p,n) VALUES (?1,?2,?3,?4)",
                params![
                    gram.as_slice(),
                    edit.key,
                    edit.expected.map(|value| (value.0).0.to_vec()),
                    edit.next.map(|value| (value.0).0.to_vec())
                ],
            )
            .map_err(|error| sql(&error))?;
        if inserted != 1 {
            return Err(IndexError::BackendViolation(
                "posting spool insert affected an unexpected row count".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn gram_page(
        &self,
        after: Option<[u8; 3]>,
        budget: &mut WorkBudget,
    ) -> Result<Vec<[u8; 3]>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let mut statement = self
            .db
            .prepare(
                "SELECT g FROM posting WHERE (?1 IS NULL OR g>?1)
            GROUP BY g ORDER BY g LIMIT 64",
            )
            .map_err(|error| sql(&error))?;
        let rows = statement
            .query_map(params![after.map(|value| value.to_vec())], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .map_err(|error| sql(&error))?;
        let mut grams = Vec::with_capacity(64);
        for row in rows {
            grams.push(
                row.map_err(|error| sql(&error))?
                    .try_into()
                    .map_err(|_error| {
                        IndexError::CorruptObject("literal scratch gram length".into())
                    })?,
            );
        }
        Ok(grams)
    }

    pub(super) fn cardinality(
        &self,
        gram: [u8; 3],
        prior: u64,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let (add, remove): (i64, i64) = self.db.query_row(
            "SELECT COALESCE(SUM(n IS NOT NULL),0),COALESCE(SUM(p IS NOT NULL),0) FROM posting WHERE g=?1",
            params![gram.as_slice()], |row| Ok((row.get(0)?,row.get(1)?))).map_err(|error| sql(&error))?;
        let add = u64::try_from(add)
            .map_err(|_error| IndexError::CorruptObject("negative posting additions".into()))?;
        let remove = u64::try_from(remove)
            .map_err(|_error| IndexError::CorruptObject("negative posting removals".into()))?;
        prior
            .checked_sub(remove)
            .and_then(|count| count.checked_add(add))
            .ok_or_else(|| {
                IndexError::CorruptObject("posting cardinality overflow or underflow".into())
            })
    }

    pub(super) fn posting_cursor(&self, grams: Vec<[u8; 3]>) -> PostingCursor<'_> {
        PostingCursor {
            spool: self,
            grams,
            slot: 0,
            after: None,
        }
    }

    pub(super) fn top_cursor(&self) -> TopCursor<'_> {
        TopCursor {
            spool: self,
            surface: 0,
            after: None,
        }
    }
}

pub(super) struct PostingCursor<'a> {
    spool: &'a LiteralEditSpool,
    grams: Vec<[u8; 3]>,
    slot: usize,
    after: Option<Vec<u8>>,
}

type EditRow = (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>);

impl MapEditCursor for PostingCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        while let Some(gram) = self.grams.get(self.slot) {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            let row: Option<EditRow> = self.spool.db.query_row(
                "SELECT k,p,n FROM posting WHERE g=?1 AND (?2 IS NULL OR k>?2) ORDER BY k LIMIT 1",
                params![gram.as_slice(), self.after.as_deref()],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|error| sql(&error))?;
            if let Some((key, prior, next)) = row {
                self.after = Some(key.clone());
                return Ok(Some(OrdinalMapEdit {
                    root: u32::try_from(self.slot).map_err(|_error| IndexError::ResourceLimited)?,
                    edit: MapEdit {
                        key,
                        expected: reference(prior)?,
                        next: reference(next)?,
                    },
                }));
            }
            self.slot = self
                .slot
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            self.after = None;
        }
        Ok(None)
    }
}

pub(super) struct TopCursor<'a> {
    spool: &'a LiteralEditSpool,
    surface: u8,
    after: Option<Vec<u8>>,
}

impl MapEditCursor for TopCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        while self.surface < 2 {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            let row: Option<EditRow> = self.spool.db.query_row(
                "SELECT k,p,n FROM edits WHERE surface=?1 AND (?2 IS NULL OR k>?2) ORDER BY k LIMIT 1",
                params![self.surface, self.after.as_deref()],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|error| sql(&error))?;
            if let Some((key, prior, next)) = row {
                self.after = Some(key.clone());
                return Ok(Some(OrdinalMapEdit {
                    root: u32::from(self.surface),
                    edit: MapEdit {
                        key,
                        expected: reference(prior)?,
                        next: reference(next)?,
                    },
                }));
            }
            self.surface = self
                .surface
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            self.after = None;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        time::{Duration, Instant},
    };

    use super::*;
    use quanta_index_core::index::WorkLimits;

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 100_000_000,
                mutations: 100_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn posting_keyset_pages_match_independent_sorted_delta() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let spool = LiteralEditSpool::new(&budget)?;
        let present = ObjectRef(Digest([7; 32]));
        let mut expected = BTreeMap::new();
        // Insert in reverse order; the public cursor must produce canonical
        // gram and owner order across both 64-gram page boundaries.
        for value in (0_u16..130).rev() {
            let [high, low] = value.to_be_bytes();
            let gram = [high, low, 9];
            for owner in (0_u8..3).rev() {
                let edit = MapEdit {
                    key: vec![owner],
                    expected: None,
                    next: Some(present),
                };
                spool.posting(gram, &edit, &mut budget)?;
                assert!(expected.insert((gram, edit.key.clone()), edit).is_none());
            }
        }
        let mut after = None;
        let mut observed = BTreeMap::new();
        let mut page_lengths = Vec::new();
        loop {
            let grams = spool.gram_page(after, &mut budget)?;
            if grams.is_empty() {
                break;
            }
            page_lengths.push(grams.len());
            after = grams.last().copied();
            for gram in &grams {
                assert_eq!(spool.cardinality(*gram, 0, &mut budget)?, 3);
            }
            let mut cursor = spool.posting_cursor(grams.clone());
            while let Some(item) = cursor.next_edit(&mut budget)? {
                let ordinal =
                    usize::try_from(item.root).map_err(|_error| IndexError::ResourceLimited)?;
                let gram = *grams
                    .get(ordinal)
                    .ok_or_else(|| IndexError::BackendViolation("scratch test ordinal".into()))?;
                let key = (gram, item.edit.key.clone());
                assert!(observed.insert(key, item.edit).is_none());
            }
        }
        assert_eq!(page_lengths, vec![64, 64, 2]);
        assert_eq!(observed, expected);
        Ok(())
    }

    #[test]
    fn duplicate_and_cardinality_underflow_are_errors() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let spool = LiteralEditSpool::new(&budget)?;
        let edit = MapEdit {
            key: vec![1],
            expected: Some(ObjectRef(Digest([8; 32]))),
            next: None,
        };
        spool.posting(*b"abc", &edit, &mut budget)?;
        assert!(spool.posting(*b"abc", &edit, &mut budget).is_err());
        assert!(matches!(
            spool.cardinality(*b"abc", 0, &mut budget),
            Err(IndexError::CorruptObject(_))
        ));
        assert_eq!(spool.cardinality(*b"abc", 1, &mut budget)?, 0);
        Ok(())
    }

    #[test]
    fn scratch_exhaustion_is_resource_limited() -> Result<(), IndexError> {
        let budget = budget()?;
        let spool = LiteralEditSpool::new(&budget)?;
        let _limit: u32 = spool
            .db
            .query_row("PRAGMA max_page_count=8", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        let failed = spool
            .db
            .execute(
                "INSERT INTO edits(surface,k,n) VALUES(0,x'01',zeroblob(1048576))",
                [],
            )
            .map_err(|error| sql(&error));
        assert_eq!(failed, Err(IndexError::ResourceLimited));
        Ok(())
    }
}
