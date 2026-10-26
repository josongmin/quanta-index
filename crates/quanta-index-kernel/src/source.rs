use crate::path_staging::{PathKey, PathPage, PathPageCursor, PathRow};
use quanta_index_scratch_sqlite::ScratchConnection;
use rusqlite::params;

#[cfg(test)]
mod coverage_tests;
mod owner_edits;

use quanta_index_core::index::{
    AttemptContext, AttemptRef, IndexError, MAX_SOURCE_PATH_BYTES, MapPageCursor, MapPageEntry,
    MapRoot, ObjectRef, OperationKey, OrdinalMapEdit, OwnerKey, OwnerMutation, OwnerMutationCursor,
    ProducerId, ScratchAllocation, ScratchRequest, ScratchSpacePort, SourceChangeCursor, SourceCut,
    SourceDelta, SourceEvent, SourceOperationMode, SourceRecordOwner, SourceSnapshotCursor,
    SourceTextAdmission, SourceTransition, ViewRef, WorkBudget, WorkUsage, owner_from_map_key,
    owner_map_key,
    ports::{ImmutableObjectPort, MapEditCursor, PersistentMapPort, SourceAuthorityPort},
    producer_owner_prefix,
};

/// Record/body validation completed by this attempt's Source application.
/// Private construction prevents an adapter or caller from asserting validation.
pub(crate) struct AppliedSource {
    delta: SourceDelta,
    attempt: AttemptRef,
}

impl AppliedSource {
    pub(crate) fn delta(&self) -> &SourceDelta {
        &self.delta
    }
}

/// Coverage needs owner/reference identities. A first snapshot still performs
/// full record validation; an applied patch consumes this attempt's proof.
pub(crate) enum CoverageChanges<'a> {
    Applied(Box<SourceChangeCursor<'a>>),
    Snapshot(Box<SourceSnapshotCursor<'a>>),
}

impl<'a> CoverageChanges<'a> {
    pub(crate) fn new(
        source: SourceCut,
        applied: Option<&AppliedSource>,
        attempt: AttemptRef,
        maps: &'a dyn PersistentMapPort,
        objects: &'a dyn ImmutableObjectPort,
    ) -> Result<Self, IndexError> {
        if let Some(applied) = applied {
            if applied.attempt != attempt || applied.delta.next != source {
                return Err(IndexError::BackendViolation(
                    "source validation belongs to another attempt or cut".into(),
                ));
            }
            Ok(Self::Applied(Box::new(SourceChangeCursor::new(
                applied.delta.changed,
                applied.delta.prior,
                applied.delta.next,
                maps,
                objects,
            ))))
        } else {
            Ok(Self::Snapshot(Box::new(SourceSnapshotCursor::new(
                source, maps, objects,
            ))))
        }
    }

    pub(crate) fn next_mutation(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<OwnerMutation>, IndexError> {
        match self {
            Self::Applied(cursor) => cursor.next_mutation(budget),
            Self::Snapshot(cursor) => Ok(cursor.next_change(budget)?.map(|change| OwnerMutation {
                owner: change.owner,
                expected: change.prior.map(|(reference, _record)| reference),
                next: change.next.map(|(reference, _record)| reference),
            })),
        }
    }
}

/// Scans only one producer's contiguous owner-key range. The retained map-page
/// position owns its page's memory charge across calls.
struct ProducerOwnerCursor<'a> {
    maps: &'a dyn PersistentMapPort,
    root: MapRoot,
    prefix: [u8; 22],
    page: MapPageCursor,
    after: Option<MapPageEntry>,
    more: bool,
    finished: bool,
}

impl<'a> ProducerOwnerCursor<'a> {
    fn new(maps: &'a dyn PersistentMapPort, root: MapRoot, producer: ProducerId) -> Self {
        Self {
            maps,
            root,
            prefix: producer_owner_prefix(producer),
            page: MapPageCursor::default(),
            after: None,
            more: true,
            finished: false,
        }
    }

    fn next_owner(&mut self, budget: &mut WorkBudget) -> Result<Option<OwnerKey>, IndexError> {
        const PAGE_SIZE: u32 = 64;
        budget.checkpoint()?;
        loop {
            if let Some(entry) = self.page.next() {
                let owner = {
                    let (key, _reference) = entry.object_parts()?;
                    if self
                        .after
                        .as_ref()
                        .map(MapPageEntry::key)
                        .transpose()?
                        .is_some_and(|previous| key.as_slice() <= previous)
                    {
                        return Err(IndexError::BackendViolation(
                            "producer owner scan is not strictly ordered".into(),
                        ));
                    }
                    if !key.starts_with(&self.prefix) {
                        self.finished = true;
                        return Ok(None);
                    }
                    owner_from_map_key(key)?
                };
                self.after = Some(entry);
                return Ok(Some(owner));
            }
            if self.finished || !self.more {
                self.finished = true;
                return Ok(None);
            }
            let after = match self.after.as_ref() {
                Some(entry) => entry.key()?,
                None => self.prefix.as_slice(),
            };
            let page = self.maps.scan(self.root, Some(after), PAGE_SIZE, budget)?;
            if (page.entries.is_empty() && page.has_more)
                || page.entries.len()
                    > usize::try_from(PAGE_SIZE).map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::BackendViolation(
                    "producer owner scan exceeded its page or did not advance".into(),
                ));
            }
            if page
                .entries
                .first()
                .is_some_and(|(key, _reference)| key.as_slice() <= after)
            {
                return Err(IndexError::BackendViolation(
                    "producer owner scan did not advance".into(),
                ));
            }
            self.more = page.has_more;
            self.page = page.cursor();
        }
    }
}

