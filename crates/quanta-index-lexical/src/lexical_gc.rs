//! Shared lexical object collection.
//!
//! Generation enumeration and registry pins
//! remain with the adapter lifecycle owner; this module consumes its complete
//! retained directory set and refuses an incomplete protected closure.

use std::path::{Path, PathBuf};
use std::time::Instant;

use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticCounterV1, DiagnosticStageV1, DiagnosticTerminalV1,
    RequestBudgetV1,
};
#[cfg(test)]
use quanta_index_core::{DiagnosticHealthCodeV1, DiagnosticLaneV1};
use rusqlite::{Connection, OptionalExtension as _, Statement, params};

use crate::file_authority::paged::AuthorityRoot;
use crate::lexical_store::{
    AdoptedRootAdvance, AdoptedRootCursor, ObjectInventoryAdvance, ObjectInventoryCursor,
    ObjectKind, ObjectRef, ObjectStore,
};

use crate::causal_profile::ClosureReadTotalsV1;
use crate::text_authority::NativeRoot;

const SWEEP_BATCH: usize = 64;
const MARK_BATCH: usize = 256;
const MAX_STEP_UNITS: usize = 256;
const MAX_REACH_DEPTH: u16 = 128;
const MAX_REACH_FRONTIER: usize = 32 * 1024;

fn db_error(context: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("lexical GC scratch {context}: {error}"))
}

/// Fixed statement bytecode local to one closure measurement or bounded GC
/// step. Reuse does not cache rows, descriptors or object authentication.
struct ReferenceStatements<'db> {
    insert: Statement<'db>,
    existing: Statement<'db>,
    expand: Statement<'db>,
}

impl<'db> ReferenceStatements<'db> {
    fn new(db: &'db Connection) -> Result<Self, CoreError> {
        Ok(Self {
            insert: db
                .prepare("INSERT OR IGNORE INTO refs(sha,kind,bytes,expanded) VALUES (?1,?2,?3,0)")
                .map_err(|error| db_error("prepare reference insert", error))?,
            existing: db
                .prepare("SELECT kind,bytes FROM refs WHERE sha=?1")
                .map_err(|error| db_error("prepare duplicate reference", error))?,
            expand: db
                .prepare("UPDATE refs SET expanded=1 WHERE sha=?1 AND expanded=0")
                .map_err(|error| db_error("prepare expanded reference", error))?,
        })
    }

    fn insert(&mut self, reference: &ObjectRef) -> Result<(), CoreError> {
        let bytes = i64::try_from(reference.bytes)
            .map_err(|error| db_error("reference byte width", error))?;
        let inserted = self
            .insert
            .execute(params![
                reference.sha256.as_slice(),
                i64::from(reference.kind.code()),
                bytes
            ])
            .map_err(|error| db_error("insert reference", error))?;
        if inserted == 0 {
            let existing: (i64, i64) = self
                .existing
                .query_row(params![reference.sha256.as_slice()], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|error| db_error("read duplicate reference", error))?;
            if existing != (i64::from(reference.kind.code()), bytes) {
                return Err(CoreError::InvalidContract(
                    "lexical GC: one object digest has conflicting kind or size".into(),
                ));
            }
        }
        Ok(())
    }

    fn expand(&mut self, reference: &ObjectRef) -> Result<usize, CoreError> {
        let updated = self
            .expand
            .execute(params![reference.sha256.as_slice()])
            .map_err(|error| db_error("mark expanded reference", error))?;
        Ok(updated)
    }
}

fn observed_sql<T>(
    enabled: bool,
    totals: &mut ClosureReadTotalsV1,
    action: impl FnOnce() -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    let started = enabled.then(Instant::now);
    let result = action();
    if let Some(started) = started {
        ClosureReadTotalsV1::accumulate_elapsed(
            &mut totals.sqlite_elapsed_ns,
            Some(started),
            &mut totals.overflowed,
            ClosureReadTotalsV1::SQLITE_ELAPSED,
        );
    }
    result
}

fn read_mark_batch(db: &Connection, limit: usize) -> Result<Vec<ObjectRef>, CoreError> {
    let mut ignored_totals = ClosureReadTotalsV1::default();
    read_mark_batch_observed(db, limit, false, &mut ignored_totals)
}

fn read_mark_batch_observed(
    db: &Connection,
    limit: usize,
    observe_sql: bool,
    totals: &mut ClosureReadTotalsV1,
) -> Result<Vec<ObjectRef>, CoreError> {
    let mut statement = observed_sql(observe_sql, totals, || {
        db.prepare("SELECT sha,kind,bytes FROM refs WHERE expanded=0 ORDER BY sha LIMIT ?1")
            .map_err(|error| db_error("prepare mark batch", error))
    })?;
    let batch_limit = limit.min(MARK_BATCH);
    let query_limit = i64::try_from(batch_limit).map_err(|error| db_error("mark bound", error))?;
    let mut rows = observed_sql(observe_sql, totals, || {
        statement
            .query(params![query_limit])
            .map_err(|error| db_error("query mark batch", error))
    })?;
    let mut batch = Vec::new();
    batch
        .try_reserve_exact(batch_limit)
        .map_err(|error| db_error("reserve mark batch", error))?;
    loop {
        let raw = observed_sql(observe_sql, totals, || {
            let Some(row) = rows
                .next()
                .map_err(|error| db_error("next mark row", error))?
            else {
                return Ok(None);
            };
            let sha: Vec<u8> = row.get(0).map_err(|error| db_error("mark digest", error))?;
            let kind: i64 = row.get(1).map_err(|error| db_error("mark kind", error))?;
            let bytes: i64 = row.get(2).map_err(|error| db_error("mark width", error))?;
            Ok(Some((sha, kind, bytes)))
        })?;
        let Some((sha, code, width)) = raw else {
            break;
        };
        let sha256: [u8; 32] = sha.try_into().map_err(|_conversion_error| {
            CoreError::InvalidContract("lexical GC scratch digest width".into())
        })?;
        let code = u8::try_from(code).map_err(|_conversion_error| {
            CoreError::InvalidContract("lexical GC scratch object kind width".into())
        })?;
        let kind = ObjectKind::from_code(code)
            .ok_or_else(|| CoreError::InvalidContract("lexical GC scratch object kind".into()))?;
        let bytes = u64::try_from(width).map_err(|_conversion_error| {
            CoreError::InvalidContract("lexical GC scratch object width".into())
        })?;
        batch.push(ObjectRef {
            kind,
            bytes,
            sha256,
        });
    }
    Ok(batch)
}

