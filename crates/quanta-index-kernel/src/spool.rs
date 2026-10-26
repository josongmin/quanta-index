//! Attempt-local sorted spill for corpus-sized map edits and work actions.
//! Only final immutable roots and catalog commit have publication authority.
use quanta_index_scratch_sqlite::ScratchConnection;
use std::collections::VecDeque;

use quanta_index_core::index::{
    IndexError, MAX_WORK_ACTION_BYTES, MapEdit, MemoryLease, MemoryLifetime, OrdinalMapEdit,
    ScratchAllocation, ScratchRequest, ScratchSpacePort, WorkAction, WorkActionDigest,
    WorkActionSet, WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, MapEditCursor, WorkActionCursor},
};
use rusqlite::params;

const SCRATCH_PAGE_SIZE: u32 = 4096;
const SCRATCH_MAX_BYTES: u64 = 134_217_728;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;
const EDIT_PAGE_START_SQL: &str =
    "SELECT root,key,expected,next FROM edits ORDER BY root,key LIMIT 128";
const EDIT_PAGE_AFTER_SQL: &str =
    "SELECT root,key,expected,next FROM edits WHERE (root,key)>(?1,?2) ORDER BY root,key LIMIT 128";
const WORK_PAGE_START_SQL: &str = "SELECT key,payload FROM actions ORDER BY key LIMIT 128";
const WORK_PAGE_AFTER_SQL: &str =
    "SELECT key,payload FROM actions WHERE key>?1 ORDER BY key LIMIT 128";
const WORK_INSERT_PREFIX: &str = "INSERT OR IGNORE INTO actions(key,payload) VALUES ";
struct WorkScratchRow {
    key: [u8; 33],
    payload: crate::staging_payload::StagedPayload,
}

#[derive(Clone, Copy)]
struct BorrowedWorkRow<'a> {
    key: [u8; 33],
    payload: &'a [u8],
}

#[derive(Default)]
struct WorkPage {
    rows: Vec<WorkScratchRow>,
    _memory: Option<MemoryLease>,
}

fn sql_error(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "kernel attempt scratch")
}

fn open(
    schema: &str,
    scratch: &dyn ScratchSpacePort,
    budget: &mut WorkBudget,
) -> Result<(ScratchConnection, Box<dyn ScratchAllocation>), IndexError> {
    let allocation = scratch.allocate(
        ScratchRequest {
            initial_bytes: SCRATCH_INITIAL_BYTES,
            max_bytes: SCRATCH_MAX_BYTES,
        },
        budget,
    )?;
    if allocation.max_bytes() != SCRATCH_MAX_BYTES {
        return Err(IndexError::BackendViolation(
            "kernel scratch allocation changed its envelope".into(),
        ));
    }
    let owned_connection =
        ScratchConnection::open(&allocation.path().join("spool.sqlite"), allocation.as_ref())?;
    let connection = owned_connection.scoped()?;
    connection
        .execute_batch("PRAGMA page_size=4096; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA temp_store=FILE; PRAGMA mmap_size=0; PRAGMA cache_size=-1024;")
        .map_err(|error| sql_error(&error))?;
    quanta_index_scratch_sqlite::configure(&connection, allocation.as_ref())?;
    let page_size: u32 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(|error| sql_error(&error))?;
    let max_pages: u32 = connection
        .query_row("PRAGMA max_page_count", [], |row| row.get(0))
        .map_err(|error| sql_error(&error))?;
    if page_size != SCRATCH_PAGE_SIZE
        || u64::from(max_pages)
            .checked_mul(u64::from(page_size))
            .ok_or(IndexError::ResourceLimited)?
            > allocation.reserved_bytes()?
    {
        return Err(IndexError::Storage(
            "kernel attempt scratch page limit was not installed".into(),
        ));
    }
    quanta_index_scratch_sqlite::reserve_for_write(&connection, allocation.as_ref(), 8192, budget)?;
    connection
        .execute_batch(schema)
        .map_err(|error| sql_error(&error))?;
    drop(connection);
    Ok((owned_connection, allocation))
}

#[derive(Default)]
struct EditPage {
    rows: VecDeque<OrdinalMapEdit>,
    _memory: Option<MemoryLease>,
}
impl EditPage {
    fn collect(
        mut rows: impl Iterator<Item = Result<OrdinalMapEdit, IndexError>>,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        let Some(first) = rows.next().transpose()? else {
            return Ok(Self::default());
        };
        let bytes = 128usize
            .checked_mul(std::mem::size_of::<OrdinalMapEdit>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        let mut values = VecDeque::new();
        values
            .try_reserve_exact(128)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for row in std::iter::once(Ok(first)).chain(rows) {
            if values.len() == 128 {
                return Err(IndexError::BackendViolation(
                    "scratch edit page exceeded its declared window".into(),
                ));
            }
            values.push_back(row?);
        }
        Ok(Self {
            rows: values,
            _memory: memory,
        })
    }
    fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    fn pop_front(&mut self) -> Option<OrdinalMapEdit> {
        self.rows.pop_front()
    }
}

pub(crate) struct EditSpool {
    connection: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
}

impl EditSpool {
    pub(crate) fn open(
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let (connection, allocation) = open(
            "CREATE TABLE edits(root INTEGER NOT NULL,key BLOB NOT NULL,expected BLOB,next BLOB,PRIMARY KEY(root,key)) WITHOUT ROWID;",
            scratch,
            budget,
        )?;
        Ok(Self {
            connection,
            allocation,
        })
    }

    pub(crate) fn push(
        &self,
        root: u32,
        edit: &MapEdit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.push_value_key(root, &edit.key, edit.expected, edit.next, budget)
    }

    pub(crate) fn push_value_key(
        &self,
        root: u32,
        key: &[u8],
        expected: Option<quanta_index_core::index::MapValue>,
        next: Option<quanta_index_core::index::MapValue>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let fields = u64::try_from(quanta_index_core::index::MAX_MAP_VALUE_WIRE_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(64))
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            mutations: 1,
            bytes: bytes
                .checked_add(fields)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.connection.scoped()?,
            self.allocation.as_ref(),
            bytes
                .checked_add(fields)
                .ok_or(IndexError::ResourceLimited)?,
            budget,
        )?;
        let expected = expected
            .as_ref()
            .map(quanta_index_scratch_sqlite::map_value)
            .transpose()?;
        let next = next
            .as_ref()
            .map(quanta_index_scratch_sqlite::map_value)
            .transpose()?;
        let inserted = self
            .connection
            .scoped()?
            .execute(
                "INSERT OR IGNORE INTO edits(root,key,expected,next) VALUES(?1,?2,?3,?4)",
                params![root, key, expected, next],
            )
            .map_err(|error| sql_error(&error))?;
        if inserted != 1 {
            return Err(IndexError::InvalidInput("duplicate staged map edit".into()));
        }
        Ok(())
    }

