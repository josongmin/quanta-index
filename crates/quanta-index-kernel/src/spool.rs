//! Attempt-local sorted spill for corpus-sized map edits and work actions.
//! Only final immutable roots and catalog commit have publication authority.
use std::collections::VecDeque;

use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapEdit, MapRoot, ObjectBatchInput, ObjectRef,
    OrdinalMapEdit, WorkAction, WorkActionDigest, WorkActionSet, WorkBudget, WorkUsage,
    ports::MapEditCursor,
};
use rusqlite::{Connection, ErrorCode, params};
use tempfile::NamedTempFile;

const SCRATCH_PAGE_SIZE: u32 = 4096;
const SCRATCH_MAX_PAGES: u32 = 32768;
const WORK_LEAF_BATCH: usize = 64;
const WORK_LEAF_BATCH_BYTES: u64 = 8 * 1024 * 1024;
type WorkScratchRow = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);

fn sql_error(error: &rusqlite::Error) -> IndexError {
    if matches!(error, rusqlite::Error::SqliteFailure(failure, _) if failure.code == ErrorCode::DiskFull)
    {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("kernel attempt scratch: {error}"))
    }
}

fn open(schema: &str) -> Result<(Connection, NamedTempFile), IndexError> {
    let file = NamedTempFile::new()
        .map_err(|error| IndexError::Storage(format!("kernel attempt scratch: {error}")))?;
    let connection = Connection::open(file.path()).map_err(|error| sql_error(&error))?;
    connection
        .execute_batch("PRAGMA page_size=4096; PRAGMA max_page_count=32768; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA temp_store=FILE; PRAGMA mmap_size=0; PRAGMA cache_size=-1024;")
        .map_err(|error| sql_error(&error))?;
    let page_size: u32 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| sql_error(&error))?;
    let max_pages: u32 = connection
        .query_row("PRAGMA max_page_count", [], |row| row.get(0))
        .map_err(|error| sql_error(&error))?;
    if page_size != SCRATCH_PAGE_SIZE || max_pages != SCRATCH_MAX_PAGES {
        return Err(IndexError::Storage(
            "kernel attempt scratch page limit was not installed".into(),
        ));
    }
    connection
        .execute_batch(schema)
        .map_err(|error| sql_error(&error))?;
    Ok((connection, file))
}

fn read_ref(bytes: Option<&[u8]>) -> Result<Option<ObjectRef>, IndexError> {
    bytes
        .map(|value| {
            let digest: [u8; 32] = value
                .try_into()
                .map_err(|_error| IndexError::CorruptObject("scratch reference length".into()))?;
            Ok(ObjectRef(Digest(digest)))
        })
        .transpose()
}

pub(crate) struct EditSpool {
    connection: Connection,
    _file: NamedTempFile,
}

impl EditSpool {
    pub(crate) fn open() -> Result<Self, IndexError> {
        let (connection, file) = open(
            "CREATE TABLE edits(root INTEGER NOT NULL,key BLOB NOT NULL,expected BLOB,next BLOB,PRIMARY KEY(root,key)) WITHOUT ROWID;",
        )?;
        Ok(Self {
            connection,
            _file: file,
        })
    }

    pub(crate) fn push(
        &self,
        root: u32,
        edit: &MapEdit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let bytes = u64::try_from(edit.key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            mutations: 1,
            bytes: bytes.checked_add(64).ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let expected = edit.expected.map(|reference| reference.0.0);
        let next = edit.next.map(|reference| reference.0.0);
        let inserted = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO edits(root,key,expected,next) VALUES(?1,?2,?3,?4)",
                params![
                    root,
                    edit.key,
                    expected.as_ref().map(<[u8; 32]>::as_slice),
                    next.as_ref().map(<[u8; 32]>::as_slice)
                ],
            )
            .map_err(|error| sql_error(&error))?;
        if inserted != 1 {
            return Err(IndexError::InvalidInput("duplicate staged map edit".into()));
        }
        Ok(())
    }