fn read_candidate_batch(
    db: &Connection,
    after: &[u8],
    limit: usize,
) -> Result<Vec<[u8; 32]>, CoreError> {
    let mut statement = db
        .prepare("SELECT sha FROM candidates WHERE sha>?1 ORDER BY sha LIMIT ?2")
        .map_err(|error| db_error("prepare candidate batch", error))?;
    let mut rows = statement
        .query(params![
            after,
            i64::try_from(limit.min(SWEEP_BATCH))
                .map_err(|error| db_error("sweep bound", error))?
        ])
        .map_err(|error| db_error("query candidate batch", error))?;
    let mut batch = Vec::new();
    batch
        .try_reserve_exact(limit.min(SWEEP_BATCH))
        .map_err(|error| db_error("reserve sweep batch", error))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| db_error("next candidate", error))?
    {
        let sha: Vec<u8> = row
            .get(0)
            .map_err(|error| db_error("candidate digest", error))?;
        batch.push(sha.try_into().map_err(|_conversion_error| {
            CoreError::InvalidContract("lexical GC scratch candidate width".into())
        })?);
    }
    Ok(batch)
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SharedPoolGcOutcome {
    Completed { marked: u64, examined: u64 },
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SharedGcStep {
    Paused,
    Completed { marked: u64, examined: u64 },
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GcPhase {
    Adopted,
    Protected,
    Mark,
    Inventory,
    Sweep,
}

/// Adapter-owned in-process pass. The temporary database is removed on drop;
/// a restart begins a fresh complete mark before it may delete any object.
pub(crate) struct SharedGcProgress {
    store: ObjectStore,
    _scratch: tempfile::TempDir,
    db: Connection,
    retained: Vec<PathBuf>,
    snapshot_epoch: u64,
    phase: GcPhase,
    adopted: Option<AdoptedRootCursor>,
    protected_index: usize,
    inventory: Option<ObjectInventoryCursor>,
    sweep_after: Vec<u8>,
    marked: u64,
    examined: u64,
}

/// Read both sealed roots, including for a quarantined but still retained
/// generation. A damaged protected root aborts collection; quarantine is not
/// permission to drop its object closure.
pub(crate) fn protected_authorities(
    directory: &Path,
) -> Result<(AuthorityRoot, NativeRoot), CoreError> {
    let generation =
        crate::sealed_generation::open_generation_dir_nofollow(directory).map_err(|error| {
            CoreError::Storage(format!("lexical GC: pin {}: {error}", directory.display()))
        })?;
    let identity = crate::index_store::read_lexical_sealed_identity_at(directory, &generation)?;
    let manifest = crate::sealed_generation::read_bound_manifest_at(
        directory,
        &generation,
        &identity.manifest_digest,
    )?;
    let file = manifest.file_authority.first().ok_or_else(|| {
        crate::index_store::sidecar_corrupt(
            directory,
            crate::lexical_root_names::FILE_ROOT_NAME,
            "committed File root absent",
        )
    })?;
    let native = manifest
        .text_authority
        .as_ref()
        .and_then(|files| files.first())
        .ok_or_else(|| {
            crate::index_store::sidecar_corrupt(
                directory,
                crate::lexical_root_names::NATIVE_ROOT_NAME,
                "committed Native root absent",
            )
        })?;
    let file_bytes = crate::sealed_generation::read_committed(&generation, directory, file, None)?;
    let native_bytes =
        crate::sealed_generation::read_committed(&generation, directory, native, None)?;
    let file_root = AuthorityRoot::decode(&file_bytes)?;
    let native_root: NativeRoot = crate::channel_payloads::decode_cbor_exact(&native_bytes)
        .map_err(|error| {
            crate::index_store::sidecar_corrupt(
                directory,
                crate::lexical_root_names::NATIVE_ROOT_NAME,
                &error,
            )
        })?;
    native_root.validate()?;
    if crate::channel_payloads::encode_cbor(&native_root, "Native root")? != native_bytes {
        return Err(crate::index_store::sidecar_corrupt(
            directory,
            crate::lexical_root_names::NATIVE_ROOT_NAME,
            "noncanonical root",
        ));
    }
    Ok((file_root, native_root))
}

pub(crate) fn protected_roots(directory: &Path) -> Result<Vec<ObjectRef>, CoreError> {
    let (file_root, native_root) = protected_authorities(directory)?;
    let mut roots = file_root.object_roots();
    roots.extend(native_root.root_objects());
    Ok(roots)
}

/// Exact physical bytes of one selected sealed root set.
///
/// The scratch table
/// deduplicates by object digest across generations; every referenced object
/// is authenticated before its encoded length is charged. This reads bounded
/// physical objects, never decodes source text or postings into a corpus map.
pub(crate) fn measure_unique_closure_bytes(
    store: &ObjectStore,
    roots: impl IntoIterator<Item = ObjectRef>,
    diagnostic: Option<&DiagnosticContextV1>,
) -> Result<u64, CoreError> {
    let span = diagnostic.and_then(|context| {
        crate::causal_profile::enter_span(context, DiagnosticStageV1::ClosureRead)
    });
    let mut totals = ClosureReadTotalsV1::default();
    let mut sqlite_batches = 0_u64;
    let mut sqlite_rows = 0_u64;
    let observe_sql = span.is_some();
    let mut unique_closure_refs = 0_u64;
    let mut closure_dependency_edges = 0_u64;
    let mut closure_authenticated_bytes = 0_u64;
    let result = (|| {
        let scratch =
            tempfile::tempdir().map_err(|error| db_error("create measure scratch", error))?;
        let db = Connection::open(scratch.path().join("lexical-measure.sqlite"))
            .map_err(|error| db_error("open measure scratch", error))?;
        observed_sql(observe_sql, &mut totals, || {
            db.execute_batch(
                "PRAGMA journal_mode=OFF;
         PRAGMA synchronous=OFF;
         PRAGMA temp_store=FILE;
         PRAGMA cache_size=-2048;
         CREATE TABLE refs(sha BLOB PRIMARY KEY, kind INTEGER NOT NULL,
             bytes INTEGER NOT NULL, expanded INTEGER NOT NULL) WITHOUT ROWID;
         CREATE INDEX refs_pending ON refs(expanded,sha);",
            )
            .map_err(|error| db_error("initialize measure table", error))
        })?;
        let mut statements =
            observed_sql(observe_sql, &mut totals, || ReferenceStatements::new(&db))?;
        for root in roots {
            observed_sql(observe_sql, &mut totals, || statements.insert(&root))?;
        }
        loop {
            if observe_sql {
                ClosureReadTotalsV1::accumulate(
                    &mut sqlite_batches,
                    Some(1),
                    &mut totals.overflowed,
                    ClosureReadTotalsV1::SQLITE_BATCHES,
                );
            }
            let batch = read_mark_batch_observed(&db, MARK_BATCH, observe_sql, &mut totals)?;
            if batch.is_empty() {
                break;
            }
            observed_sql(observe_sql, &mut totals, || {
                db.execute_batch("BEGIN IMMEDIATE")
                    .map_err(|error| db_error("begin closure batch", error))
            })?;
            let mut batch_expanded_rows = 0_u64;
            for reference in batch {
                let children = if observe_sql {
                    store.object_dependencies_for_closure(&reference, &mut totals)?
                } else {
                    store.object_dependencies(&reference, None)?
                };
                if observe_sql {
                    ClosureReadTotalsV1::accumulate(
                        &mut unique_closure_refs,
                        Some(1),
                        &mut totals.overflowed,
                        ClosureReadTotalsV1::UNIQUE_REFS,
                    );
                    ClosureReadTotalsV1::accumulate_width(
                        &mut closure_dependency_edges,
                        children.len(),
                        &mut totals.overflowed,
                        ClosureReadTotalsV1::DEPENDENCY_EDGES,
                    );
                    ClosureReadTotalsV1::accumulate(
                        &mut closure_authenticated_bytes,
                        Some(reference.bytes),
                        &mut totals.overflowed,
                        ClosureReadTotalsV1::AUTHENTICATED_BYTES,
                    );
                }
                for child in children {
                    observed_sql(observe_sql, &mut totals, || statements.insert(&child))?;
                }
                let updated =
                    observed_sql(observe_sql, &mut totals, || statements.expand(&reference))?;
                if observe_sql {
                    ClosureReadTotalsV1::accumulate_width(
                        &mut batch_expanded_rows,
                        updated,
                        &mut totals.overflowed,
                        ClosureReadTotalsV1::SQLITE_ROWS,
                    );
                }
            }
            observed_sql(observe_sql, &mut totals, || {
                db.execute_batch("COMMIT")
                    .map_err(|error| db_error("commit closure batch", error))
            })?;
            if observe_sql {
                ClosureReadTotalsV1::accumulate(
                    &mut sqlite_rows,
                    Some(batch_expanded_rows),
                    &mut totals.overflowed,
                    ClosureReadTotalsV1::SQLITE_ROWS,
                );
            }
        }
        let mut statement = observed_sql(observe_sql, &mut totals, || {
            db.prepare("SELECT bytes FROM refs ORDER BY sha")
                .map_err(|error| db_error("prepare closure lengths", error))
        })?;
        let mut rows = observed_sql(observe_sql, &mut totals, || {
            statement
                .query([])
                .map_err(|error| db_error("read closure lengths", error))
        })?;
        let mut total = 0_u64;
        loop {
            let raw = observed_sql(observe_sql, &mut totals, || {
                let Some(row) = rows
                    .next()
                    .map_err(|error| db_error("next closure length", error))?
                else {
                    return Ok(None);
                };
                let raw: i64 = row
                    .get(0)
                    .map_err(|error| db_error("closure length", error))?;
                Ok(Some(raw))
            })?;
            let Some(raw) = raw else {
                break;
            };
            let bytes = u64::try_from(raw).map_err(|_conversion_error| {
                CoreError::InvalidContract("lexical retention object length width".into())
            })?;
            total = total.checked_add(bytes).ok_or_else(|| {
                CoreError::InvalidContract("lexical retention closure byte overflow".into())
            })?;
        }
        Ok(total)
    })();
    if let (Some(context), Some(span)) = (diagnostic, span) {
        totals.publish_overflow_health(context);
        for (key, value, field) in [
            (
                DiagnosticCounterV1::UniqueClosureRefs,
                unique_closure_refs,
                ClosureReadTotalsV1::UNIQUE_REFS,
            ),
            (
                DiagnosticCounterV1::ClosureDependencyEdges,
                closure_dependency_edges,
                ClosureReadTotalsV1::DEPENDENCY_EDGES,
            ),
            (
                DiagnosticCounterV1::ClosureAuthenticatedBytes,
                closure_authenticated_bytes,
                ClosureReadTotalsV1::AUTHENTICATED_BYTES,
            ),
            (
                DiagnosticCounterV1::FilesRead,
                totals.objects_read,
                ClosureReadTotalsV1::OBJECTS_READ,
            ),
            (
                DiagnosticCounterV1::BytesRead,
                totals.bytes_read,
                ClosureReadTotalsV1::BYTES_READ,
            ),
            (
                DiagnosticCounterV1::FilesHashed,
                totals.objects_hashed,
                ClosureReadTotalsV1::OBJECTS_HASHED,
            ),
            (
                DiagnosticCounterV1::BytesHashed,
                totals.bytes_hashed,
                ClosureReadTotalsV1::BYTES_HASHED,
            ),
            (
                DiagnosticCounterV1::FilesDecoded,
                totals.objects_decoded,
                ClosureReadTotalsV1::OBJECTS_DECODED,
            ),
            (
                DiagnosticCounterV1::BytesDecoded,
                totals.bytes_decoded,
                ClosureReadTotalsV1::BYTES_DECODED,
            ),
            (
                DiagnosticCounterV1::ClosureReadElapsedNs,
                totals.read_elapsed_ns,
                ClosureReadTotalsV1::READ_ELAPSED,
            ),
            (
                DiagnosticCounterV1::ClosureHashElapsedNs,
                totals.hash_elapsed_ns,
                ClosureReadTotalsV1::HASH_ELAPSED,
            ),
            (
                DiagnosticCounterV1::ClosureDecodeElapsedNs,
                totals.decode_elapsed_ns,
                ClosureReadTotalsV1::DECODE_ELAPSED,
            ),
            (
                DiagnosticCounterV1::SqliteBatches,
                sqlite_batches,
                ClosureReadTotalsV1::SQLITE_BATCHES,
            ),
            (
                DiagnosticCounterV1::SqliteRows,
                sqlite_rows,
                ClosureReadTotalsV1::SQLITE_ROWS,
            ),
            (
                DiagnosticCounterV1::SqliteElapsedNs,
                totals.sqlite_elapsed_ns,
                ClosureReadTotalsV1::SQLITE_ELAPSED,
            ),
        ] {
            totals.publish_counter(context, &span, key, value, field);
        }
        let outcome = DiagnosticTerminalV1::from_result(&result);
        let _finished = span.finish(outcome);
    }
    result
}

/// Check whether a sealed root graph reaches the exact corrupt reference.
///
/// The bad leaf itself is never opened. Every ancestor is authenticated;
/// another unreadable ancestor aborts rather than yielding a false negative.
pub(crate) fn generation_references_object(
    store: &ObjectStore,
    roots: &[ObjectRef],
    fault: &ObjectRef,
    budget: Option<&RequestBudgetV1>,
) -> Result<bool, CoreError> {
    let mut pending = Vec::new();
    pending
        .try_reserve_exact(roots.len())
        .map_err(|error| db_error("reserve reachability roots", error))?;
    pending.extend(roots.iter().cloned().map(|root| (root, 0_u16)));
    while let Some((reference, depth)) = pending.pop() {
        if let Some(budget) = budget {
            budget.checkpoint("lexical:shared-fault:reachability")?;
        }
        if reference == *fault {
            return Ok(true);
        }
        if depth >= MAX_REACH_DEPTH {
            return Err(CoreError::InvalidContract(
                "lexical shared-fault graph exceeds reachability depth".into(),
            ));
        }
        let children = store.object_dependencies(&reference, budget)?;
        if pending
            .len()
            .checked_add(children.len())
            .is_none_or(|length| length > MAX_REACH_FRONTIER)
        {
            return Err(CoreError::InvalidContract(
                "lexical shared-fault graph exceeds bounded frontier".into(),
            ));
        }
        pending
            .try_reserve(children.len())
            .map_err(|error| db_error("reserve reachability frontier", error))?;
        let next_depth = depth.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract("lexical reachability depth overflow".into())
        })?;
        pending.extend(children.into_iter().map(|child| (child, next_depth)));
    }
    Ok(false)
}