    /// Reuse the bounded shared SQL writer for already validated coordinator
    /// edits. Preserve the scalar path's original logical admission as well.
    pub(crate) fn push_batch(
        &self,
        edits: &[OrdinalMapEdit],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if edits.is_empty() || edits.len() > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            return Err(IndexError::ResourceLimited);
        }
        let fields = u64::try_from(quanta_index_core::index::MAX_MAP_VALUE_WIRE_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(64))
            .ok_or(IndexError::ResourceLimited)?;
        let mut reservations = [0u64; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
        for (slot, edit) in edits.iter().enumerate() {
            let bytes = u64::try_from(edit.edit.key.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(fields)
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                mutations: 1,
                bytes,
                ..WorkUsage::default()
            })?;
            *reservations
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)? = bytes;
        }
        let db = self.connection.scoped()?;
        quanta_index_scratch_sqlite::write_map_edit_batch(
            &db,
            self.allocation.as_ref(),
            "INSERT OR IGNORE INTO edits(root,key,expected,next) VALUES ",
            4096,
            reservations
                .get(..edits.len())
                .ok_or(IndexError::ResourceLimited)?,
            |slot| {
                let edit = edits.get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((
                    rusqlite::types::ValueRef::Integer(i64::from(edit.root)),
                    &edit.edit,
                    0,
                ))
            },
            budget,
        )
    }

    pub(crate) fn cursor(&self) -> EditSpoolCursor<'_> {
        EditSpoolCursor {
            spool: self,
            after: None,
            page: EditPage::default(),
            finished: false,
        }
    }

    fn page(
        &self,
        after: Option<(u32, &[u8])>,
        budget: &WorkBudget,
    ) -> Result<EditPage, IndexError> {
        let io_scope = self.connection.scoped()?;
        let query = if after.is_some() {
            EDIT_PAGE_AFTER_SQL
        } else {
            EDIT_PAGE_START_SQL
        };
        let mut statement = io_scope.prepare(query).map_err(|error| sql_error(&error))?;
        let decode = |row: &rusqlite::Row<'_>| {
            let root: u32 = row.get(0)?;
            Ok((|| {
                let key = quanta_index_scratch_sqlite::bounded_blob(
                    row.get_ref(1).map_err(|error| sql_error(&error))?,
                    4096,
                )?;
                let expected = quanta_index_scratch_sqlite::optional_map_value(
                    row.get_ref(2).map_err(|error| sql_error(&error))?,
                )?;
                let next = quanta_index_scratch_sqlite::optional_map_value(
                    row.get_ref(3).map_err(|error| sql_error(&error))?,
                )?;
                OrdinalMapEdit::copy_key(root, key, expected, next, budget)
            })())
        };
        let rows = match after {
            Some((root, key)) => statement.query_map(params![root, key], decode),
            None => statement.query_map([], decode),
        }
        .map_err(|error| sql_error(&error))?;
        EditPage::collect(
            rows.map(|row| row.map_err(|error| sql_error(&error))?),
            budget,
        )
    }
}

pub(crate) struct EditSpoolCursor<'a> {
    spool: &'a EditSpool,
    after: Option<OrdinalMapEdit>,
    page: EditPage,
    finished: bool,
}

impl MapEditCursor for EditSpoolCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if self.finished {
            return Ok(None);
        }
        if self.page.is_empty() {
            self.page = self.spool.page(
                self.after
                    .as_ref()
                    .map(|position| (position.root, position.edit.key.as_slice())),
                budget,
            )?;
            if self.page.is_empty() {
                self.finished = true;
                return Ok(None);
            }
        }
        let edit = self
            .page
            .pop_front()
            .ok_or_else(|| IndexError::BackendViolation("scratch page disappeared".into()))?;
        self.after = Some(edit.clone());
        Ok(Some(edit))
    }
}

pub(crate) struct WorkSpool {
    connection: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
}

/// One admitted prepare window, flushed before sealing the work manifest.
#[derive(Default)]
pub(crate) struct WorkActionBatch {
    actions: Vec<WorkAction>,
    memory: Option<MemoryLease>,
}

impl WorkActionBatch {
    pub(crate) fn push(
        &mut self,
        action: &WorkAction,
        spool: &mut Option<WorkSpool>,
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.actions.capacity() == 0 {
            let bytes = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
                .checked_mul(std::mem::size_of::<WorkAction>())
                .ok_or(IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(
                u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            self.actions
                .try_reserve_exact(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS)
                .map_err(|_allocation| IndexError::ResourceLimited)?;
            self.memory = memory;
        }
        if self.actions.len() >= quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            return Err(IndexError::BackendViolation(
                "unflushed work action batch".into(),
            ));
        }
        self.actions.push(*action);
        if self.actions.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            self.flush(spool, scratch, budget)?;
        }
        Ok(())
    }

    pub(crate) fn flush(
        &mut self,
        spool: &mut Option<WorkSpool>,
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.actions.is_empty() {
            return Ok(());
        }
        if spool.is_none() {
            *spool = Some(WorkSpool::open(scratch, budget)?);
        }
        spool
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("work spool missing".into()))?
            .push_batch(&self.actions, budget)?;
        self.actions.clear();
        Ok(())
    }
}

impl WorkSpool {
    pub(crate) fn seal_lazy(
        slot: Option<Self>,
        objects: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<(WorkActionSet, WorkSpoolCursor), IndexError> {
        budget.checkpoint()?;
        let mut digest = WorkActionDigest::default();
        if let Some(spool) = slot.as_ref() {
            let mut after = None;
            loop {
                let page = spool.page(after.as_ref().map(<[u8; 33]>::as_slice), budget)?;
                if page.rows.is_empty() {
                    break;
                }
                for row in &page.rows {
                    let action = Self::decode_row(row, budget)?;
                    digest.push(&action)?;
                    // The removed work leaf installation used to check these
                    // physical dependencies. Verify them before Catalog takes
                    // the publication lock; retirement has no current children.
                    for child in action.children().as_slice() {
                        let stored = objects.get(*child, budget)?;
                        if stored.registration.reference != *child {
                            return Err(IndexError::CorruptObject(
                                "work action child reference mismatch".into(),
                            ));
                        }
                    }
                    after = Some(row.key);
                }
            }
        }
        let sealed = WorkActionSet {
            count: digest.count(),
            digest: digest.finish(),
        };
        Ok((sealed, WorkSpoolCursor::new(slot, sealed)))
    }

    pub(crate) fn open(
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let (connection, allocation) = open(
            "CREATE TABLE actions(key BLOB PRIMARY KEY,payload BLOB NOT NULL) WITHOUT ROWID;",
            scratch,
            budget,
        )?;
        Ok(Self {
            connection,
            allocation,
        })
    }

    pub(crate) fn push(
        &self,
        action: &WorkAction,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let row = Self::encode_action(action, budget)?;
        let db = self.connection.scoped()?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &db,
            self.allocation.as_ref(),
            Self::row_reservation(&row)?,
            budget,
        )?;
        let _inserted = db
            .prepare_cached("INSERT OR IGNORE INTO actions(key,payload) VALUES(?1,?2)")
            .map_err(|error| sql_error(&error))?
            .execute(params![row.key.as_slice(), row.payload.bytes.as_slice()])
            .map_err(|error| sql_error(&error))?;
        Ok(())
    }