pub(crate) fn validate_path(path: &str) -> Result<(), IndexError> {
    if path.is_empty()
        || path.len() > MAX_SOURCE_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(IndexError::InvalidInput(
            "source path is not canonical relative path".into(),
        ));
    }
    Ok(())
}

fn read_record(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceRecordOwner, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "source record object reference mismatch".into(),
        ));
    }
    let record = SourceRecordOwner::decode_json(
        &stored.payload,
        context.budget,
        quanta_index_core::index::MemoryLifetime::Temporary,
    )?;
    validate_path(&record.path)?;
    if stored.registration.children.as_slice() != [record.bytes] {
        return Err(IndexError::CorruptObject(
            "source record must directly own exactly its byte object".into(),
        ));
    }
    if matches!(record.text, SourceTextAdmission::Utf8 { .. }) {
        let body = context.objects.get(record.bytes, context.budget)?;
        if body.registration.reference != record.bytes
            || !body.registration.children.is_empty()
            || u64::try_from(body.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
                != record.byte_len
        {
            return Err(IndexError::CorruptObject(
                "text-admitted source body identity, edges or length mismatch".into(),
            ));
        }
        if std::str::from_utf8(&body.payload).is_err() {
            return Err(IndexError::InvalidInput(
                "source text admission requires UTF-8".into(),
            ));
        }
    }
    // Next record byte objects were fully verified and adopted atomically at
    // admission. Prior record bytes are protected by the exact base ticket.
    Ok(record)
}

fn read_path_owner(
    value: quanta_index_core::index::MapValue,
    context: &AttemptContext<'_>,
) -> Result<OwnerKey, IndexError> {
    quanta_index_core::index::owner_from_path_map_value(value, context.budget)
}

const SCRATCH_PAGE_SIZE: u32 = 4096;
const SCRATCH_MAX_BYTES: u64 = 134_217_728;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;

fn scratch_error(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "source path scratch")
}

fn owner_bytes(owner: OwnerKey) -> [u8; 32] {
    let mut bytes = [0; 32];
    if let Some(producer) = bytes.get_mut(..16) {
        producer.copy_from_slice(&owner.producer.0);
    }
    if let Some(stable_id) = bytes.get_mut(16..) {
        stable_id.copy_from_slice(&owner.stable_id.0);
    }
    bytes
}

fn owner_from_bytes(bytes: &[u8]) -> Result<OwnerKey, IndexError> {
    let bytes: &[u8; 32] = bytes
        .try_into()
        .map_err(|_error| IndexError::CorruptObject("source path scratch owner length".into()))?;
    let mut producer = [0; 16];
    producer.copy_from_slice(
        bytes.get(..16).ok_or_else(|| {
            IndexError::CorruptObject("source path scratch producer owner".into())
        })?,
    );
    let mut stable_id = [0; 16];
    stable_id.copy_from_slice(
        bytes
            .get(16..)
            .ok_or_else(|| IndexError::CorruptObject("source path scratch stable owner".into()))?,
    );
    Ok(OwnerKey {
        producer: quanta_index_core::index::ProducerId(producer),
        stable_id: quanta_index_core::index::StableOwnerId(stable_id),
    })
}

/// Attempt-local disk spill without publication authority.
///
/// The unique path key sees all mutation pages, making a cross-page swap and
/// a duplicate target distinguishable. The catalog CAS owns publication.
struct PathScratch {
    connection: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
}

const PATH_PAGE_START_SQL: &str =
    "SELECT path,leaving,incoming,expected,next FROM path_edits ORDER BY path LIMIT 128";
const PATH_PAGE_AFTER_SQL: &str = "SELECT path,leaving,incoming,expected,next FROM path_edits WHERE path > ?1 ORDER BY path LIMIT 128";

