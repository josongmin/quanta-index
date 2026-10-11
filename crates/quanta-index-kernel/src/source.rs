use std::collections::{BTreeSet, VecDeque};

use rusqlite::{Connection, ErrorCode, params};
use tempfile::NamedTempFile;

use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MAX_SOURCE_PATH_BYTES, MapEdit, MapRoot, ObjectBatchInput,
    ObjectRef, OperationKey, OrdinalMapEdit, OwnerKey, OwnerMutationCursor, SourceCut, SourceDelta,
    SourceEvent, SourceRecord, SourceTransition, WorkUsage, owner_map_key, ports::MapEditCursor,
};

fn path_key(path: &str) -> Vec<u8> {
    let mut key = b"path\0".to_vec();
    key.extend_from_slice(path.as_bytes());
    key
}

fn frontier_key(event: SourceEvent) -> Vec<u8> {
    let mut key = b"frontier\0".to_vec();
    key.extend_from_slice(&event.producer.0);
    key.extend_from_slice(&event.stream.0);
    key
}

fn decode<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    what: &'static str,
) -> Result<T, IndexError> {
    serde_json::from_slice(bytes)
        .map_err(|error| IndexError::CorruptObject(format!("{what}: {error}")))
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(value)
        .map_err(|error| IndexError::InvalidInput(format!("immutable value encoding: {error}")))
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
) -> Result<SourceRecord, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "source record object reference mismatch".into(),
        ));
    }
    let record: SourceRecord = decode(&stored.payload, "source record")?;
    validate_path(&record.path)?;
    if stored.registration.children.as_slice() != [record.bytes] {
        return Err(IndexError::CorruptObject(
            "source record must directly own exactly its byte object".into(),
        ));
    }
    // Next record byte objects were fully verified and adopted atomically at
    // admission. Prior record bytes are protected by the exact base ticket.
    Ok(record)
}

fn read_path_owner(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<OwnerKey, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference || !stored.registration.children.is_empty() {
        return Err(IndexError::CorruptObject(
            "invalid path owner marker".into(),
        ));
    }
    decode(&stored.payload, "path owner marker")
}

fn read_frontier(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceEvent, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference || !stored.registration.children.is_empty() {
        return Err(IndexError::CorruptObject("invalid frontier marker".into()));
    }
    decode(&stored.payload, "source frontier")
}

const SCRATCH_PAGE_SIZE: u32 = 4096;
const SCRATCH_MAX_PAGES: u32 = 32768;

fn scratch_error(error: &rusqlite::Error) -> IndexError {
    if matches!(error, rusqlite::Error::SqliteFailure(failure, _) if failure.code == ErrorCode::DiskFull)
    {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("source path scratch: {error}"))
    }
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

#[derive(Debug)]
struct PathRow {
    path: String,
    leaving: Option<Vec<u8>>,
    incoming: Option<Vec<u8>>,
    expected: Option<Vec<u8>>,
    next: Option<Vec<u8>>,
}

/// Attempt-local disk spill without publication authority.
///
/// The unique path key sees all mutation pages, making a cross-page swap and
/// a duplicate target distinguishable. The catalog CAS owns publication.
struct PathScratch {
    connection: Connection,
    _file: NamedTempFile,
}

impl PathScratch {
    fn open() -> Result<Self, IndexError> {
        let file = NamedTempFile::new()
            .map_err(|error| IndexError::Storage(format!("source path scratch: {error}")))?;
        let connection = Connection::open(file.path()).map_err(|error| scratch_error(&error))?;
        connection
            .execute_batch(
                "PRAGMA page_size=4096; PRAGMA max_page_count=32768; PRAGMA journal_mode=OFF; \
                 PRAGMA synchronous=OFF; PRAGMA temp_store=FILE; PRAGMA mmap_size=0; \
                 PRAGMA cache_size=-1024; CREATE TABLE path_edits( \
                 path TEXT PRIMARY KEY, leaving BLOB, incoming BLOB, expected BLOB, next BLOB);",
            )
            .map_err(|error| scratch_error(&error))?;
        let page_size: u32 = connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        let max_pages: u32 = connection
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        if page_size != SCRATCH_PAGE_SIZE || max_pages != SCRATCH_MAX_PAGES {
            return Err(IndexError::Storage(
                "source path scratch page limit was not installed".into(),
            ));
        }
        Ok(Self {
            connection,
            _file: file,
        })
    }