    fn encode_action(
        action: &WorkAction,
        budget: &mut WorkBudget,
    ) -> Result<WorkScratchRow, IndexError> {
        let key = (*action).key()?;
        let payload = crate::staging_payload::encode(action, MAX_WORK_ACTION_BYTES, budget)?;
        let incoming = u64::try_from(
            key.len()
                .checked_add(payload.bytes.len())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            mutations: 1,
            bytes: incoming,
            ..WorkUsage::default()
        })?;
        Ok(WorkScratchRow { key, payload })
    }

    fn row_reservation(row: &WorkScratchRow) -> Result<u64, IndexError> {
        let bytes = row
            .key
            .len()
            .checked_add(row.payload.bytes.len())
            .and_then(|bytes| bytes.checked_add(64))
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)
    }

    pub(crate) fn push_batch(
        &self,
        actions: &[WorkAction],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let count = actions.len();
        if count == 0 || count > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            return Err(IndexError::ResourceLimited);
        }
        if let [action] = actions {
            return self.push(action, budget);
        }
        let bytes = count
            .checked_mul(std::mem::size_of::<WorkScratchRow>())
            .ok_or(IndexError::ResourceLimited)?;
        let rows_memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(count)
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        let mut reservations = [0_u64; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
        for (slot, action) in actions.iter().enumerate() {
            let row = Self::encode_action(action, budget)?;
            *reservations
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)? = Self::row_reservation(&row)?;
            rows.push(row);
        }
        let query_bytes = count
            .checked_mul("(?,?)".len())
            .and_then(|bytes| bytes.checked_add(count.checked_sub(1)?))
            .and_then(|bytes| bytes.checked_add(WORK_INSERT_PREFIX.len()))
            .ok_or(IndexError::ResourceLimited)?;
        let query_memory = budget.reserve_memory(
            u64::try_from(query_bytes).map_err(|_width| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut query = String::new();
        query
            .try_reserve_exact(query_bytes)
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        query.push_str(WORK_INSERT_PREFIX);
        for slot in 0..count {
            if slot != 0 {
                query.push(',');
            }
            query.push_str("(?,?)");
        }
        let db = self.connection.scoped()?;
        quanta_index_scratch_sqlite::reserve_for_write_batch(
            &db,
            self.allocation.as_ref(),
            reservations
                .get(..count)
                .ok_or(IndexError::ResourceLimited)?,
            budget,
        )?;
        let mut statement = db
            .prepare_cached(&query)
            .map_err(|error| sql_error(&error))?;
        for (slot, row) in rows.iter().enumerate() {
            budget.checkpoint()?;
            let key = slot
                .checked_mul(2)
                .and_then(|value| value.checked_add(1))
                .ok_or(IndexError::ResourceLimited)?;
            let payload = key.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            statement
                .raw_bind_parameter(key, row.key.as_slice())
                .map_err(|error| sql_error(&error))?;
            statement
                .raw_bind_parameter(payload, row.payload.bytes.as_slice())
                .map_err(|error| sql_error(&error))?;
        }
        budget.checkpoint()?;
        let _inserted = statement.raw_execute().map_err(|error| sql_error(&error))?;
        drop(statement);
        drop(db);
        drop(query);
        drop(query_memory);
        drop(rows);
        drop(rows_memory);
        budget.checkpoint()?;
        Ok(())
    }

    fn page(&self, after: Option<&[u8]>, budget: &mut WorkBudget) -> Result<WorkPage, IndexError> {
        let io_scope = self.connection.scoped()?;
        let query = if after.is_some() {
            WORK_PAGE_AFTER_SQL
        } else {
            WORK_PAGE_START_SQL
        };
        let mut statement = io_scope.prepare(query).map_err(|error| sql_error(&error))?;
        let mut cursor = match after {
            Some(key) => statement.query(params![key]),
            None => statement.query([]),
        }
        .map_err(|error| sql_error(&error))?;
        let Some(first) = cursor.next().map_err(|error| sql_error(&error))? else {
            budget.checkpoint()?;
            return Ok(WorkPage::default());
        };
        let first = Self::borrow_row(first)?;
        let bytes = 128_usize
            .checked_mul(std::mem::size_of::<WorkScratchRow>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(128)
            .map_err(|_error| IndexError::ResourceLimited)?;
        rows.push(Self::copy_row(first, budget)?);
        while let Some(row) = cursor.next().map_err(|error| sql_error(&error))? {
            if rows.len() == 128 {
                return Err(IndexError::BackendViolation(
                    "work page exceeds admitted row count".into(),
                ));
            }
            rows.push(Self::copy_row(Self::borrow_row(row)?, budget)?);
        }
        Ok(WorkPage {
            rows,
            _memory: memory,
        })
    }

    fn borrow_row<'a>(row: &'a rusqlite::Row<'_>) -> Result<BorrowedWorkRow<'a>, IndexError> {
        let key = quanta_index_scratch_sqlite::fixed_blob::<33>(
            row.get_ref(0).map_err(|error| sql_error(&error))?,
        )?;
        let payload = quanta_index_scratch_sqlite::bounded_blob(
            row.get_ref(1).map_err(|error| sql_error(&error))?,
            MAX_WORK_ACTION_BYTES,
        )?;
        Ok(BorrowedWorkRow { key, payload })
    }

    fn copy_row(
        row: BorrowedWorkRow<'_>,
        budget: &mut WorkBudget,
    ) -> Result<WorkScratchRow, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: u64::try_from(row.payload.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(65)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let payload = crate::staging_payload::StagedPayload::copy(row.payload, budget)?;
        Ok(WorkScratchRow {
            key: row.key,
            payload,
        })
    }
    fn decode_row(row: &WorkScratchRow, budget: &WorkBudget) -> Result<WorkAction, IndexError> {
        let action = WorkAction::decode_canonical(&row.payload.bytes, budget)?;
        if action.key()? != row.key {
            return Err(IndexError::CorruptObject(
                "work scratch key mismatch".into(),
            ));
        }
        Ok(action)
    }
}

pub(crate) struct WorkSpoolCursor {
    spool: Option<WorkSpool>,
    page: WorkPage,
    offset: usize,
    after: Option<[u8; 33]>,
    digest: WorkActionDigest,
    expected: WorkActionSet,
    finished: bool,
    failure: Option<IndexError>,
}

impl WorkSpoolCursor {
    fn new(spool: Option<WorkSpool>, expected: WorkActionSet) -> Self {
        Self {
            spool,
            page: WorkPage::default(),
            offset: 0,
            after: None,
            digest: WorkActionDigest::default(),
            expected,
            finished: false,
            failure: None,
        }
    }

    fn advance(&mut self, budget: &mut WorkBudget) -> Result<Option<WorkAction>, IndexError> {
        budget.checkpoint()?;
        if self.finished {
            return Ok(None);
        }
        if self.offset == self.page.rows.len() {
            // Release the completed page before admitting the next bounded page.
            self.page = WorkPage::default();
            self.page = match self.spool.as_ref() {
                Some(spool) => spool.page(self.after.as_ref().map(<[u8; 33]>::as_slice), budget)?,
                None => WorkPage::default(),
            };
            self.offset = 0;
        }
        let Some(row) = self.page.rows.get(self.offset) else {
            if self.digest.count() != self.expected.count
                || self.digest.finish() != self.expected.digest
            {
                return Err(IndexError::CorruptObject(
                    "work scratch changed after sealing".into(),
                ));
            }
            self.finished = true;
            return Ok(None);
        };
        let action = WorkSpool::decode_row(row, budget)?;
        self.digest.push(&action)?;
        if self.digest.count() > self.expected.count {
            return Err(IndexError::CorruptObject(
                "work scratch exceeded sealed action count".into(),
            ));
        }
        self.after = Some(row.key);
        self.offset = self
            .offset
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Some(action))
    }
}