    pub(crate) fn cursor(&self) -> EditSpoolCursor<'_> {
        EditSpoolCursor {
            spool: self,
            after: None,
            page: VecDeque::new(),
            finished: false,
        }
    }

    fn page(&self, after: Option<(u32, &[u8])>) -> Result<Vec<OrdinalMapEdit>, IndexError> {
        let root = after.map_or(0, |(root, _)| root);
        let key = after.map(|(_, key)| key);
        let mut statement = self.connection.prepare(
            "SELECT root,key,expected,next FROM edits WHERE (?1 IS NULL OR root>?2 OR (root=?2 AND key>?1)) ORDER BY root,key LIMIT 128"
        ).map_err(|error| sql_error(&error))?;
        let rows = statement
            .query_map(params![key, root], |row| {
                let ordinal: u32 = row.get(0)?;
                let key: Vec<u8> = row.get(1)?;
                let expected: Option<Vec<u8>> = row.get(2)?;
                let next: Option<Vec<u8>> = row.get(3)?;
                Ok((ordinal, key, expected, next))
            })
            .map_err(|error| sql_error(&error))?;
        rows.map(|row| {
            let (root, key, expected, next) = row.map_err(|error| sql_error(&error))?;
            Ok(OrdinalMapEdit {
                root,
                edit: MapEdit {
                    key,
                    expected: read_ref(expected.as_deref())?,
                    next: read_ref(next.as_deref())?,
                },
            })
        })
        .collect()
    }
}

pub(crate) struct EditSpoolCursor<'a> {
    spool: &'a EditSpool,
    after: Option<(u32, Vec<u8>)>,
    page: VecDeque<OrdinalMapEdit>,
    finished: bool,
}

impl MapEditCursor for EditSpoolCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if self.finished {
            return Ok(None);
        }
        if self.page.is_empty() {
            self.page = self
                .spool
                .page(
                    self.after
                        .as_ref()
                        .map(|(root, key)| (*root, key.as_slice())),
                )?
                .into();
            if self.page.is_empty() {
                self.finished = true;
                return Ok(None);
            }
        }
        let edit = self
            .page
            .pop_front()
            .ok_or_else(|| IndexError::BackendViolation("scratch page disappeared".into()))?;
        self.after = Some((edit.root, edit.edit.key.clone()));
        Ok(Some(edit))
    }
}

pub(crate) struct WorkSpool {
    connection: Connection,
    _file: NamedTempFile,
}

impl WorkSpool {
    pub(crate) fn open() -> Result<Self, IndexError> {
        let (connection, file) = open(
            "CREATE TABLE actions(key BLOB PRIMARY KEY,payload BLOB NOT NULL,reference BLOB) WITHOUT ROWID;",
        )?;
        Ok(Self {
            connection,
            _file: file,
        })
    }