impl SharedGcProgress {
    /// The caller captures this epoch and complete retained directory set
    /// under one lifecycle fence. No object is unlinked during start.
    pub(crate) fn start(
        state_root: &Path,
        retained_generation_dirs: Vec<PathBuf>,
        snapshot_epoch: u64,
    ) -> Result<Self, CoreError> {
        let store = ObjectStore::open_existing(state_root)?;
        let scratch =
            tempfile::tempdir().map_err(|error| db_error("create temp directory", error))?;
        let db = Connection::open(scratch.path().join("lexical-gc.sqlite"))
            .map_err(|error| db_error("open temp database", error))?;
        db.execute_batch(
            "PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=FILE;
             PRAGMA cache_size=-2048;
             CREATE TABLE refs(sha BLOB PRIMARY KEY, kind INTEGER NOT NULL,
                 bytes INTEGER NOT NULL, expanded INTEGER NOT NULL) WITHOUT ROWID;
             CREATE INDEX refs_pending ON refs(expanded,sha);
             CREATE TABLE candidates(sha BLOB PRIMARY KEY) WITHOUT ROWID;",
        )
        .map_err(|error| db_error("initialize mark tables", error))?;
        Ok(Self {
            store,
            _scratch: scratch,
            db,
            retained: retained_generation_dirs,
            snapshot_epoch,
            phase: GcPhase::Adopted,
            adopted: None,
            protected_index: 0,
            inventory: None,
            sweep_after: Vec::new(),
            marked: 0,
            examined: 0,
        })
    }