    fn mark(&self, path: &str, owner: OwnerKey, leaving: bool) -> Result<(), IndexError> {
        let _inserted = self
            .connection
            .execute(
                "INSERT OR IGNORE INTO path_edits(path) VALUES(?1)",
                params![path],
            )
            .map_err(|error| scratch_error(&error))?;
        let field = if leaving { "leaving" } else { "incoming" };
        let query = format!("UPDATE path_edits SET {field}=?2 WHERE path=?1 AND {field} IS NULL");
        let changed = self
            .connection
            .execute(&query, params![path, owner_bytes(owner).as_slice()])
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

    fn page(&self, after: Option<&str>) -> Result<Vec<PathRow>, IndexError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT path,leaving,incoming,expected,next FROM path_edits \
                 WHERE (?1 IS NULL OR path > ?1) ORDER BY path LIMIT 128",
            )
            .map_err(|error| scratch_error(&error))?;
        let rows = statement
            .query_map(params![after], |row| {
                Ok(PathRow {
                    path: row.get(0)?,
                    leaving: row.get(1)?,
                    incoming: row.get(2)?,
                    expected: row.get(3)?,
                    next: row.get(4)?,
                })
            })
            .map_err(|error| scratch_error(&error))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| scratch_error(&error))
    }

    fn set_refs(
        &self,
        path: &str,
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
    ) -> Result<(), IndexError> {
        let expected = expected.map(|reference| reference.0.0);
        let next = next.map(|reference| reference.0.0);
        let changed = self
            .connection
            .execute(
                "UPDATE path_edits SET expected=?2,next=?3 WHERE path=?1",
                params![
                    path,
                    expected.as_ref().map(<[u8; 32]>::as_slice),
                    next.as_ref().map(<[u8; 32]>::as_slice)
                ],
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

fn object_from_bytes(bytes: Option<&[u8]>) -> Result<Option<ObjectRef>, IndexError> {
    bytes
        .map(|value| {
            let digest: [u8; 32] = value.try_into().map_err(|_error| {
                IndexError::CorruptObject("source path scratch reference length".into())
            })?;
            Ok(ObjectRef(Digest(digest)))
        })
        .transpose()
}

fn validate_events(
    operation: OperationKey,
    base_frontiers: MapRoot,
    events: &[SourceEvent],
    context: &mut AttemptContext<'_>,
) -> Result<Vec<Option<ObjectRef>>, IndexError> {
    if events.is_empty() {
        return Err(IndexError::InvalidInput(
            "source advance has no event".into(),
        ));
    }
    let mut expected_frontiers = Vec::with_capacity(events.len());
    let mut last: Option<(
        quanta_index_core::index::ProducerId,
        quanta_index_core::index::StreamId,
    )> = None;
    let mut operation_event = false;
    for event in events {
        let pair = (event.producer, event.stream);
        if last.is_some_and(|previous| previous >= pair) {
            return Err(IndexError::InvalidInput(
                "source events must be unique and sorted by producer/stream".into(),
            ));
        }
        last = Some(pair);
        if pair == (operation.producer, operation.stream)
            && event.stream_epoch == operation.stream_epoch
        {
            operation_event = true;
        }
        let key = frontier_key(*event);
        let expected = context.maps.lookup(base_frontiers, &key, context.budget)?;
        match expected {
            Some(reference) => {
                let previous = read_frontier(reference, context)?;
                if previous.producer != event.producer || previous.stream != event.stream {
                    return Err(IndexError::CorruptObject(
                        "source frontier key mismatch".into(),
                    ));
                }
                // An existing stream cannot silently reset its epoch. That
                // requires an explicit complete reconciliation contract.
                let published_sequence = previous.sequence;
                if previous.stream_epoch != event.stream_epoch
                    || event.predecessor != published_sequence
                {
                    return Err(IndexError::Conflict);
                }
            }
            None if event.predecessor != 0 => return Err(IndexError::Conflict),
            None => {}
        }
        if event.sequence
            != event
                .predecessor
                .checked_add(1)
                .ok_or_else(|| IndexError::InvalidInput("source sequence overflow".into()))?
        {
            return Err(IndexError::Conflict);
        }
        expected_frontiers.push(expected);
    }
    if !operation_event {
        return Err(IndexError::InvalidInput(
            "operation key is not bound to a source event".into(),
        ));
    }
    Ok(expected_frontiers)
}

fn stage_events(
    events: &[SourceEvent],
    expected_frontiers: &[Option<ObjectRef>],
    context: &mut AttemptContext<'_>,
) -> Result<Vec<MapEdit>, IndexError> {
    if events.len() != expected_frontiers.len() {
        return Err(IndexError::BackendViolation(
            "source frontier validation lost an event".into(),
        ));
    }
    let mut edits = Vec::with_capacity(events.len());
    for (event_page, expected_page) in events.chunks(64).zip(expected_frontiers.chunks(64)) {
        let mut inputs = Vec::with_capacity(event_page.len());
        let mut predicted = Vec::with_capacity(event_page.len());
        for event in event_page {
            let payload = encode(event)?;
            predicted.push(
                context
                    .objects
                    .identify(&payload, &[], context.budget)?
                    .reference,
            );
            inputs.push(ObjectBatchInput {
                payload,
                children: Vec::new(),
            });
        }
        let installed = context.put_batch(&inputs)?;
        if installed.len() != event_page.len() || predicted.len() != event_page.len() {
            return Err(IndexError::BackendViolation(
                "source frontier batch lost an ordinal result".into(),
            ));
        }
        for ((event, expected), (next, predicted)) in event_page
            .iter()
            .zip(expected_page)
            .zip(installed.into_iter().zip(predicted))
        {
            if next != predicted {
                return Err(IndexError::BackendViolation(
                    "source frontier batch identity or order mismatch".into(),
                ));
            }
            edits.push(MapEdit {
                key: frontier_key(*event),
                expected: *expected,
                next: Some(next),
            });
        }
    }
    if edits.len() != events.len() {
        return Err(IndexError::BackendViolation(
            "source frontier edits omit an event".into(),
        ));
    }
    Ok(edits)
}

struct SourceEditCursor<'a> {
    owners: OwnerMutationCursor<'a>,
    paths: &'a PathScratch,
    path_page: VecDeque<PathRow>,
    after_path: Option<String>,
    frontier_edits: VecDeque<MapEdit>,
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
                        return Ok(Some(OrdinalMapEdit {
                            root: 0,
                            edit: MapEdit {
                                key: owner_map_key(mutation.owner),
                                expected: mutation.expected,
                                next: mutation.next,
                            },
                        }));
                    }
                    None => self.phase = 1,
                },
                1 => {
                    if self.path_page.is_empty() {
                        self.path_page = self.paths.page(self.after_path.as_deref())?.into();
                        if self.path_page.is_empty() {
                            self.phase = 2;
                            continue;
                        }
                    }
                    let row = self.path_page.pop_front().ok_or_else(|| {
                        IndexError::BackendViolation("source path page disappeared".into())
                    })?;
                    self.after_path = Some(row.path.clone());
                    let expected = object_from_bytes(row.expected.as_deref())?;
                    let next = object_from_bytes(row.next.as_deref())?;
                    if expected != next {
                        return Ok(Some(OrdinalMapEdit {
                            root: 1,
                            edit: MapEdit {
                                key: path_key(&row.path),
                                expected,
                                next,
                            },
                        }));
                    }
                }
                _ => {
                    return Ok(self
                        .frontier_edits
                        .pop_front()
                        .map(|edit| OrdinalMapEdit { root: 2, edit }));
                }
            }
        }
    }
}