impl WorkActionCursor for WorkSpoolCursor {
    fn next_action(&mut self, budget: &mut WorkBudget) -> Result<Option<WorkAction>, IndexError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        match self.advance(budget) {
            Ok(action) => Ok(action),
            Err(error) => {
                self.failure = Some(error.clone());
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::StatementStatus;
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    use quanta_index_core::index::{
        CollectionId, Digest, IndexInstanceId, MapRoot, MemoryLeaseOwner, MemoryScope,
        MemoryScopePort, ObjectRef, ProjectionId, SlotId, SourceCut, TenantId, WorkIntent,
        WorkLimits, WorkScope, WorkspaceKey, WorktreeId,
    };
    use quanta_index_storage::{FileObjectStore, FileScratchSpace};
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    use super::*;

    struct Ledger {
        cap: u64,
        live: AtomicU64,
        peak: AtomicU64,
    }
    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
        }
    }
    struct Scope(Arc<Ledger>);
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Temporary {
                return Err(IndexError::BackendViolation(
                    "work page retained temporary data".into(),
                ));
            }
            let next = self
                .0
                .live
                .load(Ordering::SeqCst)
                .checked_add(bytes)
                .ok_or(IndexError::ResourceLimited)?;
            if next > self.0.cap {
                return Err(IndexError::ResourceLimited);
            }
            self.0.live.store(next, Ordering::SeqCst);
            let _prior = self.0.peak.fetch_max(next, Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }
    fn work_budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 500,
                writes: 500,
                bytes: 4_000_000,
                mutations: 500,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }
    fn bind(budget: &mut WorkBudget, cap: u64) -> Arc<Ledger> {
        let ledger = Arc::new(Ledger {
            cap,
            live: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        });
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        ledger
    }
    fn require(value: bool) -> Result<(), IndexError> {
        if value {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "work scratch admission oracle mismatch".into(),
            ))
        }
    }

    fn action(number: u8) -> WorkAction {
        WorkAction::Retire(WorkIntent {
            workspace: WorkspaceKey {
                tenant: TenantId([0; 16]),
                collection: CollectionId([0; 16]),
                worktree: WorktreeId([0; 16]),
                slot: SlotId([0; 16]),
            },
            epoch: u64::from(number),
            instance: IndexInstanceId([number; 16]),
            scope: WorkScope::ProjectionDomain {
                projection: ProjectionId([0; 16]),
                source: SourceCut {
                    owners: MapRoot(None),
                    paths: MapRoot(None),
                    frontiers: MapRoot(None),
                },
                producer_policy: Digest([0; 32]),
                dependency_context: Digest([0; 32]),
            },
        })
    }

    fn create_with_child(child: ObjectRef) -> Result<WorkAction, IndexError> {
        let WorkAction::Retire(mut intent) = action(1) else {
            return Err(IndexError::BackendViolation(
                "test action is not a retirement".into(),
            ));
        };
        intent.scope = WorkScope::ProjectionDomain {
            projection: ProjectionId([0; 16]),
            source: SourceCut {
                owners: MapRoot(Some(child)),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            producer_policy: Digest([0; 32]),
            dependency_context: Digest([0; 32]),
        };
        Ok(WorkAction::Create(intent))
    }

    #[test]
    fn work_batch_preserves_scalar_commitment_and_logical_costs()
    -> Result<(), Box<dyn std::error::Error>> {
        for count in [64_u8, 65, 129] {
            let home = tempfile::tempdir()?;
            let space =
                FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
            let objects = FileObjectStore::open(home.path().join("objects"))?;
            let mut scalar_budget = work_budget()?;
            let mut batch_budget = work_budget()?;
            let scalar = WorkSpool::open(&space, &mut scalar_budget)?;
            let batched = WorkSpool::open(&space, &mut batch_budget)?;
            let mut input: Vec<_> = (0..count).map(action).collect();
            // Duplicates within and across the 64-row boundary retain one key.
            input.push(action(0));
            input.push(action(1));
            for item in &input {
                scalar.push(item, &mut scalar_budget)?;
            }
            for items in input.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
                batched.push_batch(items, &mut batch_budget)?;
            }
            require(scalar_budget.usage() == batch_budget.usage())?;
            let (scalar_set, mut scalar_cursor) =
                WorkSpool::seal_lazy(Some(scalar), &objects, &mut scalar_budget)?;
            let (batch_set, mut batch_cursor) =
                WorkSpool::seal_lazy(Some(batched), &objects, &mut batch_budget)?;
            let mut expected = (0..count)
                .map(action)
                .map(|item| item.key().map(|key| (key, item)))
                .collect::<Result<Vec<_>, IndexError>>()?;
            expected.sort_by_key(|(key, _item)| *key);
            let mut gold = WorkActionDigest::default();
            for (_key, item) in expected {
                gold.push(&item)?;
                require(scalar_cursor.next_action(&mut scalar_budget)? == Some(item))?;
                require(batch_cursor.next_action(&mut batch_budget)? == Some(item))?;
            }
            require(scalar_set.count == u64::from(count) && batch_set.count == u64::from(count))?;
            require(scalar_set.digest == batch_set.digest && batch_set.digest == gold.finish())?;
            require(scalar_cursor.next_action(&mut scalar_budget)?.is_none())?;
            require(batch_cursor.next_action(&mut batch_budget)?.is_none())?;
        }
        Ok(())
    }

    #[test]
    fn work_batch_producer_flushes_64_and_tail_with_one_sql_for_the_full_window()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let mut work = work_budget()?;
        let ledger = bind(&mut work, 1_000_000);
        let mut spool = None;
        let mut batch = WorkActionBatch::default();
        for number in 0..63 {
            batch.push(&action(number), &mut spool, &space, &mut work)?;
        }
        require(spool.is_none())?;
        batch.push(&action(63), &mut spool, &space, &mut work)?;
        batch.push(&action(64), &mut spool, &space, &mut work)?;
        let db = spool
            .as_ref()
            .ok_or("missing flushed work spool")?
            .connection
            .scoped()?;
        let count: i64 = db.query_row("SELECT count(*) FROM actions", [], |row| row.get(0))?;
        require(count == 64)?;
        let query = format!(
            "INSERT OR IGNORE INTO actions(key,payload) VALUES {}",
            vec!["(?,?)"; 64].join(",")
        );
        let statement = db.prepare_cached(&query)?;
        require(statement.get_status(StatementStatus::Run) == 1)?;
        drop(statement);
        drop(db);
        batch.flush(&mut spool, &space, &mut work)?;
        batch.flush(&mut spool, &space, &mut work)?;
        let objects = FileObjectStore::open(home.path().join("objects"))?;
        let (sealed, mut cursor) = WorkSpool::seal_lazy(spool, &objects, &mut work)?;
        require(sealed.count == 65)?;
        let mut expected = (0..65)
            .map(action)
            .map(|item| item.key().map(|key| (key, item)))
            .collect::<Result<Vec<_>, IndexError>>()?;
        expected.sort_by_key(|(key, _item)| *key);
        for (_key, item) in expected {
            require(cursor.next_action(&mut work)? == Some(item))?;
        }
        require(cursor.next_action(&mut work)?.is_none())?;
        drop(cursor);
        drop(batch);
        require(ledger.live.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }

    #[test]
    fn work_batch_rejects_bounds_and_memory_without_inserting_rows()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let mut work = work_budget()?;
        let spool = WorkSpool::open(&space, &mut work)?;
        let actions: Vec<_> = (0..65).map(action).collect();
        require(matches!(
            spool.push_batch(&[], &mut work),
            Err(IndexError::ResourceLimited)
        ))?;
        require(matches!(
            spool.push_batch(&actions, &mut work),
            Err(IndexError::ResourceLimited)
        ))?;
        let ledger = bind(&mut work, 0);
        let bounded = actions.get(..64).ok_or(IndexError::ResourceLimited)?;
        require(matches!(
            spool.push_batch(bounded, &mut work),
            Err(IndexError::ResourceLimited)
        ))?;
        require(ledger.live.load(Ordering::SeqCst) == 0)?;
        let mut unopened = None;
        let mut batch = WorkActionBatch::default();
        require(matches!(
            batch.push(&action(0), &mut unopened, &space, &mut work),
            Err(IndexError::ResourceLimited)
        ))?;
        require(unopened.is_none() && ledger.live.load(Ordering::SeqCst) == 0)?;
        let count: i64 =
            spool
                .connection
                .scoped()?
                .query_row("SELECT count(*) FROM actions", [], |row| row.get(0))?;
        require(count == 0)?;
        Ok(())
    }

    #[test]
    fn sealing_create_checks_existing_missing_and_corrupt_physical_child()
    -> Result<(), Box<dyn std::error::Error>> {
        for state in ["present", "missing", "corrupt"] {
            let home = tempfile::tempdir()?;
            let space =
                FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
            let objects = FileObjectStore::open(home.path().join("objects"))?;
            let mut work = work_budget()?;
            let child = if state == "missing" {
                ObjectRef(Digest([99; 32]))
            } else {
                objects.put(b"physical child", &[], &mut work)?.reference
            };
            if state == "corrupt" {
                let mut name = String::new();
                for byte in child.0.0 {
                    write!(&mut name, "{byte:02x}")?;
                }
                let path = home
                    .path()
                    .join("objects")
                    .join("objects")
                    .join(format!("{:02x}", child.0.0[0]))
                    .join(name);
                std::fs::write(path, b"corrupt")?;
            }
            let spool = WorkSpool::open(&space, &mut work)?;
            let item = create_with_child(child)?;
            spool.push(&item, &mut work)?;
            let result = WorkSpool::seal_lazy(Some(spool), &objects, &mut work);
            if state == "present" {
                let (sealed, mut cursor) = result?;
                require(sealed.count == 1 && cursor.next_action(&mut work)? == Some(item))?;
                require(cursor.next_action(&mut work)?.is_none())?;
            } else {
                require(matches!(result, Err(IndexError::CorruptObject(_))))?;
            }
        }
        Ok(())
    }

    #[test]
    fn sealed_work_cursor_streams_65_and_129_rows_with_bounded_pages()
    -> Result<(), Box<dyn std::error::Error>> {
        for count in [65_u8, 129_u8] {
            let home = tempfile::tempdir()?;
            let space =
                FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
            let objects = FileObjectStore::open(home.path().join("objects"))?;
            let mut work = work_budget()?;
            let spool = WorkSpool::open(&space, &mut work)?;
            let mut expected = Vec::new();
            for number in 0..count {
                let item = action(number);
                spool.push(&item, &mut work)?;
                expected.push(item);
            }
            expected.sort_by_key(|item| item.key().expect("valid fixed identity"));
            let (page_bytes, largest_payload): (i64, i64) = spool.connection.scoped()?.query_row(
                "SELECT COALESCE(sum(length(payload)),0),COALESCE(max(length(payload)),0) FROM (SELECT payload FROM actions ORDER BY key LIMIT 128)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let page_bytes = u64::try_from(page_bytes)?;
            let largest_payload = u64::try_from(largest_payload)?;
            let decode_memory = largest_payload
                .checked_mul(8)
                .and_then(|bytes| bytes.checked_add(512))
                .ok_or(IndexError::ResourceLimited)?;
            let cap = u64::try_from(128 * std::mem::size_of::<WorkScratchRow>())?
                .checked_add(page_bytes)
                .and_then(|bytes| bytes.checked_add(decode_memory))
                .ok_or(IndexError::ResourceLimited)?;
            let ledger = bind(&mut work, cap);
            let (sealed, mut cursor) = WorkSpool::seal_lazy(Some(spool), &objects, &mut work)?;
            let mut digest = WorkActionDigest::default();
            for item in &expected {
                digest.push(item)?;
            }
            require(sealed.count == u64::from(count) && sealed.digest == digest.finish())?;
            let mut observed = Vec::new();
            while let Some(item) = cursor.next_action(&mut work)? {
                observed.push(item);
            }
            require(observed == expected && cursor.next_action(&mut work)?.is_none())?;
            drop(cursor);
            require(ledger.live.load(Ordering::SeqCst) == 0)?;
        }
        Ok(())
    }

    #[test]
    fn sealed_work_cursor_rechecks_commitment_and_sticks_on_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let objects = FileObjectStore::open(home.path().join("objects"))?;
        let mut work = work_budget()?;
        let spool = WorkSpool::open(&space, &mut work)?;
        spool.push(&action(1), &mut work)?;
        spool.push(&action(2), &mut work)?;
        let (_sealed, mut cursor) = WorkSpool::seal_lazy(Some(spool), &objects, &mut work)?;
        let key = action(2).key()?;
        let removed = cursor
            .spool
            .as_ref()
            .ok_or("missing owned spool")?
            .connection
            .scoped()?
            .execute("DELETE FROM actions WHERE key=?1", params![key.as_slice()])?;
        require(removed == 1)?;
        require(cursor.next_action(&mut work)?.is_some())?;
        let failure = cursor
            .next_action(&mut work)
            .expect_err("changed scratch must fail at EOF");
        require(matches!(failure, IndexError::CorruptObject(_)))?;
        require(cursor.next_action(&mut work) == Err(failure))?;
        Ok(())
    }

    #[test]
    fn sealing_refuses_noncanonical_payload_and_mismatched_key()
    -> Result<(), Box<dyn std::error::Error>> {
        for corrupt_key in [false, true] {
            let home = tempfile::tempdir()?;
            let space =
                FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
            let objects = FileObjectStore::open(home.path().join("objects"))?;
            let mut work = work_budget()?;
            let spool = WorkSpool::open(&space, &mut work)?;
            let item = action(7);
            spool.push(&item, &mut work)?;
            if corrupt_key {
                let original = item.key()?;
                let bad = [255_u8; 33];
                let changed = spool.connection.scoped()?.execute(
                    "UPDATE actions SET key=?2 WHERE key=?1",
                    params![original.as_slice(), bad.as_slice()],
                )?;
                require(changed == 1)?;
            } else {
                let changed = spool
                    .connection
                    .scoped()?
                    .execute("UPDATE actions SET payload=?1", params![b"{}".as_slice()])?;
                require(changed == 1)?;
            }
            require(matches!(
                WorkSpool::seal_lazy(Some(spool), &objects, &mut work),
                Err(IndexError::CorruptObject(_))
            ))?;
        }
        Ok(())
    }

    #[test]
    fn sealed_work_cursor_rejects_an_extra_action_before_returning_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let objects = FileObjectStore::open(home.path().join("objects"))?;
        let mut work = work_budget()?;
        let spool = WorkSpool::open(&space, &mut work)?;
        spool.push(&action(1), &mut work)?;
        let (sealed, mut cursor) = WorkSpool::seal_lazy(Some(spool), &objects, &mut work)?;
        require(sealed.count == 1)?;
        let extra = action(2);
        let key = extra.key()?;
        let payload = crate::staging_payload::encode(&extra, MAX_WORK_ACTION_BYTES, &work)?;
        let inserted = cursor
            .spool
            .as_ref()
            .ok_or("missing owned spool")?
            .connection
            .scoped()?
            .execute(
                "INSERT INTO actions(key,payload) VALUES(?1,?2)",
                params![key.as_slice(), payload.bytes.as_slice()],
            )?;
        require(inserted == 1 && cursor.next_action(&mut work)?.is_some())?;
        let failure = cursor
            .next_action(&mut work)
            .expect_err("extra action must fail");
        require(matches!(failure, IndexError::CorruptObject(_)))?;
        require(cursor.next_action(&mut work) == Err(failure))?;
        Ok(())
    }

    #[test]
    fn sealed_work_cursor_empty_and_cancelled_eof_are_checked()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut work = work_budget()?;
        let home = tempfile::tempdir()?;
        let objects = FileObjectStore::open(home.path().join("objects"))?;
        let (sealed, mut cursor) = WorkSpool::seal_lazy(None, &objects, &mut work)?;
        require(sealed == WorkActionSet::empty() && cursor.next_action(&mut work)?.is_none())?;
        work.request().cancel_handle().cancel();
        require(matches!(
            cursor.next_action(&mut work),
            Err(IndexError::Cancelled)
        ))?;
        require(matches!(
            cursor.next_action(&mut work),
            Err(IndexError::Cancelled)
        ))?;
        Ok(())
    }

    #[test]
    fn work_page_admits_array_and_payload_then_releases_on_partial_refusal()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let mut work = work_budget()?;
        let spool = WorkSpool::open(&space, &mut work)?;
        let key = [1_u8; 33];
        let _inserted = spool.connection.scoped()?.execute(
            "INSERT INTO actions(key,payload) VALUES(?1,?2)",
            params![key.as_slice(), b"{}".as_slice()],
        )?;
        let array = u64::try_from(128 * std::mem::size_of::<WorkScratchRow>())?;
        for cap in [0, array, array + 2] {
            let ledger = bind(&mut work, cap);
            require(spool.page(Some(key.as_slice()), &mut work)?.rows.is_empty())?;
            require(ledger.peak.load(Ordering::SeqCst) == 0)?;
            let result = spool.page(None, &mut work);
            if cap == array + 2 {
                let page = result?;
                let row = page.rows.first().ok_or("missing row")?;
                require(row.key == key && row.payload.bytes == b"{}")?;
                require(ledger.live.load(Ordering::SeqCst) == array + 2)?;
                drop(page);
            } else {
                require(matches!(result, Err(IndexError::ResourceLimited)))?;
                require(ledger.peak.load(Ordering::SeqCst) == cap)?;
            }
            require(ledger.live.load(Ordering::SeqCst) == 0)?;
        }
        Ok(())
    }

    #[test]
    fn work_page_preserves_128_boundary_and_refuses_blob_shapes_before_copy()
    -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let mut work = work_budget()?;
        let spool = WorkSpool::open(&space, &mut work)?;
        for number in 0..129_u8 {
            let mut key = [0_u8; 33];
            key[32] = number;
            let _inserted = spool.connection.scoped()?.execute(
                "INSERT INTO actions(key,payload) VALUES(?1,?2)",
                params![key.as_slice(), b"{}".as_slice()],
            )?;
        }
        let array = u64::try_from(128 * std::mem::size_of::<WorkScratchRow>())?;
        let ledger = bind(&mut work, array + 256);
        let page = spool.page(None, &mut work)?;
        require(page.rows.len() == 128)?;
        let after = page.rows.last().ok_or("missing boundary")?.key;
        require(after.last() == Some(&127))?;
        drop(page);
        let tail = spool.page(Some(after.as_slice()), &mut work)?;
        require(
            tail.rows.len() == 1
                && tail
                    .rows
                    .first()
                    .is_some_and(|row| row.key.last() == Some(&128)),
        )?;
        drop(tail);
        require(ledger.live.load(Ordering::SeqCst) == 0)?;
        let denied = bind(&mut work, 0);
        let _removed = spool
            .connection
            .scoped()?
            .execute("DELETE FROM actions", [])?;
        for (key, payload) in [
            (vec![0_u8; 32], b"{}".to_vec()),
            (vec![0_u8; 33], vec![0_u8; MAX_WORK_ACTION_BYTES + 1]),
        ] {
            let _inserted = spool.connection.scoped()?.execute(
                "INSERT INTO actions(key,payload) VALUES(?1,?2)",
                params![key, payload],
            )?;
            require(matches!(
                spool.page(None, &mut work),
                Err(IndexError::CorruptObject(_))
            ))?;
            let _removed = spool
                .connection
                .scoped()?
                .execute("DELETE FROM actions", [])?;
        }
        require(denied.peak.load(Ordering::SeqCst) == 0)?;
        Ok(())
    }

    #[test]
    fn edit_batch_preserves_mixed_root_inline_gold_and_refuses_before_sql()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_core::index::{Digest, MapValue, ObjectRef};
        let home = tempfile::tempdir()?;
        let space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let mut work = work_budget()?;
        let spool = EditSpool::open(&space, &mut work)?;
        let mut rows = Vec::new();
        for number in 0..64u8 {
            rows.push(OrdinalMapEdit::copy_key(
                u32::from(number % 2),
                &[number],
                None,
                Some(MapValue::inline(
                    &[number],
                    Some(ObjectRef(Digest([number; 32]))),
                )?),
                &work,
            )?);
        }
        spool.push_batch(&rows, &mut work)?;
        let mut cursor = spool.cursor();
        for root in 0..2u32 {
            for number in (0..64u8).filter(|number| u32::from(number % 2) == root) {
                let edit = cursor.next_edit(&mut work)?.ok_or("batch omitted edit")?;
                require(
                    edit.root == root && edit.edit.key == [number] && edit.edit.expected.is_none(),
                )?;
                require(
                    edit.edit.next
                        == Some(MapValue::inline(
                            &[number],
                            Some(ObjectRef(Digest([number; 32]))),
                        )?),
                )?;
            }
        }
        require(cursor.next_edit(&mut work)?.is_none())?;
        let (mut denied, _ledger) = crate::staging_payload::tests::scoped_budget(0)?;
        let row = OrdinalMapEdit::copy_key(0, &[64], None, None, &work)?;
        require(matches!(
            spool.push_batch(std::slice::from_ref(&row), &mut denied),
            Err(IndexError::ResourceLimited)
        ))?;
        require(matches!(
            spool.push_batch(&[], &mut work),
            Err(IndexError::ResourceLimited)
        ))?;
        rows.push(row.clone());
        require(matches!(
            spool.push_batch(&rows, &mut work),
            Err(IndexError::ResourceLimited)
        ))?;
        let mut cancelled = work_budget()?;
        cancelled.request().cancel_handle().cancel();
        require(matches!(
            spool.push_batch(std::slice::from_ref(&row), &mut cancelled),
            Err(IndexError::Cancelled)
        ))?;
        let count: i64 =
            spool
                .connection
                .scoped()?
                .query_row("SELECT COUNT(*) FROM edits", [], |row| row.get(0))?;
        require(count == 64)?;
        // A collision is a producer/backend violation and must not masquerade
        // as a successful short batch. This disposable spool is abandoned.
        require(matches!(
            spool.push_batch(
                std::slice::from_ref(rows.first().ok_or("batch input absent")?),
                &mut work
            ),
            Err(IndexError::BackendViolation(_))
        ))?;
        Ok(())
    }

    #[test]
    fn work_and_edit_successor_pages_seek_without_prefix_scans()
    -> Result<(), Box<dyn std::error::Error>> {
        fn key(number: u32) -> Result<[u8; 33], IndexError> {
            let mut key = [0_u8; 33];
            key.get_mut(..4)
                .ok_or(IndexError::ResourceLimited)?
                .copy_from_slice(&number.to_be_bytes());
            Ok(key)
        }

        let home = tempfile::tempdir()?;
        let space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let mut work = work_budget()?;
        let (connection, allocation) = open(
            "CREATE TABLE actions(key BLOB PRIMARY KEY,payload BLOB NOT NULL) WITHOUT ROWID; \
             CREATE TABLE edits(root INTEGER NOT NULL,key BLOB NOT NULL,expected BLOB,next BLOB,PRIMARY KEY(root,key)) WITHOUT ROWID;",
            &space,
            &mut work,
        )?;
        let db = connection.scoped()?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &db,
            allocation.as_ref(),
            1_000_000,
            &mut work,
        )?;
        db.execute_batch("BEGIN IMMEDIATE")?;
        let mut actions = db.prepare_cached("INSERT INTO actions(key,payload) VALUES(?1,?2)")?;
        let mut edits = db.prepare_cached("INSERT INTO edits(root,key) VALUES(?1,?2)")?;
        for ordinal in 0..1024_u32 {
            let action_key = key(ordinal)?;
            let edit_key = key(ordinal % 512)?;
            let _action = actions.execute(params![action_key.as_slice(), b"x".as_slice()])?;
            let root = ordinal
                .checked_div(512)
                .ok_or(IndexError::ResourceLimited)?;
            let _edit = edits.execute(params![root, edit_key.as_slice()])?;
        }
        drop(actions);
        drop(edits);
        db.execute_batch("COMMIT")?;

        let mut first_work_query = db.prepare(WORK_PAGE_START_SQL)?;
        let mut first_work_rows = first_work_query.query([])?;
        let mut first_work = Vec::new();
        while let Some(row) = first_work_rows.next()? {
            first_work.push(row.get::<_, Vec<u8>>(0)?);
        }
        drop(first_work_rows);
        let expected_first_work = (0..128_u32)
            .map(|number| key(number).map(|key| key.to_vec()))
            .collect::<Result<Vec<_>, _>>()?;
        require(first_work == expected_first_work)?;

        let mut first_edit_query = db.prepare(EDIT_PAGE_START_SQL)?;
        let mut first_edit_rows = first_edit_query.query([])?;
        let mut first_edits = Vec::new();
        while let Some(row) = first_edit_rows.next()? {
            first_edits.push((row.get::<_, u32>(0)?, row.get::<_, Vec<u8>>(1)?));
        }
        drop(first_edit_rows);
        let expected_first_edits = (0..128_u32)
            .map(|number| key(number).map(|key| (0_u32, key.to_vec())))
            .collect::<Result<Vec<_>, _>>()?;
        require(first_edits == expected_first_edits)?;

        let work_after = key(400)?;
        let expected_work = (401..529_u32)
            .map(|number| key(number).map(|key| key.to_vec()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut work_query = db.prepare(WORK_PAGE_AFTER_SQL)?;
        let mut work_rows = work_query.query(params![work_after.as_slice()])?;
        let mut observed_work = Vec::new();
        while let Some(row) = work_rows.next()? {
            observed_work.push(row.get::<_, Vec<u8>>(0)?);
        }
        drop(work_rows);
        require(observed_work == expected_work)?;
        require(work_query.get_status(StatementStatus::FullscanStep) == 0)?;

        let edit_after = key(400)?;
        let expected_edits = (401..512_u32)
            .map(|number| key(number).map(|key| (0_u32, key.to_vec())))
            .chain((0..17_u32).map(|number| key(number).map(|key| (1_u32, key.to_vec()))))
            .collect::<Result<Vec<_>, _>>()?;
        let mut edit_query = db.prepare(EDIT_PAGE_AFTER_SQL)?;
        let mut edit_rows = edit_query.query(params![0_u32, edit_after.as_slice()])?;
        let mut observed_edits = Vec::new();
        while let Some(row) = edit_rows.next()? {
            observed_edits.push((row.get::<_, u32>(0)?, row.get::<_, Vec<u8>>(1)?));
        }
        drop(edit_rows);
        require(observed_edits == expected_edits)?;
        require(edit_query.get_status(StatementStatus::FullscanStep) == 0)?;

        // On this same bundled SQLite connection, the former nullable-OR
        // predicates walk the prefix before a deep continuation.
        let mut old_work = db.prepare(
            "SELECT key,payload FROM actions WHERE (?1 IS NULL OR key>?1) ORDER BY key LIMIT 128",
        )?;
        let mut old_work_rows = old_work.query(params![work_after.as_slice()])?;
        while old_work_rows.next()?.is_some() {}
        drop(old_work_rows);
        require(old_work.get_status(StatementStatus::FullscanStep) > 0)?;
        let mut old_edits = db.prepare(
            "SELECT root,key,expected,next FROM edits WHERE (?1 IS NULL OR root>?2 OR (root=?2 AND key>?1)) ORDER BY root,key LIMIT 128",
        )?;
        let mut old_edit_rows = old_edits.query(params![edit_after.as_slice(), 0_u32])?;
        while old_edit_rows.next()?.is_some() {}
        drop(old_edit_rows);
        require(old_edits.get_status(StatementStatus::FullscanStep) > 0)?;
        Ok(())
    }

    #[test]
    fn scratch_uses_file_with_enforced_page_cap() -> Result<(), IndexError> {
        let home = tempfile::tempdir()
            .map_err(|error| IndexError::Storage(format!("test scratch root: {error}")))?;
        let scratch_space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            deadline,
        );
        let (connection, allocation) = open(
            "CREATE TABLE payloads(value BLOB);",
            &scratch_space,
            &mut budget,
        )?;
        let temp_store: u32 = connection
            .scoped()?
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        if temp_store != 1 {
            return Err(IndexError::BackendViolation(format!(
                "work scratch temp_store must be FILE (1), got {temp_store}"
            )));
        }
        let max_pages: u32 = connection
            .scoped()?
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        let maximum_page_bytes = u64::from(max_pages)
            .checked_mul(u64::from(SCRATCH_PAGE_SIZE))
            .ok_or(IndexError::ResourceLimited)?;
        let reserved_bytes = allocation.reserved_bytes()?;
        if maximum_page_bytes > reserved_bytes || reserved_bytes < SCRATCH_INITIAL_BYTES {
            return Err(IndexError::BackendViolation(format!(
                "work scratch page cap {maximum_page_bytes} exceeds reserved {reserved_bytes}, or reservation is below initial {SCRATCH_INITIAL_BYTES}"
            )));
        }
        let refused = quanta_index_scratch_sqlite::reserve_for_write(
            &*connection.scoped()?,
            allocation.as_ref(),
            SCRATCH_MAX_BYTES,
            &mut budget,
        );
        require(matches!(refused, Err(IndexError::ResourceLimited)))?;
        require(allocation.reserved_bytes()? == reserved_bytes)?;
        let untouched_rows: i64 = connection
            .scoped()?
            .query_row("SELECT count(*) FROM payloads", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        require(untouched_rows == 0)?;
        let _changed: u32 = connection
            .scoped()?
            .query_row("PRAGMA max_page_count=4", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        let full = connection
            .scoped()?
            .execute("INSERT INTO payloads(value) VALUES(zeroblob(1048576))", []);
        // The page guard must still reject the write. A raw SQLITE_FULL has no
        // operation-scoped admission witness, so preserve its storage outcome.
        if !matches!(&full, Err(rusqlite::Error::SqliteFailure(failure, _)) if failure.code == rusqlite::ErrorCode::DiskFull)
        {
            return Err(IndexError::BackendViolation(format!(
                "work scratch hard page cap failed to emit SQLITE_FULL: {full:?}"
            )));
        }
        require(matches!(
            full.map_err(|error| sql_error(&error)),
            Err(IndexError::Storage(_))
        ))?;
        Ok(())
    }
}