    /// A unit is one root, directory entry, authenticated object, or sweep
    /// candidate. One admitted attempt header can be 8 MiB; initial retired
    /// control recovery is separately capped at 4096 attempt records.
    pub(crate) fn step(
        &mut self,
        max_units: usize,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<SharedGcStep, CoreError> {
        if max_units == 0 || max_units > MAX_STEP_UNITS {
            return Err(CoreError::InvalidContract(
                "lexical GC step unit bound must be in 1..=256".into(),
            ));
        }
        if self.store.gc_epoch()? != self.snapshot_epoch {
            return Ok(SharedGcStep::Stale);
        }
        let mut references = ReferenceStatements::new(&self.db)?;
        let mut candidate_insert = self
            .db
            .prepare("INSERT OR IGNORE INTO candidates(sha) VALUES (?1)")
            .map_err(|error| db_error("prepare candidate insert", error))?;
        let mut candidate_liveness = self
            .db
            .prepare("SELECT 1 FROM refs WHERE sha=?1")
            .map_err(|error| db_error("prepare candidate liveness", error))?;
        let mut remaining = max_units;
        while remaining > 0 {
            if let Some(budget) = budget {
                budget.checkpoint("lexical:gc:step")?;
            }
            match self.phase {
                GcPhase::Adopted => {
                    if self.adopted.is_none() {
                        self.adopted = Some(self.store.adopted_root_cursor()?);
                        remaining = remaining.saturating_sub(1);
                        continue;
                    }
                    let cursor = self.adopted.as_mut().ok_or_else(|| {
                        CoreError::InvalidContract("lexical GC attempt cursor absent".into())
                    })?;
                    match cursor.next_step(&self.store)? {
                        AdoptedRootAdvance::Root(reference) => {
                            references.insert(&reference)?;
                        }
                        AdoptedRootAdvance::Advanced => {}
                        AdoptedRootAdvance::Done => {
                            self.adopted = None;
                            self.phase = GcPhase::Protected;
                        }
                    }
                    remaining = remaining.saturating_sub(1);
                }
                GcPhase::Protected => {
                    let Some(directory) = self.retained.get(self.protected_index) else {
                        self.retained.clear();
                        self.phase = GcPhase::Mark;
                        remaining = remaining.saturating_sub(1);
                        continue;
                    };
                    for reference in protected_roots(directory)? {
                        references.insert(&reference)?;
                    }
                    self.protected_index =
                        self.protected_index.checked_add(1).ok_or_else(|| {
                            CoreError::InvalidContract("lexical GC root cursor overflow".into())
                        })?;
                    remaining = remaining.saturating_sub(1);
                }
                GcPhase::Mark => {
                    let batch = read_mark_batch(&self.db, remaining)?;
                    if batch.is_empty() {
                        self.phase = GcPhase::Inventory;
                        remaining = remaining.saturating_sub(1);
                        continue;
                    }
                    self.db
                        .execute_batch("BEGIN IMMEDIATE")
                        .map_err(|error| db_error("begin mark batch", error))?;
                    let count = batch.len();
                    for reference in batch {
                        if let Some(budget) = budget {
                            budget.checkpoint("lexical:gc:object")?;
                        }
                        for child in self.store.object_dependencies(&reference, budget)? {
                            references.insert(&child)?;
                        }
                        let _expanded_rows = references.expand(&reference)?;
                    }
                    self.db
                        .execute_batch("COMMIT")
                        .map_err(|error| db_error("commit mark batch", error))?;
                    remaining = remaining.saturating_sub(count);
                }
                GcPhase::Inventory => {
                    if self.inventory.is_none() {
                        self.inventory = Some(self.store.object_inventory_cursor()?);
                        remaining = remaining.saturating_sub(1);
                        continue;
                    }
                    let cursor = self.inventory.as_mut().ok_or_else(|| {
                        CoreError::InvalidContract("lexical GC inventory cursor absent".into())
                    })?;
                    match cursor.next_step(&self.store)? {
                        ObjectInventoryAdvance::Digest(digest) => {
                            let _inserted = candidate_insert
                                .execute(params![digest.as_slice()])
                                .map_err(|error| db_error("insert candidate", error))?;
                        }
                        ObjectInventoryAdvance::Advanced => {}
                        ObjectInventoryAdvance::Done => {
                            self.inventory = None;
                            self.phase = GcPhase::Sweep;
                            let marked: i64 = self
                                .db
                                .query_row("SELECT COUNT(*) FROM refs", [], |row| row.get(0))
                                .map_err(|error| db_error("count marked objects", error))?;
                            self.marked = u64::try_from(marked).map_err(|_conversion_error| {
                                CoreError::InvalidContract("lexical GC mark count width".into())
                            })?;
                        }
                    }
                    remaining = remaining.saturating_sub(1);
                }
                GcPhase::Sweep => {
                    let batch = read_candidate_batch(&self.db, &self.sweep_after, remaining)?;
                    let Some(last) = batch.last() else {
                        if self.store.gc_epoch()? != self.snapshot_epoch {
                            return Ok(SharedGcStep::Stale);
                        }
                        return Ok(SharedGcStep::Completed {
                            marked: self.marked,
                            examined: self.examined,
                        });
                    };
                    if let Some(budget) = budget {
                        budget.checkpoint("lexical:gc:sweep")?;
                    }
                    if !self.store.gc_sweep_checked_batch(
                        self.snapshot_epoch,
                        &batch,
                        &mut |digest| {
                            let exists = candidate_liveness
                                .query_row(params![digest.as_slice()], |row| row.get::<_, i64>(0))
                                .optional()
                                .map_err(|error| db_error("check candidate liveness", error))?;
                            Ok(exists.is_some())
                        },
                    )? {
                        return Ok(SharedGcStep::Stale);
                    }
                    self.sweep_after = last.to_vec();
                    self.examined = self
                        .examined
                        .checked_add(
                            u64::try_from(batch.len())
                                .map_err(|error| db_error("sweep batch width", error))?,
                        )
                        .ok_or_else(|| {
                            CoreError::InvalidContract("lexical GC examined overflow".into())
                        })?;
                    remaining = remaining.saturating_sub(batch.len());
                }
            }
        }
        Ok(SharedGcStep::Paused)
    }
}

#[cfg(test)]
pub(crate) fn collect_shared_pool(
    state_root: &Path,
    retained_generation_dirs: &[PathBuf],
    snapshot_epoch: u64,
    budget: Option<&RequestBudgetV1>,
) -> Result<SharedPoolGcOutcome, CoreError> {
    let mut progress = SharedGcProgress::start(
        state_root,
        retained_generation_dirs.to_vec(),
        snapshot_epoch,
    )?;
    loop {
        match progress.step(MAX_STEP_UNITS, budget)? {
            SharedGcStep::Paused => {}
            SharedGcStep::Completed { marked, examined } => {
                return Ok(SharedPoolGcOutcome::Completed { marked, examined });
            }
            SharedGcStep::Stale => return Ok(SharedPoolGcOutcome::Stale),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::lexical_store::{AttemptBinding, ObjectKind};
    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticEmitV1, DiagnosticEventKindV1,
        DiagnosticEventSinkV1, DiagnosticRootKindV1,
    };

    #[derive(Debug)]
    struct FixedClock;

    impl DiagnosticClockV1 for FixedClock {
        fn now_ns(&self) -> u64 {
            10
        }
    }

    #[derive(Debug, Default)]
    struct RecordingSink(Mutex<Vec<DiagnosticEmitV1>>);

    impl DiagnosticEventSinkV1 for RecordingSink {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            self.0.lock().expect("trace sink lock").push(event);
            true
        }
    }

    fn binding() -> AttemptBinding {
        AttemptBinding {
            root_incarnation: [1; 16],
            repository_sha256: [2; 32],
            revision_sha256: [3; 32],
            source_event_sha256: [4; 32],
            target_generation: 1,
            base_root_sha256: None,
            operations_sha256: [5; 32],
            format: 17,
        }
    }

    #[test]
    fn closure_trace_overflow_stops_each_aggregate_and_emits_one_health_event() {
        let mut totals = ClosureReadTotalsV1 {
            sqlite_elapsed_ns: u64::MAX,
            ..ClosureReadTotalsV1::default()
        };
        observed_sql(true, &mut totals, || {
            std::thread::sleep(std::time::Duration::from_micros(1));
            Ok(())
        })
        .expect("SQL action result is preserved");
        assert_eq!(totals.sqlite_elapsed_ns, u64::MAX);
        assert_ne!(totals.overflowed & ClosureReadTotalsV1::SQLITE_ELAPSED, 0);

        let mut attempted_batches = u64::MAX;
        ClosureReadTotalsV1::accumulate(
            &mut attempted_batches,
            Some(1),
            &mut totals.overflowed,
            ClosureReadTotalsV1::SQLITE_BATCHES,
        );
        let mut expanded_rows = u64::MAX - 1;
        ClosureReadTotalsV1::accumulate(
            &mut expanded_rows,
            Some(2),
            &mut totals.overflowed,
            ClosureReadTotalsV1::SQLITE_ROWS,
        );
        assert_eq!(attempted_batches, u64::MAX);
        assert_eq!(expanded_rows, u64::MAX - 1);
        ClosureReadTotalsV1::accumulate(
            &mut expanded_rows,
            Some(1),
            &mut totals.overflowed,
            ClosureReadTotalsV1::SQLITE_ROWS,
        );
        assert_eq!(expanded_rows, u64::MAX - 1, "overflowed field is frozen");
        let mut dependency_edges = 4;
        ClosureReadTotalsV1::accumulate(
            &mut dependency_edges,
            None,
            &mut totals.overflowed,
            ClosureReadTotalsV1::DEPENDENCY_EDGES,
        );
        assert_eq!(dependency_edges, 4, "lossy width is not emitted");
        assert_ne!(totals.overflowed & ClosureReadTotalsV1::DEPENDENCY_EDGES, 0);

        let sink = Arc::new(RecordingSink::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([7; 16]),
            Arc::new(FixedClock),
            sink.clone(),
        );
        let span = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("trace root");
        totals.publish_overflow_health(&context);
        totals.publish_counter(
            &context,
            &span,
            DiagnosticCounterV1::SqliteElapsedNs,
            totals.sqlite_elapsed_ns,
            ClosureReadTotalsV1::SQLITE_ELAPSED,
        );
        totals.publish_counter(
            &context,
            &span,
            DiagnosticCounterV1::FilesRead,
            totals.objects_read,
            ClosureReadTotalsV1::OBJECTS_READ,
        );
        let events = sink.0.lock().expect("trace sink lock");
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    &event.kind,
                    DiagnosticEventKindV1::Health {
                        code: DiagnosticHealthCodeV1::CounterOverflow
                    }
                ))
                .count(),
            1
        );
        assert!(!events.iter().any(|event| matches!(
            &event.kind,
            DiagnosticEventKindV1::Counter {
                key: DiagnosticCounterV1::SqliteElapsedNs,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            DiagnosticEventKindV1::Counter {
                key: DiagnosticCounterV1::FilesRead,
                value: 0,
                ..
            }
        )));
        drop(events);
    }

    #[test]
    fn external_mark_preserves_adopted_object_and_sweeps_after_retirement() {
        let dir = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(dir.path()).expect("pool");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let body = store
            .put_blob(&lease, ObjectKind::FileBody, &[], b"retained")
            .expect("body");
        let retained = collect_shared_pool(
            dir.path(),
            &[],
            store.gc_epoch().expect("snapshot epoch"),
            None,
        )
        .expect("protected mark");
        assert!(matches!(retained, SharedPoolGcOutcome::Completed { marked, .. } if marked > 0));
        assert_eq!(
            store.read_blob(&body, 8, None).expect("still retained"),
            b"retained"
        );
        lease.discard_unsealed().expect("retire unsealed attempt");
        let swept = collect_shared_pool(
            dir.path(),
            &[],
            store.gc_epoch().expect("snapshot epoch"),
            None,
        )
        .expect("orphan sweep");
        assert!(
            matches!(swept, SharedPoolGcOutcome::Completed { marked: 0, examined } if examined > 0)
        );
        assert!(store.read_blob(&body, 8, None).is_err());
    }

    #[test]
    fn stale_epoch_refuses_collection_before_marking() {
        let dir = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(dir.path()).expect("pool");
        let old_epoch = store.gc_epoch().expect("snapshot epoch");
        let lease = store.begin_attempt(&binding(), &[]).expect("new adoption");
        assert_eq!(
            collect_shared_pool(dir.path(), &[], old_epoch, None).expect("stale collection"),
            SharedPoolGcOutcome::Stale
        );
        lease.discard_unsealed().expect("retire attempt");
    }

    #[test]
    fn one_unit_steps_do_not_unlink_before_complete_mark_and_inventory() {
        let dir = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(dir.path()).expect("pool");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let body = store
            .put_blob(&lease, ObjectKind::FileBody, &[], b"orphan")
            .expect("body");
        lease.discard_unsealed().expect("retire attempt");
        let mut progress = SharedGcProgress::start(
            dir.path(),
            Vec::new(),
            store.gc_epoch().expect("snapshot epoch"),
        )
        .expect("start collection");
        for _ in 0..1024 {
            if progress.phase == GcPhase::Sweep {
                break;
            }
            assert_eq!(
                progress.step(1, None).expect("bounded step"),
                SharedGcStep::Paused
            );
            assert_eq!(
                store.read_blob(&body, 8, None).expect("pre-sweep object"),
                b"orphan"
            );
        }
        assert_eq!(progress.phase, GcPhase::Sweep);
        let mut completed = false;
        for _ in 0..32 {
            match progress.step(1, None).expect("sweep step") {
                SharedGcStep::Paused => {}
                SharedGcStep::Completed { examined, .. } => {
                    assert!(examined > 0);
                    completed = true;
                    break;
                }
                SharedGcStep::Stale => panic!("unchanged epoch became stale"),
            }
        }
        assert!(completed);
        assert!(store.read_blob(&body, 8, None).is_err());
    }

    #[test]
    fn epoch_change_between_bounded_steps_refuses_deletion() {
        let dir = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(dir.path()).expect("pool");
        let mut progress = SharedGcProgress::start(
            dir.path(),
            Vec::new(),
            store.gc_epoch().expect("snapshot epoch"),
        )
        .expect("start collection");
        assert_eq!(
            progress.step(1, None).expect("initial step"),
            SharedGcStep::Paused
        );
        let lease = store.begin_attempt(&binding(), &[]).expect("new attempt");
        let body = store
            .put_blob(&lease, ObjectKind::FileBody, &[], b"new live body")
            .expect("adopted body");
        assert_eq!(
            progress.step(256, None).expect("stale step"),
            SharedGcStep::Stale
        );
        assert_eq!(
            store.read_blob(&body, 16, None).expect("new body remains"),
            b"new live body"
        );
        lease.discard_unsealed().expect("retire attempt");
    }

    #[test]
    fn closure_measure_diamond_has_fixed_bytes_and_refuses_conflicts_and_corruption() {
        use std::fmt::Write as _;

        let dir = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(dir.path()).expect("pool");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let leaf = store
            .put_blob(&lease, ObjectKind::Record, &[], b"leaf")
            .expect("leaf");
        let left = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                std::slice::from_ref(&leaf.root),
                b"a",
            )
            .expect("left");
        let right = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                std::slice::from_ref(&leaf.root),
                b"b",
            )
            .expect("right");
        let root = store
            .put_blob(
                &lease,
                ObjectKind::Record,
                &[left.root, right.root],
                b"root",
            )
            .expect("root");
        // Fixed wire widths: shared leaf 26, each parent 64, root 108.
        // The same leaf and root are charged once across all supplied roots.
        assert_eq!(
            measure_unique_closure_bytes(
                &store,
                [root.root.clone(), leaf.root.clone(), root.root.clone()],
                None,
            )
            .expect("shared diamond"),
            262,
        );
        for forged in [
            ObjectRef {
                kind: ObjectKind::Segment,
                ..root.root
            },
            ObjectRef {
                bytes: root.root.bytes.saturating_add(1),
                ..root.root
            },
        ] {
            assert!(matches!(
                measure_unique_closure_bytes(&store, [root.root.clone(), forged], None),
                Err(CoreError::InvalidContract(message))
                    if message.contains("conflicting kind or size")
            ));
        }
        let mut digest = String::new();
        for byte in leaf.root.sha256 {
            write!(&mut digest, "{byte:02x}").expect("format digest");
        }
        let path = dir
            .path()
            .join("lexical-object-pool")
            .join("objects")
            .join(digest.get(..2).expect("shard"))
            .join(&digest);
        let mut bytes = std::fs::read(&path).expect("leaf object");
        *bytes.last_mut().expect("nonempty leaf") ^= 1;
        std::fs::write(path, bytes).expect("corrupt leaf");
        assert!(matches!(
            measure_unique_closure_bytes(&store, [root.root], None),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ));
    }

    #[test]
    fn closure_measure_authenticates_blocks_and_deduplicates_shared_roots() {
        let dir = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(dir.path()).expect("pool");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let payload = vec![7_u8; 70_usize.saturating_mul(1024)];
        let body = store
            .put_blob(&lease, ObjectKind::FileBody, &[], &payload)
            .expect("multi-block body");
        let once =
            measure_unique_closure_bytes(&store, [body.root.clone()], None).expect("one root");
        let sink = Arc::new(RecordingSink::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([1; 16]),
            Arc::new(FixedClock),
            sink.clone(),
        );
        let root = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("trace root");
        let child = context.child_context(&root);
        let twice = measure_unique_closure_bytes(
            &store,
            [body.root.clone(), body.root.clone()],
            Some(&child),
        )
        .expect("shared root");
        root.finish(DiagnosticTerminalV1::Ok)
            .expect("trace root terminal");
        assert_eq!(once, twice);
        assert!(once > body.root.bytes);
        let events = sink.0.lock().expect("trace sink lock");
        let count = |key| {
            events.iter().find_map(|event| {
                if let DiagnosticEventKindV1::Counter {
                    key: observed,
                    value,
                    ..
                } = &event.kind
                {
                    (*observed == key).then_some(*value)
                } else {
                    None
                }
            })
        };
        assert!(count(DiagnosticCounterV1::UniqueClosureRefs).is_some_and(|refs| refs > 1));
        assert!(count(DiagnosticCounterV1::ClosureDependencyEdges).is_some_and(|edges| edges > 0));
        assert_eq!(
            count(DiagnosticCounterV1::ClosureAuthenticatedBytes),
            Some(twice)
        );
        assert!(
            count(DiagnosticCounterV1::SqliteBatches).is_some_and(|batches| batches >= 2),
            "the terminal empty mark-batch query is an attempted SQLite batch"
        );
        lease.discard_unsealed().expect("retire attempt");
    }

    #[test]
    fn closure_measure_does_not_count_uncommitted_expanded_rows() {
        let dir = tempfile::tempdir().expect("state root");
        let store = ObjectStore::open(dir.path()).expect("pool");
        let lease = store.begin_attempt(&binding(), &[]).expect("attempt");
        let present = store
            .put_blob(&lease, ObjectKind::FileBody, &[], b"retained")
            .expect("present body");
        let sink = Arc::new(RecordingSink::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([2; 16]),
            Arc::new(FixedClock),
            sink.clone(),
        );
        let root = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("trace root");
        let child = context.child_context(&root);
        let missing = ObjectRef {
            kind: ObjectKind::FileBody,
            bytes: 1,
            sha256: [u8::MAX; 32],
        };
        assert!(present.root.sha256 < missing.sha256);
        assert!(
            measure_unique_closure_bytes(&store, [present.root, missing], Some(&child)).is_err()
        );
        root.finish(DiagnosticTerminalV1::Ok)
            .expect("trace root terminal");
        let events = sink.0.lock().expect("trace sink lock");
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            DiagnosticEventKindV1::Counter {
                key: DiagnosticCounterV1::SqliteRows,
                value: 0,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            DiagnosticEventKindV1::Counter {
                key: DiagnosticCounterV1::UniqueClosureRefs,
                value: 1,
                ..
            }
        )));
        drop(events);
        lease.discard_unsealed().expect("retire attempt");
    }
}