    pub(crate) fn push(
        &self,
        action: &WorkAction,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let key = (*action).key()?;
        let payload = serde_json::to_vec(action)
            .map_err(|error| IndexError::InvalidInput(format!("work action encoding: {error}")))?;
        budget.charge(WorkUsage {
            mutations: 1,
            bytes: u64::try_from(
                key.len()
                    .checked_add(payload.len())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let _inserted = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO actions(key,payload) VALUES(?1,?2)",
                params![key, payload],
            )
            .map_err(|error| sql_error(&error))?;
        Ok(())
    }

    fn page(&self, after: Option<&[u8]>) -> Result<Vec<WorkScratchRow>, IndexError> {
        let mut statement = self.connection.prepare("SELECT key,payload,reference FROM actions WHERE (?1 IS NULL OR key>?1) ORDER BY key LIMIT 128").map_err(|error| sql_error(&error))?;
        let rows = statement
            .query_map(params![after], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|error| sql_error(&error))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| sql_error(&error))
    }

    pub(crate) fn stage(
        &self,
        context: &mut AttemptContext<'_>,
    ) -> Result<WorkActionSet, IndexError> {
        let mut digest = WorkActionDigest::default();
        let mut after = None;
        loop {
            let page = self.page(after.as_deref())?;
            if page.is_empty() {
                break;
            }
            for rows in page.chunks(WORK_LEAF_BATCH) {
                let mut inputs = Vec::with_capacity(rows.len());
                let mut predicted = Vec::with_capacity(rows.len());
                let mut aggregate = 0_u64;
                for (key, payload, _) in rows {
                    let action: WorkAction = serde_json::from_slice(payload).map_err(|error| {
                        IndexError::CorruptObject(format!("staged work action: {error}"))
                    })?;
                    if action.key()? != *key {
                        return Err(IndexError::CorruptObject(
                            "work scratch key mismatch".into(),
                        ));
                    }
                    digest.push(&action)?;
                    let children = action.children();
                    let child_bytes = children
                        .len()
                        .checked_mul(32)
                        .ok_or(IndexError::ResourceLimited)?;
                    let input_bytes = payload
                        .len()
                        .checked_add(child_bytes)
                        .and_then(|bytes| bytes.checked_add(256))
                        .ok_or(IndexError::ResourceLimited)?;
                    aggregate = aggregate
                        .checked_add(
                            u64::try_from(input_bytes)
                                .map_err(|_error| IndexError::ResourceLimited)?,
                        )
                        .ok_or(IndexError::ResourceLimited)?;
                    if aggregate > WORK_LEAF_BATCH_BYTES {
                        return Err(IndexError::ResourceLimited);
                    }
                    predicted.push(
                        context
                            .objects
                            .identify(payload, &children, context.budget)?
                            .reference,
                    );
                    inputs.push(ObjectBatchInput {
                        payload: payload.clone(),
                        children,
                    });
                }
                let installed = context.put_batch(&inputs)?;
                if installed.len() != rows.len() || predicted.len() != rows.len() {
                    return Err(IndexError::BackendViolation(
                        "work leaf batch lost an ordinal result".into(),
                    ));
                }
                for (((key, _, _), reference), expected_identity) in
                    rows.iter().zip(installed).zip(predicted)
                {
                    if reference != expected_identity {
                        return Err(IndexError::BackendViolation(
                            "work leaf batch identity or order mismatch".into(),
                        ));
                    }
                    let changed = self
                        .connection
                        .execute(
                            "UPDATE actions SET reference=?2 WHERE key=?1",
                            params![key, reference.0.0.as_slice()],
                        )
                        .map_err(|error| sql_error(&error))?;
                    if changed != 1 {
                        return Err(IndexError::BackendViolation(
                            "work scratch lost action".into(),
                        ));
                    }
                    after = Some(key.clone());
                }
            }
        }
        if digest.count() == 0 {
            return Ok(WorkActionSet::empty());
        }
        let mut cursor = WorkSpoolMapCursor {
            spool: self,
            page: VecDeque::new(),
            after: None,
            finished: false,
        };
        let roots = context.apply_stream(&[MapRoot(None)], &mut cursor)?;
        let [root]: [MapRoot; 1] = roots
            .try_into()
            .map_err(|_roots| IndexError::BackendViolation("work manifest lost root".into()))?;
        Ok(WorkActionSet {
            root,
            count: digest.count(),
            digest: digest.finish(),
        })
    }
}

struct WorkSpoolMapCursor<'a> {
    spool: &'a WorkSpool,
    page: VecDeque<WorkScratchRow>,
    after: Option<Vec<u8>>,
    finished: bool,
}

impl MapEditCursor for WorkSpoolMapCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if self.finished {
            return Ok(None);
        }
        if self.page.is_empty() {
            self.page = self.spool.page(self.after.as_deref())?.into();
            if self.page.is_empty() {
                self.finished = true;
                return Ok(None);
            }
        }
        let (key, _, reference) = self
            .page
            .pop_front()
            .ok_or_else(|| IndexError::BackendViolation("work scratch page disappeared".into()))?;
        let next = read_ref(reference.as_deref())?
            .ok_or_else(|| IndexError::BackendViolation("work action was not installed".into()))?;
        self.after = Some(key.clone());
        Ok(Some(OrdinalMapEdit {
            root: 0,
            edit: MapEdit {
                key,
                expected: None,
                next: Some(next),
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_uses_file_with_enforced_page_cap() -> Result<(), IndexError> {
        let (connection, _file) = open("CREATE TABLE payloads(value BLOB);")?;
        let temp_store: u32 = connection
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        assert_eq!(temp_store, 1);
        let max_pages: u32 = connection
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        assert_eq!(max_pages, SCRATCH_MAX_PAGES);
        let _changed: u32 = connection
            .query_row("PRAGMA max_page_count=4", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        assert!(matches!(
            connection
                .execute("INSERT INTO payloads(value) VALUES(zeroblob(1048576))", [])
                .map_err(|error| sql_error(&error)),
            Err(IndexError::ResourceLimited)
        ));
        Ok(())
    }
}