impl PathScratch {
    fn open(scratch: &dyn ScratchSpacePort, budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let allocation = scratch.allocate(
            ScratchRequest {
                initial_bytes: SCRATCH_INITIAL_BYTES,
                max_bytes: SCRATCH_MAX_BYTES,
            },
            budget,
        )?;
        if allocation.max_bytes() != SCRATCH_MAX_BYTES {
            return Err(IndexError::BackendViolation(
                "source path scratch allocation changed its envelope".into(),
            ));
        }
        let owned_connection =
            ScratchConnection::open(&allocation.path().join("paths.sqlite"), allocation.as_ref())?;
        let connection = owned_connection.scoped()?;
        connection
            .execute_batch(
                "PRAGMA page_size=4096; PRAGMA journal_mode=OFF; \
                 PRAGMA synchronous=OFF; PRAGMA temp_store=FILE; PRAGMA mmap_size=0; \
                 PRAGMA cache_size=-1024;",
            )
            .map_err(|error| scratch_error(&error))?;
        quanta_index_scratch_sqlite::configure(&connection, allocation.as_ref())?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &connection,
            allocation.as_ref(),
            8192,
            budget,
        )?;
        connection
            .execute_batch(
                "CREATE TABLE path_edits( \
                 path TEXT PRIMARY KEY, leaving BLOB, incoming BLOB, expected BLOB, next BLOB); \
                 CREATE TABLE owner_edits(owner BLOB PRIMARY KEY NOT NULL, expected BLOB, next BLOB) WITHOUT ROWID;",
            )
            .map_err(|error| scratch_error(&error))?;
        let page_size: u32 = connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        let max_pages: u32 = connection
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        if page_size != SCRATCH_PAGE_SIZE
            || u64::from(max_pages)
                .checked_mul(u64::from(page_size))
                .ok_or(IndexError::ResourceLimited)?
                > allocation.reserved_bytes()?
        {
            return Err(IndexError::Storage(
                "source path scratch page limit was not installed".into(),
            ));
        }
        drop(connection);
        Ok(Self {
            connection: owned_connection,
            allocation,
        })
    }

    fn mark(
        &self,
        path: &str,
        owner: OwnerKey,
        leaving: bool,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let incoming = u64::try_from(path.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(160)
            .ok_or(IndexError::ResourceLimited)?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.connection.scoped()?,
            self.allocation.as_ref(),
            incoming,
            budget,
        )?;
        // One guarded upsert keeps the two independent path claims. A second
        // claimant must fail even when its owner identity is identical.
        let query = if leaving {
            "INSERT INTO path_edits(path,leaving) VALUES(?1,?2) \
             ON CONFLICT(path) DO UPDATE SET leaving=excluded.leaving \
             WHERE path_edits.leaving IS NULL"
        } else {
            "INSERT INTO path_edits(path,incoming) VALUES(?1,?2) \
             ON CONFLICT(path) DO UPDATE SET incoming=excluded.incoming \
             WHERE path_edits.incoming IS NULL"
        };
        let db = self.connection.scoped()?;
        let changed = db
            .prepare_cached(query)
            .map_err(|error| scratch_error(&error))?
            .execute(params![path, owner_bytes(owner).as_slice()])
            .map_err(|error| scratch_error(&error))?;
        if changed != 1 {
            return Err(if leaving {
                IndexError::CorruptObject("duplicate prior path owner".into())
            } else {
                IndexError::InvalidInput("duplicate target path".into())
            });
        }
        Ok(())
    }

    fn page(&self, after: Option<&str>, budget: &mut WorkBudget) -> Result<PathPage, IndexError> {
        budget.checkpoint()?;
        let io_scope = self.connection.scoped()?;
        // Separate the first page from the seek so SQLite can use the path
        // index rather than rescan the entire preceding prefix on every page.
        let query = if after.is_some() {
            PATH_PAGE_AFTER_SQL
        } else {
            PATH_PAGE_START_SQL
        };
        let mut statement = io_scope
            .prepare(query)
            .map_err(|error| scratch_error(&error))?;
        let mut cursor = match after {
            Some(path) => statement.query(params![path]),
            None => statement.query([]),
        }
        .map_err(|error| scratch_error(&error))?;
        let Some(first) = cursor.next().map_err(|error| scratch_error(&error))? else {
            budget.checkpoint()?;
            return Ok(PathPage::default());
        };
        let first = borrow_path_row(first)?;
        let mut page = PathPage::new(budget)?;
        page.push(copy_path_row(&first, budget)?)?;
        while let Some(row) = cursor.next().map_err(|error| scratch_error(&error))? {
            page.push(copy_path_row(&borrow_path_row(row)?, budget)?)?;
        }
        Ok(page)
    }

    fn set_refs(
        &self,
        path: &str,
        expected: Option<quanta_index_core::index::MapValue>,
        next: Option<quanta_index_core::index::MapValue>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let incoming = u64::try_from(path.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(256)
            .ok_or(IndexError::ResourceLimited)?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.connection.scoped()?,
            self.allocation.as_ref(),
            incoming,
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
        let changed = self
            .connection
            .scoped()?
            .execute(
                "UPDATE path_edits SET expected=?2,next=?3 WHERE path=?1",
                params![path, expected, next],
            )
            .map_err(|error| scratch_error(&error))?;
        if changed != 1 {
            return Err(IndexError::BackendViolation(
                "source path scratch lost path".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct BorrowedPathRow<'a> {
    path: &'a str,
    leaving: Option<OwnerKey>,
    incoming: Option<OwnerKey>,
    expected: Option<quanta_index_core::index::MapValue>,
    next: Option<quanta_index_core::index::MapValue>,
}

fn optional_path_blob(
    value: rusqlite::types::ValueRef<'_>,
) -> Result<Option<[u8; 32]>, IndexError> {
    if value == rusqlite::types::ValueRef::Null {
        Ok(None)
    } else {
        quanta_index_scratch_sqlite::fixed_blob(value).map(Some)
    }
}

fn borrow_path_row<'a>(row: &'a rusqlite::Row<'_>) -> Result<BorrowedPathRow<'a>, IndexError> {
    let rusqlite::types::ValueRef::Text(bytes) =
        row.get_ref(0).map_err(|error| scratch_error(&error))?
    else {
        return Err(IndexError::CorruptObject(
            "source path scratch path type".into(),
        ));
    };
    if bytes.len() > MAX_SOURCE_PATH_BYTES {
        return Err(IndexError::CorruptObject(
            "source path scratch path exceeds bound".into(),
        ));
    }
    let path = std::str::from_utf8(bytes)
        .map_err(|_error| IndexError::CorruptObject("source path scratch UTF-8".into()))?;
    validate_path(path)
        .map_err(|_error| IndexError::CorruptObject("source path scratch canonical path".into()))?;
    let leaving = optional_path_blob(row.get_ref(1).map_err(|error| scratch_error(&error))?)?
        .as_ref()
        .map(|bytes| owner_from_bytes(bytes))
        .transpose()?;
    let incoming = optional_path_blob(row.get_ref(2).map_err(|error| scratch_error(&error))?)?
        .as_ref()
        .map(|bytes| owner_from_bytes(bytes))
        .transpose()?;
    let expected = quanta_index_scratch_sqlite::optional_map_value(
        row.get_ref(3).map_err(|error| scratch_error(&error))?,
    )?;
    let next = quanta_index_scratch_sqlite::optional_map_value(
        row.get_ref(4).map_err(|error| scratch_error(&error))?,
    )?;
    Ok(BorrowedPathRow {
        path,
        leaving,
        incoming,
        expected,
        next,
    })
}

fn copy_path_row(
    row: &BorrowedPathRow<'_>,
    budget: &mut WorkBudget,
) -> Result<PathRow, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: u64::try_from(row.path.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(256)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    PathRow::copy(
        row.path,
        row.leaving,
        row.incoming,
        row.expected,
        row.next,
        budget,
    )
}

fn validate_source_authority(
    operation: OperationKey,
    base_view: ViewRef,
    mode: SourceOperationMode,
    events: &[SourceEvent],
    authority: &dyn SourceAuthorityPort,
    budget: &mut WorkBudget,
) -> Result<Option<ProducerId>, IndexError> {
    match mode {
        SourceOperationMode::Unchanged => Err(IndexError::InvalidInput(
            "source transition has no source operation mode".into(),
        )),
        SourceOperationMode::Advance => {
            for event in events {
                budget.checkpoint()?;
                if authority.source_epoch(base_view, event.producer, budget)?
                    != Some(event.stream_epoch)
                {
                    return Err(IndexError::Conflict);
                }
            }
            Ok(None)
        }
        SourceOperationMode::Resync { prior_epoch } => {
            let [event] = events else {
                return Err(IndexError::InvalidInput(
                    "source resync requires exactly one event".into(),
                ));
            };
            if (event.producer, event.stream, event.stream_epoch)
                != (operation.producer, operation.stream, operation.stream_epoch)
                || event.stream_epoch <= prior_epoch
                || event.predecessor != 0
                || event.sequence != 1
                || authority.source_epoch(base_view, event.producer, budget)? != Some(prior_epoch)
            {
                return Err(IndexError::Conflict);
            }
            Ok(Some(event.producer))
        }
    }
}

struct SourceEditCursor<'a> {
    owners: owner_edits::OwnerEditCursor<'a>,
    paths: &'a PathScratch,
    path_page: PathPageCursor,
    after_path: Option<PathRow>,
    frontier_edits: crate::frontier::FrontierEdits,
    phase: u8,
}

impl MapEditCursor for SourceEditCursor<'_> {
    fn next_edit(
        &mut self,
        budget: &mut quanta_index_core::index::WorkBudget,
    ) -> Result<Option<OrdinalMapEdit>, IndexError> {
        loop {
            match self.phase {
                0 => match self.owners.next_mutation(budget)? {
                    Some(mutation) => {
                        return Ok(Some(OrdinalMapEdit::copy_object_key(
                            0,
                            &owner_map_key(mutation.owner),
                            mutation.expected,
                            mutation.next,
                            budget,
                        )?));
                    }
                    None => self.phase = 1,
                },
                1 => {
                    if self.path_page.is_empty() {
                        self.path_page = PathPageCursor::default();
                        self.path_page = self
                            .paths
                            .page(
                                self.after_path.as_ref().map(|row| row.path.as_str()),
                                budget,
                            )?
                            .into_iter();
                        if self.path_page.is_empty() {
                            self.phase = 2;
                            continue;
                        }
                    }
                    let row = self.path_page.next().ok_or_else(|| {
                        IndexError::BackendViolation("source path page disappeared".into())
                    })?;
                    self.after_path = Some(row.clone());
                    let expected = row.expected;
                    let next = row.next;
                    if expected != next {
                        return Ok(Some(OrdinalMapEdit::copy_key_parts(
                            1,
                            &[b"path\0", row.path.as_bytes()],
                            expected,
                            next,
                            budget,
                        )?));
                    }
                }
                _ => return self.frontier_edits.next_edit(budget),
            }
        }
    }
}

pub(crate) fn apply_source(
    operation: OperationKey,
    base_view: ViewRef,
    base: SourceCut,
    transition: &SourceTransition,
    mode: SourceOperationMode,
    authority: &dyn SourceAuthorityPort,
    context: &mut AttemptContext<'_>,
) -> Result<AppliedSource, IndexError> {
    let span = crate::diagnostic::begin(
        context.budget,
        quanta_index_core::DiagnosticStageV1::NativeSourceApply,
    );
    let result = apply_source_inner(
        operation, base_view, base, transition, mode, authority, context,
    )
    .map(|delta| AppliedSource {
        delta,
        attempt: context.attempt,
    });
    crate::diagnostic::finish_work(
        context.budget,
        quanta_index_core::DiagnosticStageV1::NativeSourceApply,
        span,
        result,
    )
}

fn apply_source_inner(
    operation: OperationKey,
    base_view: ViewRef,
    base: SourceCut,
    transition: &SourceTransition,
    mode: SourceOperationMode,
    authority: &dyn SourceAuthorityPort,
    context: &mut AttemptContext<'_>,
) -> Result<SourceDelta, IndexError> {
    if base != transition.expected_source {
        return Err(IndexError::Conflict);
    }
    crate::frontier::validate_event_shape(operation, &transition.events)?;
    let owner_digest = transition.owner_digest().map_err(|error| {
        IndexError::InvalidInput(format!("source owner digest encoding: {error}"))
    })?;
    if transition
        .events
        .iter()
        .any(|event| event.payload != owner_digest)
    {
        return Err(IndexError::InvalidInput(
            "source event payload does not bind complete owner transition".into(),
        ));
    }
    let resync_producer = validate_source_authority(
        operation,
        base_view,
        mode,
        &transition.events,
        authority,
        context.budget,
    )?;
    context.budget.charge(WorkUsage {
        mutations: u64::try_from(transition.events.len())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let scratch = PathScratch::open(context.scratch, context.budget)?;
    scratch.begin_owner_spool(context.budget)?;
    let mut mutations = OwnerMutationCursor::new(transition.owners, context.maps, context.objects);
    let mut producer_owners = resync_producer
        .map(|producer| ProducerOwnerCursor::new(context.maps, base.owners, producer));
    let mut next_existing = match producer_owners.as_mut() {
        Some(cursor) => cursor.next_owner(context.budget)?,
        None => None,
    };
    while let Some(mutation) = mutations.next_mutation(context.budget)? {
        if transition
            .events
            .binary_search_by_key(&mutation.owner.producer, |event| event.producer)
            .is_err()
        {
            return Err(IndexError::InvalidInput(
                "mutated owner has no source event".into(),
            ));
        }
        if let Some(producer) = resync_producer {
            if mutation.owner.producer != producer {
                return Err(IndexError::InvalidInput(
                    "source resync mutation crosses producer authority".into(),
                ));
            }
            if let Some(existing) = next_existing {
                if existing < mutation.owner {
                    return Err(IndexError::Conflict);
                }
                if existing == mutation.owner {
                    next_existing = match producer_owners.as_mut() {
                        Some(cursor) => cursor.next_owner(context.budget)?,
                        None => {
                            return Err(IndexError::BackendViolation(
                                "source resync owner cursor absent".into(),
                            ));
                        }
                    };
                }
            }
        }
        let key = owner_map_key(mutation.owner);
        let actual = context
            .maps
            .lookup_object(base.owners, &key, context.budget)?;
        if actual != mutation.expected {
            return Err(IndexError::Conflict);
        }
        if actual.is_none() && mutation.next.is_none() {
            return Err(IndexError::InvalidInput(
                "delete requires existing owner".into(),
            ));
        }
        let prior = actual
            .map(|reference| read_record(reference, context).map(|record| (reference, record)))
            .transpose()?;
        let next = mutation
            .next
            .map(|reference| read_record(reference, context).map(|record| (reference, record)))
            .transpose()?;
        if let Some((_, prior)) = &prior {
            let key = PathKey::copy(&prior.path, context.budget)?;
            let marker = context
                .maps
                .lookup(base.paths, key.as_slice(), context.budget)?;
            let recorded_owner = marker
                .map(|reference| read_path_owner(reference, context))
                .transpose()?;
            if recorded_owner != Some(mutation.owner) {
                return Err(IndexError::CorruptObject(
                    "source owner/path reverse entry mismatch".into(),
                ));
            }
        }
        let old_path = prior.as_ref().map(|(_, record)| record.path.as_str());
        let new_path = next.as_ref().map(|(_, record)| record.path.as_str());
        if old_path != new_path {
            if let Some(path) = old_path {
                scratch.mark(path, mutation.owner, true, context.budget)?;
            }
            if let Some(path) = new_path {
                scratch.mark(path, mutation.owner, false, context.budget)?;
            }
        }
        scratch.append_owner_edit(&mutation, context.budget)?;
    }
    if next_existing.is_some() {
        return Err(IndexError::Conflict);
    }
    let owners = scratch.validated_owner_edits(&mutations, transition.owners, context.budget)?;
    drop(mutations);

    let mut after: Option<PathRow> = None;
    loop {
        let page = scratch.page(after.as_ref().map(|row| row.path.as_str()), context.budget)?;
        if page.is_empty() {
            break;
        }
        for row in page {
            let key = PathKey::copy(&row.path, context.budget)?;
            let expected = context
                .maps
                .lookup(base.paths, key.as_slice(), context.budget)?;
            let current_owner = expected
                .map(|reference| read_path_owner(reference, context))
                .transpose()?;
            if let Some(leaver) = row.leaving {
                if current_owner != Some(leaver) {
                    return Err(IndexError::CorruptObject(
                        "owner/path reverse entry mismatch".into(),
                    ));
                }
            } else if current_owner.is_some() {
                return Err(IndexError::Conflict);
            }
            // This is attempt-local scratch, not an installed path marker.
            // Once the base path is validated, retain its final inline value
            // here so map application needs no second scratch rewrite pass.
            let next = row
                .incoming
                .map(quanta_index_core::index::path_owner_map_value)
                .transpose()?;
            scratch.set_refs(&row.path, expected, next, context.budget)?;
            after = Some(row);
        }
    }

    // Complete all base-path and frontier validation before installing any
    // immutable map or frontier object, including a collision in a later
    // scratch page. The scratch values above have no publication authority.
    let frontier_preparation = crate::frontier::PreparedFrontiers::prepare(
        operation,
        base.frontiers,
        &transition.events,
        context,
    )?;

    let map_span = crate::diagnostic::begin(
        context.budget,
        quanta_index_core::DiagnosticStageV1::NativeSourceMapApply,
    );
    let result = (|| {
        // Next frontier objects share the attempt's custody and are installed
        // only after every immutable-base precondition has passed.
        let frontier_edits = frontier_preparation.stage(context)?;
        let mut edits = SourceEditCursor {
            owners,
            paths: &scratch,
            path_page: PathPageCursor::default(),
            after_path: None,
            frontier_edits,
            phase: 0,
        };
        let roots = context.apply_stream(&[base.owners, base.paths, base.frontiers], &mut edits)?;
        let [owners, paths, frontiers]: [MapRoot; 3] =
            roots.as_slice().try_into().map_err(|_roots| {
                IndexError::BackendViolation("source map stream lost ordinal output".into())
            })?;
        Ok(SourceDelta {
            prior: base,
            next: SourceCut {
                owners,
                paths,
                frontiers,
            },
            changed: transition.owners,
        })
    })();
    crate::diagnostic::finish_work(
        context.budget,
        quanta_index_core::DiagnosticStageV1::NativeSourceMapApply,
        map_span,
        result,
    )
}

#[cfg(test)]
mod tests {
    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !($condition) {
                return Err(IndexError::BackendViolation(format!(
                    "path oracle failed: {}",
                    stringify!($condition)
                )));
            }
        };
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {
            match (&$left, &$right) {
                (left, right) if left != right => {
                    return Err(IndexError::BackendViolation(format!(
                        "path oracle mismatch: {left:?} != {right:?}"
                    )));
                }
                _ => {}
            }
        };
    }
    use std::time::{Duration, Instant};

    use quanta_index_core::index::{Digest, MapValue, WorkLimits};
    use quanta_index_storage::FileScratchSpace;

    use super::*;

    pub(super) fn path_fixture() -> Result<(tempfile::TempDir, PathScratch), IndexError> {
        let home = tempfile::tempdir()
            .map_err(|error| IndexError::Storage(format!("test path root: {error}")))?;
        let space =
            FileScratchSpace::open_standalone(home.path().join("scratch"), SCRATCH_MAX_BYTES)?;
        let (mut budget, _ledger) = crate::path_staging::tests::budget(u64::MAX)?;
        let paths = PathScratch::open(&space, &mut budget)?;
        Ok((home, paths))
    }

    #[test]
    fn path_claims_preserve_swap_identity_and_reject_duplicate_claimants() -> Result<(), IndexError>
    {
        let (_home, paths) = path_fixture()?;
        let (mut work, _ledger) = crate::path_staging::tests::budget(u64::MAX)?;
        let prior = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: quanta_index_core::index::StableOwnerId([2; 16]),
        };
        let next = OwnerKey {
            producer: ProducerId([3; 16]),
            stable_id: quanta_index_core::index::StableOwnerId([4; 16]),
        };
        let expected = MapValue::Object(ObjectRef(Digest([7; 32])));
        let replacement = quanta_index_core::index::path_owner_map_value(next)?;
        for (path, incoming_first) in [("a", false), ("b", true)] {
            paths.mark(
                path,
                if incoming_first { next } else { prior },
                !incoming_first,
                &mut work,
            )?;
            paths.set_refs(path, Some(expected), Some(replacement), &mut work)?;
            paths.mark(
                path,
                if incoming_first { prior } else { next },
                incoming_first,
                &mut work,
            )?;
            for duplicate in [prior, next] {
                check!(matches!(
                    paths.mark(path, duplicate, true, &mut work),
                    Err(IndexError::CorruptObject(_))
                ));
                check!(matches!(
                    paths.mark(path, duplicate, false, &mut work),
                    Err(IndexError::InvalidInput(_))
                ));
            }
        }
        let page = paths.page(None, &mut work)?;
        check_eq!(page.as_slice().len(), 2);
        for row in page.as_slice() {
            check_eq!(row.leaving, Some(prior));
            check_eq!(row.incoming, Some(next));
            check_eq!(row.expected, Some(expected));
            check_eq!(row.next, Some(replacement));
        }
        work.request().cancel_handle().cancel();
        check!(matches!(
            paths.mark("c", next, false, &mut work),
            Err(IndexError::Cancelled)
        ));
        let count: i64 = paths
            .connection
            .scoped()?
            .query_row("SELECT COUNT(*) FROM path_edits", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        check_eq!(count, 2);
        Ok(())
    }

    #[test]
    fn path_owner_inline_gold_is_bounded_and_needs_no_heap_grant() -> Result<(), IndexError> {
        let owner = OwnerKey {
            producer: ProducerId([19; 16]),
            stable_id: quanta_index_core::index::StableOwnerId([41; 16]),
        };
        let (work, ledger) = crate::path_staging::tests::budget(0)?;
        let value = quanta_index_core::index::path_owner_map_value(owner)?;
        let mut gold = [19; 32];
        gold.get_mut(16..)
            .ok_or(IndexError::ResourceLimited)?
            .fill(41);
        check_eq!(
            value,
            quanta_index_core::index::MapValue::inline(&gold, None)?
        );
        check_eq!(
            quanta_index_core::index::owner_from_path_map_value(value, &work)?,
            owner
        );
        for wrong in [
            quanta_index_core::index::MapValue::inline(&gold[..31], None)?,
            quanta_index_core::index::MapValue::inline(
                &gold,
                Some(ObjectRef(quanta_index_core::index::Digest([1; 32]))),
            )?,
            quanta_index_core::index::MapValue::Object(ObjectRef(
                quanta_index_core::index::Digest([1; 32]),
            )),
        ] {
            check!(matches!(
                quanta_index_core::index::owner_from_path_map_value(wrong, &work),
                Err(IndexError::CorruptObject(_))
            ));
        }
        work.request().cancel_handle().cancel();
        check!(matches!(
            quanta_index_core::index::owner_from_path_map_value(value, &work),
            Err(IndexError::Cancelled)
        ));
        check_eq!(ledger.live(), 0);
        Ok(())
    }

    #[test]
    fn malformed_path_rows_refuse_before_page_and_string_admission() -> Result<(), IndexError> {
        let (_home, paths) = path_fixture()?;
        let (mut denied, ledger) = crate::path_staging::tests::budget(0)?;
        // Empty SQL output does not need heap backing or a zero-byte lease.
        check!(paths.page(None, &mut denied)?.is_empty());
        for row in [
            "x'ff',NULL,NULL,NULL,NULL",
            "CAST(x'ff' AS TEXT),NULL,NULL,NULL,NULL",
            "'../a',NULL,NULL,NULL,NULL",
            "'a',x'00',NULL,NULL,NULL",
            "'a',NULL,'owner',NULL,NULL",
            "'a',NULL,NULL,x'',NULL",
            "'a',NULL,NULL,NULL,7",
        ] {
            let _rows_changed = paths
                .connection
                .scoped()?
                .execute("DELETE FROM path_edits", [])
                .map_err(|error| scratch_error(&error))?;
            let _rows_changed = paths
                .connection
                .scoped()?
                .execute(
                    &format!(
                        "INSERT INTO path_edits(path,leaving,incoming,expected,next) VALUES({row})"
                    ),
                    [],
                )
                .map_err(|error| scratch_error(&error))?;
            check!(matches!(
                paths.page(None, &mut denied),
                Err(IndexError::CorruptObject(_))
            ));
            check_eq!(ledger.live(), 0);
        }
        let _rows_changed = paths
            .connection
            .scoped()?
            .execute("DELETE FROM path_edits", [])
            .map_err(|error| scratch_error(&error))?;
        let _rows_changed = paths
            .connection
            .scoped()?
            .execute(
                "INSERT INTO path_edits(path) VALUES(?1)",
                params![
                    "a".repeat(
                        MAX_SOURCE_PATH_BYTES
                            .checked_add(1)
                            .ok_or(IndexError::ResourceLimited)?
                    )
                ],
            )
            .map_err(|error| scratch_error(&error))?;
        check!(matches!(
            paths.page(None, &mut denied),
            Err(IndexError::CorruptObject(_))
        ));
        let _rows_changed = paths
            .connection
            .scoped()?
            .execute("DELETE FROM path_edits", [])
            .map_err(|error| scratch_error(&error))?;
        let _rows_changed = paths
            .connection
            .scoped()?
            .execute("INSERT INTO path_edits(path) VALUES('a')", [])
            .map_err(|error| scratch_error(&error))?;
        check!(matches!(
            paths.page(None, &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(ledger.live(), 0);
        Ok(())
    }

    #[test]
    fn path_row_fixed_fields_match_independent_identity_gold() -> Result<(), IndexError> {
        let (_home, paths) = path_fixture()?;
        let mut leaving = [1u8; 32];
        leaving
            .get_mut(16..)
            .ok_or(IndexError::ResourceLimited)?
            .fill(2);
        let mut incoming = [5u8; 32];
        incoming
            .get_mut(16..)
            .ok_or(IndexError::ResourceLimited)?
            .fill(6);
        let _rows_changed = paths.connection.scoped()?.execute(
            "INSERT INTO path_edits(path,leaving,incoming,expected,next) VALUES(?1,?2,?3,?4,?5)",
            params!["a", leaving.as_slice(), incoming.as_slice(), quanta_index_scratch_sqlite::map_value(&MapValue::Object(ObjectRef(Digest([7;32]))))?, quanta_index_scratch_sqlite::map_value(&MapValue::Object(ObjectRef(Digest([8;32]))))?],
        ).map_err(|error| scratch_error(&error))?;
        let (mut budget, ledger) = crate::path_staging::tests::budget(u64::MAX)?;
        let page = paths.page(None, &mut budget)?;
        let row = page
            .as_slice()
            .first()
            .ok_or_else(|| IndexError::BackendViolation("identity row absent".into()))?;
        check_eq!(row.path, "a");
        check_eq!(
            row.leaving,
            Some(OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: quanta_index_core::index::StableOwnerId([2; 16])
            })
        );
        check_eq!(
            row.incoming,
            Some(OwnerKey {
                producer: ProducerId([5; 16]),
                stable_id: quanta_index_core::index::StableOwnerId([6; 16])
            })
        );
        check_eq!(row.expected, Some(ObjectRef(Digest([7; 32])).into()));
        check_eq!(row.next, Some(ObjectRef(Digest([8; 32])).into()));
        drop(page);
        check_eq!(ledger.live(), 0);
        Ok(())
    }

    #[test]
    fn path_pages_keep_position_custody_and_refund_partial_copy_refusal() -> Result<(), IndexError>
    {
        let (_home, paths) = path_fixture()?;
        for ordinal in 0..129 {
            let _rows_changed = paths
                .connection
                .scoped()?
                .execute(
                    "INSERT INTO path_edits(path) VALUES(?1)",
                    params![format!("a{ordinal:03}")],
                )
                .map_err(|error| scratch_error(&error))?;
        }
        let plan: String = paths
            .connection
            .scoped()?
            .query_row(
                &format!("EXPLAIN QUERY PLAN {PATH_PAGE_AFTER_SQL}"),
                params!["a127"],
                |row| row.get(3),
            )
            .map_err(|error| scratch_error(&error))?;
        check!(plan.contains("SEARCH path_edits") && plan.contains("path>?"));
        let (mut budget, ledger) = crate::path_staging::tests::budget(u64::MAX)?;
        let mut page = paths.page(None, &mut budget)?.into_iter();
        let mut after = None;
        for ordinal in 0..128 {
            let row = page
                .next()
                .ok_or_else(|| IndexError::BackendViolation("path page omitted row".into()))?;
            check_eq!(row.path, format!("a{ordinal:03}"));
            after = Some(row);
        }
        check!(page.next().is_none());
        drop(page);
        let retained = ledger.live();
        check!(retained > 0);
        let position = after
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("path position absent".into()))?;
        let final_page = paths.page(Some(&position.path), &mut budget)?;
        check_eq!(final_page.as_slice().len(), 1);
        let last = final_page
            .as_slice()
            .first()
            .ok_or_else(|| IndexError::BackendViolation("last path absent".into()))?;
        check_eq!(last.path, "a128");
        check!(paths.page(Some(&last.path), &mut budget)?.is_empty());
        drop(final_page);
        check_eq!(ledger.live(), retained);
        drop(after);
        check_eq!(ledger.live(), 0);

        // Admit only the slot array: refusing the first string releases that array.
        let cap = u64::try_from(
            128usize
                .checked_mul(std::mem::size_of::<PathRow>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        let (mut denied, ledger) = crate::path_staging::tests::budget(cap)?;
        check!(matches!(
            paths.page(None, &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(ledger.live(), 0);
        // Admit one row too: a later refusal must refund the copied prefix.
        let (probe, ledger) = crate::path_staging::tests::budget(u64::MAX)?;
        let row = PathRow::copy("a000", None, None, None, None, &probe)?;
        let one_row = ledger.live();
        drop(row);
        check_eq!(ledger.live(), 0);
        let (mut denied, ledger) = crate::path_staging::tests::budget(
            cap.checked_add(one_row)
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        check!(matches!(
            paths.page(None, &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(ledger.live(), 0);
        Ok(())
    }

    #[test]
    fn path_bound_leaves_room_for_canonical_key_namespace() -> Result<(), IndexError> {
        let maximum = "a".repeat(MAX_SOURCE_PATH_BYTES);
        validate_path(&maximum)?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or(IndexError::ResourceLimited)?;
        let budget = WorkBudget::new(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 10_000,
                mutations: 1,
            },
            deadline,
        );
        let key_bytes = PathKey::copy(&maximum, &budget)?.as_slice().len();
        if key_bytes != 4096 {
            return Err(IndexError::BackendViolation(format!(
                "maximum source path key must be 4096 bytes, got {key_bytes}"
            )));
        }
        let beyond_maximum = validate_path(&format!("{maximum}a"));
        if !matches!(&beyond_maximum, Err(IndexError::InvalidInput(_))) {
            return Err(IndexError::BackendViolation(format!(
                "source path over the maximum was not refused as invalid input: {beyond_maximum:?}"
            )));
        }
        Ok(())
    }

    #[test]
    fn path_scratch_uses_file_with_page_cap() -> Result<(), IndexError> {
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
        let scratch = PathScratch::open(&scratch_space, &mut budget)?;
        let page_size: u32 = scratch
            .connection
            .scoped()?
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        let max_pages: u32 = scratch
            .connection
            .scoped()?
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        let temp_store: u32 = scratch
            .connection
            .scoped()?
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        if page_size != SCRATCH_PAGE_SIZE {
            return Err(IndexError::BackendViolation(format!(
                "source scratch page size differs: {page_size}"
            )));
        }
        let maximum_page_bytes = u64::from(max_pages)
            .checked_mul(u64::from(page_size))
            .ok_or(IndexError::ResourceLimited)?;
        let reserved_bytes = scratch.allocation.reserved_bytes()?;
        if maximum_page_bytes > reserved_bytes || reserved_bytes < SCRATCH_INITIAL_BYTES {
            return Err(IndexError::BackendViolation(format!(
                "source scratch page cap {maximum_page_bytes} exceeds reserved {reserved_bytes}, or reservation is below initial {SCRATCH_INITIAL_BYTES}"
            )));
        }
        if temp_store != 1 {
            return Err(IndexError::BackendViolation(format!(
                "source scratch temp_store must be FILE (1), got {temp_store}"
            )));
        }
        Ok(())
    }
}
