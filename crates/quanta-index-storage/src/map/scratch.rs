//! Disposable, private map-build scratch. Its rows never authorize a view.

use quanta_index_core::index::{
    Digest, IndexError, MapEdit, ObjectRef, OrdinalMapEdit, WorkBudget, WorkUsage,
};
use rusqlite::{Connection, OptionalExtension as _, params};
use tempfile::TempDir;

use super::{Node, decode_node};
use crate::{
    FileObjectStore,
    object::{PreparedObject, decode_stored_wire},
};

const SCRATCH_PAGE_COUNT: i64 = 1_048_576; // 4 GiB at a 4096-byte page size.
const NODE_READ_BOUND: u64 = 16 + 17 * 32 + 13 + 8192;
const EDIT_READ_BOUND: u64 = 4096 + 64 + 128;
type EditRow = (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>);

pub(crate) struct MapScratch {
    db: Connection,
    _directory: TempDir,
}

impl MapScratch {
    pub(crate) fn open(
        store: &FileObjectStore,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 8192,
            ..WorkUsage::default()
        })?;
        let root = store.scratch_root()?;
        let directory = tempfile::Builder::new()
            .prefix("map-")
            .tempdir_in(root)
            .map_err(|error| IndexError::Storage(format!("create map scratch: {error}")))?;
        let db = Connection::open(directory.path().join("scratch.sqlite"))
            .map_err(|error| sql_error(&error))?;
        db.execute_batch(&format!(
            "PRAGMA page_size=4096;
             PRAGMA max_page_count={SCRATCH_PAGE_COUNT};
             PRAGMA cache_size=-2048;
             PRAGMA mmap_size=0;
             PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=FILE;
             CREATE TABLE node(reference BLOB PRIMARY KEY NOT NULL, wire BLOB NOT NULL) WITHOUT ROWID;
             CREATE TABLE edit(root INTEGER NOT NULL, key BLOB NOT NULL,
               expected BLOB, next BLOB, PRIMARY KEY(root,key)) WITHOUT ROWID;
             CREATE TABLE mark(reference BLOB PRIMARY KEY NOT NULL, state INTEGER NOT NULL) WITHOUT ROWID;"
        ))
        .map_err(|error| sql_error(&error))?;
        Ok(Self {
            db,
            _directory: directory,
        })
    }

    pub(crate) fn insert_node(
        &self,
        object: &PreparedObject,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            bytes: object.registration.bytes,
            ..WorkUsage::default()
        })?;
        let key = (object.registration.reference.0).0;
        let inserted = self
            .db
            .execute(
                "INSERT OR IGNORE INTO node(reference,wire) VALUES (?1,?2)",
                params![key.as_slice(), object.wire.as_slice()],
            )
            .map_err(|error| sql_error(&error))?;
        if inserted == 0 {
            let existing = self
                .db
                .query_row(
                    "SELECT wire FROM node WHERE reference=?1",
                    params![key.as_slice()],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .map_err(|error| sql_error(&error))?;
            if existing != object.wire {
                return Err(IndexError::CorruptObject(
                    "staged object digest collision".into(),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn prepared(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<PreparedObject>, IndexError> {
        self.wire(reference, budget)?
            .map(|wire| {
                let stored = decode_stored_wire(reference, &wire, budget)?;
                Ok(PreparedObject {
                    registration: stored.registration,
                    wire,
                })
            })
            .transpose()
    }

    fn wire(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<Vec<u8>>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let key = (reference.0).0;
        let length: Option<i64> = self
            .db
            .query_row(
                "SELECT length(wire) FROM node WHERE reference=?1",
                params![key.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql_error(&error))?;
        let Some(length) = length else {
            return Ok(None);
        };
        let length = u64::try_from(length).map_err(|_conversion_error| {
            IndexError::CorruptObject("invalid scratch wire length".into())
        })?;
        if length > NODE_READ_BOUND || length == 0 {
            return Err(IndexError::CorruptObject(
                "scratch map node wire exceeds physical limit".into(),
            ));
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: length,
            ..WorkUsage::default()
        })?;
        let wire: Option<Vec<u8>> = self
            .db
            .query_row(
                "SELECT wire FROM node WHERE reference=?1",
                params![key.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql_error(&error))?;
        let wire = wire.ok_or_else(|| {
            IndexError::CorruptObject("scratch map node changed during read".into())
        })?;
        let actual_length = u64::try_from(wire.len()).map_err(|_conversion_error| {
            IndexError::CorruptObject("scratch map node length overflows".into())
        })?;
        if actual_length != length {
            return Err(IndexError::CorruptObject(
                "scratch map node changed during read".into(),
            ));
        }
        Ok(Some(wire))
    }

    pub(crate) fn read_node(
        &self,
        store: &FileObjectStore,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Node, IndexError> {
        if let Some(wire) = self.wire(reference, budget)? {
            decode_node(decode_stored_wire(reference, &wire, budget)?)
        } else {
            store.read_node(reference, budget)
        }
    }

    pub(crate) fn insert_edit(
        &self,
        ordinal: &OrdinalMapEdit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let expected = ordinal.edit.expected.map(|reference| (reference.0).0);
        let next = ordinal.edit.next.map(|reference| (reference.0).0);
        budget.charge(WorkUsage {
            writes: 1,
            bytes: u64::try_from(ordinal.edit.key.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                .checked_add(EDIT_READ_BOUND)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let inserted = self
            .db
            .execute(
                "INSERT INTO edit(root,key,expected,next) VALUES (?1,?2,?3,?4)",
                params![
                    i64::from(ordinal.root),
                    ordinal.edit.key.as_slice(),
                    expected.as_ref().map(<[u8; 32]>::as_slice),
                    next.as_ref().map(<[u8; 32]>::as_slice)
                ],
            )
            .map_err(|error| sql_error(&error))?;
        if inserted != 1 {
            return Err(IndexError::Storage("scratch edit insertion failed".into()));
        }
        Ok(())
    }

    pub(crate) fn next_edit(
        &self,
        root: u32,
        after: Option<&[u8]>,
        budget: &mut WorkBudget,
    ) -> Result<Option<MapEdit>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: EDIT_READ_BOUND,
            ..WorkUsage::default()
        })?;
        let row: Option<EditRow> = if let Some(after) = after {
            self.db
                .query_row(
                    "SELECT key,expected,next FROM edit WHERE root=?1 AND key>?2 ORDER BY key LIMIT 1",
                    params![i64::from(root), after],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|error| sql_error(&error))?
        } else {
            self.db
                .query_row(
                    "SELECT key,expected,next FROM edit WHERE root=?1 ORDER BY key LIMIT 1",
                    params![i64::from(root)],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(|error| sql_error(&error))?
        };
        row.map(|(key, expected, next)| {
            Ok(MapEdit {
                key,
                expected: decode_optional_ref(expected)?,
                next: decode_optional_ref(next)?,
            })
        })
        .transpose()
    }

    pub(crate) fn mark(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<u8>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let key = (reference.0).0;
        self.db
            .query_row(
                "SELECT state FROM mark WHERE reference=?1",
                params![key.as_slice()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql_error(&error))
    }

    pub(crate) fn set_mark(
        &self,
        reference: ObjectRef,
        state: u8,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            ..WorkUsage::default()
        })?;
        let key = (reference.0).0;
        let _updated = self
            .db
            .execute(
                "INSERT INTO mark(reference,state) VALUES (?1,?2)
                 ON CONFLICT(reference) DO UPDATE SET state=excluded.state",
                params![key.as_slice(), state],
            )
            .map_err(|error| sql_error(&error))?;
        Ok(())
    }
}

fn decode_optional_ref(bytes: Option<Vec<u8>>) -> Result<Option<ObjectRef>, IndexError> {
    bytes
        .map(|bytes| {
            let digest: [u8; 32] = bytes.try_into().map_err(|_bytes| {
                IndexError::CorruptObject("scratch edit reference has wrong length".into())
            })?;
            Ok(ObjectRef(Digest(digest)))
        })
        .transpose()
}

fn sql_error(error: &rusqlite::Error) -> IndexError {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                rusqlite::ErrorCode::DiskFull | rusqlite::ErrorCode::OutOfMemory
            )
    ) {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("map scratch: {error}"))
    }
}