pub(crate) fn apply_source(
    operation: OperationKey,
    base: SourceCut,
    transition: &SourceTransition,
    context: &mut AttemptContext<'_>,
) -> Result<SourceDelta, IndexError> {
    if base != transition.expected_source {
        return Err(IndexError::Conflict);
    }
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
    context.budget.charge(WorkUsage {
        mutations: u64::try_from(transition.events.len())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let scratch = PathScratch::open()?;
    let event_producers: BTreeSet<_> = transition
        .events
        .iter()
        .map(|event| event.producer)
        .collect();
    let mut mutations = OwnerMutationCursor::new(transition.owners, context.maps, context.objects);
    while let Some(mutation) = mutations.next_mutation(context.budget)? {
        if !event_producers.contains(&mutation.owner.producer) {
            return Err(IndexError::InvalidInput(
                "mutated owner has no source event".into(),
            ));
        }
        let key = owner_map_key(mutation.owner);
        let actual = context.maps.lookup(base.owners, &key, context.budget)?;
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
            let key = path_key(&prior.path);
            let marker = context.maps.lookup(base.paths, &key, context.budget)?;
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
                scratch.mark(path, mutation.owner, true)?;
            }
            if let Some(path) = new_path {
                scratch.mark(path, mutation.owner, false)?;
            }
        }
    }

    let mut after = None;
    loop {
        let page = scratch.page(after.as_deref())?;
        if page.is_empty() {
            break;
        }
        for row in page {
            let key = path_key(&row.path);
            let expected = context.maps.lookup(base.paths, &key, context.budget)?;
            let current_owner = expected
                .map(|reference| read_path_owner(reference, context))
                .transpose()?;
            if let Some(leaver) = row.leaving.as_deref() {
                if current_owner != Some(owner_from_bytes(leaver)?) {
                    return Err(IndexError::CorruptObject(
                        "owner/path reverse entry mismatch".into(),
                    ));
                }
            } else if current_owner.is_some() {
                return Err(IndexError::Conflict);
            }
            scratch.set_refs(&row.path, expected, None)?;
            after = Some(row.path);
        }
    }

    // Complete all base-path and frontier validation before installing any
    // path or frontier marker, including a collision in a later scratch page.
    let expected_frontiers =
        validate_events(operation, base.frontiers, &transition.events, context)?;

    let mut after = None;
    loop {
        let page = scratch.page(after.as_deref())?;
        if page.is_empty() {
            break;
        }
        for rows in page.chunks(64) {
            let mut inputs = Vec::new();
            let mut predicted = Vec::new();
            for row in rows {
                if let Some(owner) = row.incoming.as_deref() {
                    let payload = encode(&owner_from_bytes(owner)?)?;
                    predicted.push(
                        context
                            .objects
                            .identify(&payload, &[], context.budget)?
                            .reference,
                    );
                    inputs.push(ObjectBatchInput {
                        payload,
                        children: Vec::new(),
                    });
                }
            }
            let installed = if inputs.is_empty() {
                Vec::new()
            } else {
                context.put_batch(&inputs)?
            };
            if installed.len() != inputs.len() || predicted.len() != inputs.len() {
                return Err(IndexError::BackendViolation(
                    "source path batch lost an ordinal result".into(),
                ));
            }
            let mut installed = installed.into_iter().zip(predicted);
            for row in rows {
                let next = if row.incoming.is_some() {
                    let (next, expected_identity) = installed.next().ok_or_else(|| {
                        IndexError::BackendViolation("source path batch lost an object".into())
                    })?;
                    if next != expected_identity {
                        return Err(IndexError::BackendViolation(
                            "source path batch identity or order mismatch".into(),
                        ));
                    }
                    Some(next)
                } else {
                    None
                };
                scratch.set_refs(&row.path, object_from_bytes(row.expected.as_deref())?, next)?;
                after = Some(row.path.clone());
            }
            if installed.next().is_some() {
                return Err(IndexError::BackendViolation(
                    "source path batch returned an extra object".into(),
                ));
            }
        }
    }

    // Next frontier objects share the attempt's custody and are installed
    // only after every immutable-base precondition has passed.
    let frontier_edits = stage_events(&transition.events, &expected_frontiers, context)?;
    let mut edits = SourceEditCursor {
        owners: OwnerMutationCursor::new(transition.owners, context.maps, context.objects),
        paths: &scratch,
        path_page: VecDeque::new(),
        after_path: None,
        frontier_edits: frontier_edits.into(),
        phase: 0,
    };
    let roots = context.apply_stream(&[base.owners, base.paths, base.frontiers], &mut edits)?;
    let [owners, paths, frontiers]: [MapRoot; 3] = roots.try_into().map_err(|_roots| {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_bound_leaves_room_for_canonical_key_namespace() -> Result<(), IndexError> {
        let maximum = "a".repeat(MAX_SOURCE_PATH_BYTES);
        validate_path(&maximum)?;
        assert_eq!(path_key(&maximum).len(), 4096);
        assert!(matches!(
            validate_path(&format!("{maximum}a")),
            Err(IndexError::InvalidInput(_))
        ));
        Ok(())
    }

    #[test]
    fn path_scratch_uses_file_with_page_cap() -> Result<(), IndexError> {
        let scratch = PathScratch::open()?;
        let page_size: u32 = scratch
            .connection
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        let max_pages: u32 = scratch
            .connection
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        let temp_store: u32 = scratch
            .connection
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .map_err(|error| scratch_error(&error))?;
        assert_eq!(page_size, SCRATCH_PAGE_SIZE);
        assert_eq!(max_pages, SCRATCH_MAX_PAGES);
        assert_eq!(temp_store, 1);
        Ok(())
    }
}
