//! Request-local membership for exact lexical history-retention unions.
//!
//! A sealed generation is immutable while its directory-lifecycle read guard
//! is held. The `SQLite` file caches authenticated object edges and per-generation
//! membership; a later subset query updates only changed memberships instead
//! of walking the object pool again. The main-file cap does not reserve disk
//! blocks or bound `SQLite`'s process memory journal. This is disposable state.

use std::collections::BTreeSet;
use std::sync::{MutexGuard, RwLockReadGuard, TryLockError};
use std::time::Instant;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind};
use quanta_index_core::domains::generation::{
    GenerationStorageKeyV1, SealedGenerationByteSession, SealedGenerationBytesV1,
};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticCounterV1, DiagnosticStageV1, DiagnosticTerminalV1,
    RequestBudgetV1,
};
use quanta_index_scratch_sqlite::retention::RetainedInodeMembership;
use rusqlite::{Connection, OptionalExtension as _, params};

use crate::LexicalAdapter;
use crate::causal_profile::ClosureReadTotalsV1;
use crate::generation_dir::is_writer_lock_entry;
use crate::lexical_store::{MAX_CLOSURE_REFS, ObjectKind, ObjectRef, ObjectStore};

fn storage(action: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("lexical retention membership {action}: {error}"))
}

fn refusal(reason: &str) -> CoreError {
    CoreError::InvalidContract(format!("lexical retention membership: {reason}"))
}

fn checkpoint(budget: Option<&RequestBudgetV1>, stage: &'static str) -> Result<(), CoreError> {
    if let Some(budget) = budget {
        budget.checkpoint(stage)?;
    }
    Ok(())
}

fn rollback_savepoint(db: &Connection, name: &str) -> Result<(), CoreError> {
    if db.is_autocommit() {
        // SQLite may have already rolled back the transaction on SQLITE_FULL.
        return Ok(());
    }
    match db.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}")) {
        Ok(()) => Ok(()),
        Err(first) => {
            // There is no other transaction on this private connection. A
            // failed savepoint rollback must still close it before retry.
            db.execute_batch("ROLLBACK").map_err(|second| {
                CoreError::Storage(format!(
                    "lexical retention membership rollback failed ({first}); full rollback failed ({second})"
                ))
            })
        }
    }
}

fn width(value: u64) -> Result<i64, CoreError> {
    i64::try_from(value).map_err(|error| storage("integer width", error))
}

fn generation_id(generation: ManifestGeneration) -> [u8; 8] {
    generation.get().to_be_bytes()
}

fn decode_reference(sha: Vec<u8>, kind: i64, bytes: i64) -> Result<ObjectRef, CoreError> {
    let sha256: [u8; 32] = sha
        .try_into()
        .map_err(|error: Vec<u8>| storage("object digest width", error.len()))?;
    let kind = u8::try_from(kind)
        .map_err(|error| storage("object kind width", error))
        .and_then(|code| {
            ObjectKind::from_code(code).ok_or_else(|| refusal("object kind differs"))
        })?;
    let bytes = u64::try_from(bytes).map_err(|error| storage("object width", error))?;
    Ok(ObjectRef {
        kind,
        bytes,
        sha256,
    })
}

#[derive(Default)]
struct ClosureObservation {
    enabled: bool,
    totals: ClosureReadTotalsV1,
    sqlite_batches: u64,
    sqlite_rows: u64,
    unique_refs: u64,
    dependency_edges: u64,
    authenticated_bytes: u64,
}

impl ClosureObservation {
    fn sql<T>(&mut self, operation: impl FnOnce() -> Result<T, CoreError>) -> Result<T, CoreError> {
        let started = self.enabled.then(Instant::now);
        let result = operation();
        if let Some(started) = started {
            ClosureReadTotalsV1::accumulate_elapsed(
                &mut self.totals.sqlite_elapsed_ns,
                Some(started),
                &mut self.totals.overflowed,
                ClosureReadTotalsV1::SQLITE_ELAPSED,
            );
        }
        result
    }

    fn batch(&mut self) {
        if self.enabled {
            ClosureReadTotalsV1::accumulate(
                &mut self.sqlite_batches,
                Some(1),
                &mut self.totals.overflowed,
                ClosureReadTotalsV1::SQLITE_BATCHES,
            );
        }
    }

    fn rows(&mut self, count: usize) {
        if self.enabled {
            ClosureReadTotalsV1::accumulate_width(
                &mut self.sqlite_rows,
                count,
                &mut self.totals.overflowed,
                ClosureReadTotalsV1::SQLITE_ROWS,
            );
        }
    }

    fn authenticated(&mut self, reference: &ObjectRef, children: usize) {
        if self.enabled {
            ClosureReadTotalsV1::accumulate(
                &mut self.unique_refs,
                Some(1),
                &mut self.totals.overflowed,
                ClosureReadTotalsV1::UNIQUE_REFS,
            );
            ClosureReadTotalsV1::accumulate_width(
                &mut self.dependency_edges,
                children,
                &mut self.totals.overflowed,
                ClosureReadTotalsV1::DEPENDENCY_EDGES,
            );
            ClosureReadTotalsV1::accumulate(
                &mut self.authenticated_bytes,
                Some(reference.bytes),
                &mut self.totals.overflowed,
                ClosureReadTotalsV1::AUTHENTICATED_BYTES,
            );
        }
    }

    fn publish(
        &self,
        diagnostic: &DiagnosticContextV1,
        span: &quanta_index_core::DiagnosticSpanGuardV1,
    ) {
        self.totals.publish_overflow_health(diagnostic);
        for (key, value, field) in [
            (
                DiagnosticCounterV1::UniqueClosureRefs,
                self.unique_refs,
                ClosureReadTotalsV1::UNIQUE_REFS,
            ),
            (
                DiagnosticCounterV1::ClosureDependencyEdges,
                self.dependency_edges,
                ClosureReadTotalsV1::DEPENDENCY_EDGES,
            ),
            (
                DiagnosticCounterV1::ClosureAuthenticatedBytes,
                self.authenticated_bytes,
                ClosureReadTotalsV1::AUTHENTICATED_BYTES,
            ),
            (
                DiagnosticCounterV1::FilesRead,
                self.totals.objects_read,
                ClosureReadTotalsV1::OBJECTS_READ,
            ),
            (
                DiagnosticCounterV1::BytesRead,
                self.totals.bytes_read,
                ClosureReadTotalsV1::BYTES_READ,
            ),
            (
                DiagnosticCounterV1::FilesHashed,
                self.totals.objects_hashed,
                ClosureReadTotalsV1::OBJECTS_HASHED,
            ),
            (
                DiagnosticCounterV1::BytesHashed,
                self.totals.bytes_hashed,
                ClosureReadTotalsV1::BYTES_HASHED,
            ),
            (
                DiagnosticCounterV1::FilesDecoded,
                self.totals.objects_decoded,
                ClosureReadTotalsV1::OBJECTS_DECODED,
            ),
            (
                DiagnosticCounterV1::BytesDecoded,
                self.totals.bytes_decoded,
                ClosureReadTotalsV1::BYTES_DECODED,
            ),
            (
                DiagnosticCounterV1::ClosureReadElapsedNs,
                self.totals.read_elapsed_ns,
                ClosureReadTotalsV1::READ_ELAPSED,
            ),
            (
                DiagnosticCounterV1::ClosureHashElapsedNs,
                self.totals.hash_elapsed_ns,
                ClosureReadTotalsV1::HASH_ELAPSED,
            ),
            (
                DiagnosticCounterV1::ClosureDecodeElapsedNs,
                self.totals.decode_elapsed_ns,
                ClosureReadTotalsV1::DECODE_ELAPSED,
            ),
            (
                DiagnosticCounterV1::SqliteBatches,
                self.sqlite_batches,
                ClosureReadTotalsV1::SQLITE_BATCHES,
            ),
            (
                DiagnosticCounterV1::SqliteRows,
                self.sqlite_rows,
                ClosureReadTotalsV1::SQLITE_ROWS,
            ),
            (
                DiagnosticCounterV1::SqliteElapsedNs,
                self.totals.sqlite_elapsed_ns,
                ClosureReadTotalsV1::SQLITE_ELAPSED,
            ),
        ] {
            self.totals
                .publish_counter(diagnostic, span, key, value, field);
        }
    }
}

/// One disposable, bounded main database. Its tables are scoped to one pair.
pub(crate) struct ClosureMembership {
    db: Connection,
    _directory: tempfile::TempDir,
    selected: BTreeSet<ManifestGeneration>,
    selected_bytes: u64,
}

impl ClosureMembership {
    pub(crate) fn open(max_main_file_bytes: u64) -> Result<Self, CoreError> {
        const PAGE_BYTES: u64 = 4096;
        if max_main_file_bytes < 64 * 1024 || !max_main_file_bytes.is_multiple_of(PAGE_BYTES) {
            return Err(refusal(
                "scratch main-file cap must be page aligned and at least 64 KiB",
            ));
        }
        let directory = tempfile::tempdir().map_err(|error| storage("create scratch", error))?;
        let db = Connection::open(directory.path().join("closure.sqlite"))
            .map_err(|error| storage("open scratch", error))?;
        db.execute_batch(
            "PRAGMA page_size=4096;
             PRAGMA journal_mode=MEMORY;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=MEMORY;
             PRAGMA cache_size=-2048;
             PRAGMA mmap_size=0;",
        )
        .map_err(|error| storage("configure scratch", error))?;
        let page_size: i64 = db
            .query_row("PRAGMA main.page_size", [], |row| row.get(0))
            .map_err(|error| storage("read scratch page size", error))?;
        let journal_mode: String = db
            .query_row("PRAGMA main.journal_mode", [], |row| row.get(0))
            .map_err(|error| storage("read scratch journal mode", error))?;
        let temp_store: i64 = db
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .map_err(|error| storage("read scratch temp store", error))?;
        let installed_page_size =
            u64::try_from(page_size).map_err(|error| storage("scratch page size width", error))?;
        if installed_page_size != PAGE_BYTES || journal_mode != "memory" || temp_store != 2 {
            return Err(refusal("SQLite scratch configuration was not installed"));
        }
        let pages = max_main_file_bytes
            .checked_div(PAGE_BYTES)
            .ok_or_else(|| refusal("scratch page size divisor is zero"))?;
        let installed: i64 = db
            .query_row(&format!("PRAGMA main.max_page_count={pages}"), [], |row| {
                row.get(0)
            })
            .map_err(|error| storage("install scratch cap", error))?;
        let installed = u64::try_from(installed)
            .map_err(|error| storage("installed scratch cap width", error))?;
        if installed != pages {
            return Err(refusal("scratch main-file cap was not installed"));
        }
        db.execute_batch(
            "CREATE TABLE objects(
                 sha BLOB PRIMARY KEY, kind INTEGER NOT NULL,
                 bytes INTEGER NOT NULL, authenticated INTEGER NOT NULL DEFAULT 0
             ) WITHOUT ROWID;
             CREATE TABLE edges(
                 parent BLOB NOT NULL, child BLOB NOT NULL,
                 PRIMARY KEY(parent,child)
             ) WITHOUT ROWID;
             CREATE TABLE members(
                 generation BLOB NOT NULL CHECK(length(generation)=8), sha BLOB NOT NULL,
                 expanded INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY(generation,sha)
             ) WITHOUT ROWID;
             CREATE INDEX pending_members ON members(generation,expanded,sha);
             CREATE INDEX member_object ON members(sha,generation);
             CREATE TABLE completed(generation BLOB PRIMARY KEY CHECK(length(generation)=8), manifest_digest TEXT NOT NULL) WITHOUT ROWID;
             CREATE TABLE selected_refs(sha BLOB PRIMARY KEY, refcount INTEGER NOT NULL) WITHOUT ROWID;",
        )
        .map_err(|error| storage("create scratch schema", error))?;
        Ok(Self {
            db,
            _directory: directory,
            selected: BTreeSet::new(),
            selected_bytes: 0,
        })
    }

    pub(crate) fn cached(
        &self,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<bool, CoreError> {
        let known: Option<String> = self
            .db
            .query_row(
                "SELECT manifest_digest FROM completed WHERE generation=?1",
                params![&generation_id(generation)[..]],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| storage("read completed generation", error))?;
        match known {
            None => Ok(false),
            Some(known) if known == manifest_digest => Ok(true),
            Some(_) => Err(refusal("cached generation identity changed")),
        }
    }

    fn insert_reference(&self, reference: &ObjectRef) -> Result<(), CoreError> {
        let encoded_bytes = width(reference.bytes)?;
        let _inserted = self
            .db
            .execute(
                "INSERT OR IGNORE INTO objects(sha,kind,bytes) VALUES (?1,?2,?3)",
                params![
                    reference.sha256.as_slice(),
                    i64::from(reference.kind.code()),
                    encoded_bytes
                ],
            )
            .map_err(|error| storage("insert object", error))?;
        let observed: (i64, i64) = self
            .db
            .query_row(
                "SELECT kind,bytes FROM objects WHERE sha=?1",
                params![reference.sha256.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| storage("read object descriptor", error))?;
        if observed != (i64::from(reference.kind.code()), encoded_bytes) {
            return Err(refusal("one digest has conflicting kind or encoded width"));
        }
        Ok(())
    }

    fn insert_member(
        &self,
        generation: &[u8; 8],
        reference: &ObjectRef,
    ) -> Result<usize, CoreError> {
        self.insert_reference(reference)?;
        self.db
            .execute(
                "INSERT OR IGNORE INTO members(generation,sha) VALUES (?1,?2)",
                params![&generation[..], reference.sha256.as_slice()],
            )
            .map_err(|error| storage("insert generation member", error))
    }

    fn next_pending(&self, generation: &[u8; 8]) -> Result<Option<ObjectRef>, CoreError> {
        let row: Option<(Vec<u8>, i64, i64)> = self
            .db
            .query_row(
                "SELECT o.sha,o.kind,o.bytes FROM members m
                 JOIN objects o ON o.sha=m.sha
                 WHERE m.generation=?1 AND m.expanded=0 ORDER BY m.sha LIMIT 1",
                params![&generation[..]],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| storage("read pending member", error))?;
        row.map(|(sha, kind, bytes)| decode_reference(sha, kind, bytes))
            .transpose()
    }

    fn dependencies(
        &self,
        store: &ObjectStore,
        reference: &ObjectRef,
        observation: &mut ClosureObservation,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<Vec<ObjectRef>, CoreError> {
        checkpoint(budget, "lexical-retention:dependencies")?;
        let authenticated: i64 = observation.sql(|| {
            self.db
                .query_row(
                    "SELECT authenticated FROM objects WHERE sha=?1",
                    params![reference.sha256.as_slice()],
                    |row| row.get(0),
                )
                .map_err(|error| storage("read object authentication", error))
        })?;
        if authenticated == 0 {
            let children = if observation.enabled {
                store.object_dependencies_for_closure(reference, &mut observation.totals)?
            } else {
                store.object_dependencies(reference, budget)?
            };
            observation.authenticated(reference, children.len());
            for child in &children {
                checkpoint(budget, "lexical-retention:dependency")?;
                observation.sql(|| self.insert_reference(child))?;
                let _inserted = observation.sql(|| {
                    self.db
                        .execute(
                            "INSERT OR IGNORE INTO edges(parent,child) VALUES (?1,?2)",
                            params![reference.sha256.as_slice(), child.sha256.as_slice()],
                        )
                        .map_err(|error| storage("insert authenticated dependency", error))
                })?;
            }
            let _updated = observation.sql(|| {
                self.db
                    .execute(
                        "UPDATE objects SET authenticated=1 WHERE sha=?1 AND authenticated=0",
                        params![reference.sha256.as_slice()],
                    )
                    .map_err(|error| storage("record object authentication", error))
            })?;
            return Ok(children);
        }
        if authenticated != 1 {
            return Err(refusal("invalid authentication state"));
        }
        let children = observation.sql(|| {
            let mut statement = self
                .db
                .prepare(
                    "SELECT o.sha,o.kind,o.bytes FROM edges e
                 JOIN objects o ON o.sha=e.child WHERE e.parent=?1 ORDER BY e.child",
                )
                .map_err(|error| storage("prepare cached dependencies", error))?;
            let mut rows = statement
                .query(params![reference.sha256.as_slice()])
                .map_err(|error| storage("query cached dependencies", error))?;
            let mut children = Vec::new();
            while let Some(row) = rows
                .next()
                .map_err(|error| storage("next cached dependency", error))?
            {
                checkpoint(budget, "lexical-retention:cached-dependency")?;
                children.push(decode_reference(
                    row.get(0)
                        .map_err(|error| storage("cached dependency sha", error))?,
                    row.get(1)
                        .map_err(|error| storage("cached dependency kind", error))?,
                    row.get(2)
                        .map_err(|error| storage("cached dependency width", error))?,
                )?);
            }
            Ok(children)
        })?;
        Ok(children)
    }

    /// Stage one complete, independently authenticated sealed closure.
    /// Failure rolls back every object, edge and member inserted by this probe.
    #[cfg(test)]
    pub(crate) fn record_generation(
        &self,
        store: Option<&ObjectStore>,
        generation: ManifestGeneration,
        manifest_digest: &str,
        roots: &[ObjectRef],
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        self.record_generation_observed(store, generation, manifest_digest, roots, None, diagnostic)
    }

    pub(crate) fn record_generation_with_budget(
        &self,
        store: Option<&ObjectStore>,
        generation: ManifestGeneration,
        manifest_digest: &str,
        roots: &[ObjectRef],
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        self.record_generation_observed(
            store,
            generation,
            manifest_digest,
            roots,
            Some(budget),
            diagnostic,
        )
    }

    fn record_generation_observed(
        &self,
        store: Option<&ObjectStore>,
        generation: ManifestGeneration,
        manifest_digest: &str,
        roots: &[ObjectRef],
        budget: Option<&RequestBudgetV1>,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        let span = diagnostic.and_then(|context| {
            crate::causal_profile::enter_span(context, DiagnosticStageV1::ClosureRead)
        });
        let mut observation = ClosureObservation {
            enabled: span.is_some(),
            ..ClosureObservation::default()
        };
        let result = self.record_generation_inner(
            store,
            generation,
            manifest_digest,
            roots,
            &mut observation,
            budget,
        );
        if let (Some(context), Some(span)) = (diagnostic, span) {
            observation.publish(context, &span);
            let outcome = DiagnosticTerminalV1::from_result(&result);
            let _finished = span.finish(outcome);
        }
        result
    }

    fn record_generation_inner(
        &self,
        store: Option<&ObjectStore>,
        generation: ManifestGeneration,
        manifest_digest: &str,
        roots: &[ObjectRef],
        observation: &mut ClosureObservation,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        checkpoint(budget, "lexical-retention:record-generation")?;
        if observation.sql(|| self.cached(generation, manifest_digest))? {
            return Ok(());
        }
        observation.sql(|| {
            self.db
                .execute_batch("SAVEPOINT lexical_retention_probe")
                .map_err(|error| storage("begin generation probe", error))
        })?;
        let result = (|| {
            let generation = generation_id(generation);
            let mut members = 0_usize;
            let mut expanded_rows = 0_usize;
            for root in roots {
                checkpoint(budget, "lexical-retention:root")?;
                let inserted = observation.sql(|| self.insert_member(&generation, root))?;
                members = members
                    .checked_add(inserted)
                    .ok_or_else(|| refusal("member count overflow"))?;
            }
            if members > MAX_CLOSURE_REFS {
                return Err(refusal("object closure exceeds traversal bound"));
            }
            loop {
                checkpoint(budget, "lexical-retention:pending-object")?;
                observation.batch();
                let Some(reference) = observation.sql(|| self.next_pending(&generation))? else {
                    break;
                };
                let store =
                    store.ok_or_else(|| refusal("object store missing for nonempty closure"))?;
                let children = self.dependencies(store, &reference, observation, budget)?;
                for child in children {
                    checkpoint(budget, "lexical-retention:child-object")?;
                    let inserted = observation.sql(|| self.insert_member(&generation, &child))?;
                    members = members
                        .checked_add(inserted)
                        .ok_or_else(|| refusal("member count overflow"))?;
                    if members > MAX_CLOSURE_REFS {
                        return Err(refusal("object closure exceeds traversal bound"));
                    }
                }
                let updated = observation.sql(|| self.db
                    .execute(
                        "UPDATE members SET expanded=1 WHERE generation=?1 AND sha=?2 AND expanded=0",
                        params![&generation[..], reference.sha256.as_slice()],
                    )
                    .map_err(|error| storage("complete member expansion", error)))?;
                if updated != 1 {
                    return Err(refusal("member expansion changed unexpectedly"));
                }
                expanded_rows = expanded_rows
                    .checked_add(updated)
                    .ok_or_else(|| refusal("expanded member count overflow"))?;
            }
            let inserted = observation.sql(|| {
                self.db
                    .execute(
                        "INSERT INTO completed(generation,manifest_digest) VALUES (?1,?2)",
                        params![&generation[..], manifest_digest],
                    )
                    .map_err(|error| storage("complete generation", error))
            })?;
            if inserted != 1 {
                return Err(refusal("generation completion changed unexpectedly"));
            }
            checkpoint(budget, "lexical-retention:record-commit")?;
            Ok(expanded_rows)
        })();
        match result {
            Ok(expanded_rows) => {
                if let Err(error) = observation.sql(|| {
                    self.db
                        .execute_batch("RELEASE lexical_retention_probe")
                        .map_err(|error| storage("commit generation probe", error))
                }) {
                    observation.sql(|| rollback_savepoint(&self.db, "lexical_retention_probe"))?;
                    return Err(error);
                }
                // Count only expansion rows whose generation probe committed.
                observation.rows(expanded_rows);
                Ok(())
            }
            Err(error) => {
                observation.sql(|| rollback_savepoint(&self.db, "lexical_retention_probe"))?;
                Err(error)
            }
        }
    }

    fn change_selected_generation(
        &self,
        generation: ManifestGeneration,
        add: bool,
        total: &mut u64,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<(), CoreError> {
        const BATCH: i64 = 256;
        let id = generation_id(generation);
        let mut after = Vec::<u8>::new();
        loop {
            checkpoint(budget, "lexical-retention:selection-batch")?;
            let batch: Vec<(Vec<u8>, u64)> = {
                let mut statement = self
                    .db
                    .prepare(
                        "SELECT m.sha,o.bytes FROM members m JOIN objects o ON o.sha=m.sha
                         WHERE m.generation=?1 AND m.sha>?2 ORDER BY m.sha LIMIT ?3",
                    )
                    .map_err(|error| storage("prepare changed-generation members", error))?;
                let mut rows = statement
                    .query(params![&id[..], &after, BATCH])
                    .map_err(|error| storage("query changed-generation members", error))?;
                let mut batch = Vec::new();
                while let Some(row) = rows
                    .next()
                    .map_err(|error| storage("next changed-generation member", error))?
                {
                    checkpoint(budget, "lexical-retention:selection-row")?;
                    let sha: Vec<u8> = row
                        .get(0)
                        .map_err(|error| storage("changed member digest", error))?;
                    if sha.len() != 32 {
                        return Err(refusal("changed member digest width"));
                    }
                    let width: i64 = row
                        .get(1)
                        .map_err(|error| storage("changed member width", error))?;
                    let width = u64::try_from(width)
                        .map_err(|error| storage("changed member width", error))?;
                    batch.push((sha, width));
                }
                batch
            };
            if batch.is_empty() {
                break;
            }
            for (sha, bytes) in &batch {
                checkpoint(budget, "lexical-retention:selection-change")?;
                let prior: Option<i64> = self
                    .db
                    .query_row(
                        "SELECT refcount FROM selected_refs WHERE sha=?1",
                        params![sha],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| storage("read selected reference count", error))?;
                match (add, prior) {
                    (true, None) => {
                        let _inserted = self
                            .db
                            .execute(
                                "INSERT INTO selected_refs(sha,refcount) VALUES (?1,1)",
                                params![sha],
                            )
                            .map_err(|error| storage("select object", error))?;
                        *total = total
                            .checked_add(*bytes)
                            .ok_or_else(|| refusal("selected object byte total overflow"))?;
                    }
                    (true, Some(count)) if count > 0 && count < i64::MAX => {
                        let next_count = count
                            .checked_add(1)
                            .ok_or_else(|| refusal("selected reference count overflow"))?;
                        let _updated = self
                            .db
                            .execute(
                                "UPDATE selected_refs SET refcount=?2 WHERE sha=?1",
                                params![sha, next_count],
                            )
                            .map_err(|error| storage("increment selected reference", error))?;
                    }
                    (false, Some(1)) => {
                        let _deleted = self
                            .db
                            .execute("DELETE FROM selected_refs WHERE sha=?1", params![sha])
                            .map_err(|error| storage("deselect object", error))?;
                        *total = total
                            .checked_sub(*bytes)
                            .ok_or_else(|| refusal("selected object byte total underflow"))?;
                    }
                    (false, Some(count)) if count > 1 => {
                        let next_count = count
                            .checked_sub(1)
                            .ok_or_else(|| refusal("selected reference count underflow"))?;
                        let _updated = self
                            .db
                            .execute(
                                "UPDATE selected_refs SET refcount=?2 WHERE sha=?1",
                                params![sha, next_count],
                            )
                            .map_err(|error| storage("decrement selected reference", error))?;
                    }
                    _ => return Err(refusal("selected reference count differs")),
                }
            }
            let Some((sha, _bytes)) = batch.last() else {
                return Err(refusal("nonempty changed-member batch became empty"));
            };
            after.clone_from(sha);
        }
        Ok(())
    }

    /// Maintain the exact digest union by changing only entered or retired
    /// generation memberships. A failed delta rolls back SQL and Rust totals.
    #[cfg(test)]
    pub(crate) fn union_bytes(
        &mut self,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<u64, CoreError> {
        self.union_bytes_inner(generations, None)
    }

    pub(crate) fn union_bytes_with_budget(
        &mut self,
        generations: &BTreeSet<ManifestGeneration>,
        budget: &RequestBudgetV1,
    ) -> Result<u64, CoreError> {
        self.union_bytes_inner(generations, Some(budget))
    }

    fn union_bytes_inner(
        &mut self,
        generations: &BTreeSet<ManifestGeneration>,
        budget: Option<&RequestBudgetV1>,
    ) -> Result<u64, CoreError> {
        checkpoint(budget, "lexical-retention:union-entry")?;
        for generation in generations {
            checkpoint(budget, "lexical-retention:union-generation")?;
            let id = generation_id(*generation);
            let known: Option<i64> = self
                .db
                .query_row(
                    "SELECT 1 FROM completed WHERE generation=?1",
                    params![&id[..]],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| storage("check requested generation", error))?;
            if known.is_none() {
                return Err(refusal("requested closure was not authenticated"));
            }
        }
        if *generations == self.selected {
            return Ok(self.selected_bytes);
        }
        let removed: Vec<_> = self.selected.difference(generations).copied().collect();
        let added: Vec<_> = generations.difference(&self.selected).copied().collect();
        self.db
            .execute_batch("SAVEPOINT lexical_retention_selection")
            .map_err(|error| storage("begin selected-set delta", error))?;
        let mut total = self.selected_bytes;
        let result = (|| {
            for generation in removed {
                self.change_selected_generation(generation, false, &mut total, budget)?;
            }
            for generation in added {
                self.change_selected_generation(generation, true, &mut total, budget)?;
            }
            checkpoint(budget, "lexical-retention:union-commit")?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                if let Err(error) = self.db.execute_batch("RELEASE lexical_retention_selection") {
                    rollback_savepoint(&self.db, "lexical_retention_selection")?;
                    return Err(storage("commit selected-set delta", error));
                }
                self.selected.clone_from(generations);
                self.selected_bytes = total;
                Ok(total)
            }
            Err(error) => {
                rollback_savepoint(&self.db, "lexical_retention_selection")?;
                Err(error)
            }
        }
    }
}

/// One pair-local sequence of exact subset measurements.
pub(crate) struct LexicalRetentionSession<'a> {
    adapter: &'a LexicalAdapter,
    repo_id: RepoId,
    revision_id: RevisionId,
    diagnostic: Option<DiagnosticContextV1>,
    budget: RequestBudgetV1,
    inodes: RetainedInodeMembership,
    closure: ClosureMembership,
    store: Option<ObjectStore>,
    mutation_guards: Vec<(usize, MutexGuard<'a, ()>)>,
    _lifecycle: RwLockReadGuard<'a, ()>,
}

impl<'a> LexicalRetentionSession<'a> {
    pub(crate) fn begin(
        adapter: &'a LexicalAdapter,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        budget: RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Self, CoreError> {
        budget.checkpoint("lexical-retention:begin")?;
        let lifecycle = adapter.directory_lifecycle_read_guard()?;
        let total = adapter.retention_measurement_policy.max_main_file_bytes();
        let half = total
            .checked_div(2)
            .ok_or_else(|| refusal("scratch database split divisor is zero"))?;
        Ok(Self {
            adapter,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            diagnostic: diagnostic.cloned(),
            budget,
            inodes: RetainedInodeMembership::open(half)?,
            closure: ClosureMembership::open(half)?,
            store: None,
            mutation_guards: Vec::new(),
            _lifecycle: lifecycle,
        })
    }

    fn generation_dir(&self, generation: ManifestGeneration) -> std::path::PathBuf {
        self.adapter.index_path(&crate::GenKey {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation,
        })
    }

    /// A missing directory is not pinned or cached. An existing generation's
    /// mutation stripe is held before reading files that scrub may update.
    /// Acquiring while the lifecycle read guard is held must never wait: scrub
    /// takes its mutation stripe before that guard.
    fn pinned_sealed_digest(
        &mut self,
        generation: ManifestGeneration,
    ) -> Result<Option<String>, CoreError> {
        self.budget.checkpoint("lexical-retention:identity")?;
        let directory = self.generation_dir(generation);
        let opened = match crate::sealed_generation::open_generation_dir_nofollow(&directory) {
            Ok(opened) => opened,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage("open sealed generation", error)),
        };
        let key = crate::GenKey {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation,
        };
        let stripe = self.adapter.generation_mutation_stripe(&key)?;
        if !self
            .mutation_guards
            .iter()
            .any(|(held, _guard)| *held == stripe)
        {
            let mutex = self
                .adapter
                .generation_mutations
                .get(stripe)
                .ok_or_else(|| {
                    CoreError::Storage("lexical retention mutation stripe outside table".into())
                })?;
            let guard = match mutex.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::WouldBlock) => {
                    return Err(CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                        message: format!(
                            "lexical retention: generation {} mutation stripe {stripe} is busy; retry measurement",
                            generation.get()
                        ),
                    });
                }
                Err(TryLockError::Poisoned(error)) => {
                    return Err(CoreError::Storage(format!(
                        "lexical retention mutation stripe poisoned: {error}"
                    )));
                }
            };
            // Retain the stripe through identity and the complete pair session.
            self.mutation_guards.push((stripe, guard));
        }
        let identity = crate::index_store::read_lexical_sealed_identity_at(&directory, &opened)?;
        if identity.repo_id != self.repo_id
            || identity.revision_id != self.revision_id
            || identity.track != SearchPlaneTrackKind::Lexical
            || identity.manifest_generation != generation
        {
            return Err(CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityScopeMismatch,
                message: format!(
                    "lexical retention: sealed identity does not own {}",
                    directory.display()
                ),
            });
        }
        Ok(Some(identity.manifest_digest))
    }

    fn measure_inner(
        &mut self,
        generations: &BTreeSet<ManifestGeneration>,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        self.budget.checkpoint("lexical-retention:entry")?;
        let mut present = BTreeSet::new();
        let mut absent = BTreeSet::new();
        for &generation in generations {
            self.budget.checkpoint("lexical-retention:generation")?;
            let Some(digest) = self.pinned_sealed_digest(generation)? else {
                let _new_absence = absent.insert(generation);
                continue;
            };
            if !self.closure.cached(generation, &digest)? {
                let directory = self.generation_dir(generation);
                let roots = crate::lexical_gc::protected_roots(&directory)?;
                if !roots.is_empty() && self.store.is_none() {
                    self.store = Some(self.adapter.open_existing_store()?);
                }
                self.closure.record_generation_with_budget(
                    self.store.as_ref(),
                    generation,
                    &digest,
                    &roots,
                    &self.budget,
                    diagnostic,
                )?;
            }
            let _new_present = present.insert(generation);
        }
        let pair_dir = self.adapter.state_root.join(
            GenerationStorageKeyV1::for_repo_revision(&self.repo_id, &self.revision_id).as_str(),
        );
        let adapter = self.adapter;
        let repo_id = &self.repo_id;
        let revision_id = &self.revision_id;
        let inode_bytes = self.inodes.measure_with_budget(
            &self.adapter.state_root,
            &present,
            &|generation| {
                adapter.index_path(&crate::GenKey {
                    repo_id: repo_id.clone(),
                    revision_id: revision_id.clone(),
                    generation,
                })
            },
            &is_writer_lock_entry,
            &self.budget,
            diagnostic,
        )?;
        if !inode_bytes.absent.is_empty() {
            return Err(storage(
                "measure present generation",
                format!(
                    "{} disappeared while lifecycle guard was held",
                    pair_dir.display()
                ),
            ));
        }
        let object_bytes = self
            .closure
            .union_bytes_with_budget(&present, &self.budget)?;
        let bytes = inode_bytes
            .bytes
            .checked_add(object_bytes)
            .ok_or_else(|| refusal("retained byte union overflow"))?;
        self.budget.checkpoint("lexical-retention:complete")?;
        Ok(SealedGenerationBytesV1 { bytes, absent })
    }
}

impl SealedGenerationByteSession for LexicalRetentionSession<'_> {
    fn measure(
        &mut self,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        let diagnostic = self.diagnostic.clone();
        crate::causal_profile::observe_result(
            diagnostic.as_ref(),
            DiagnosticStageV1::RetentionMeasure,
            |diagnostic| self.measure_inner(generations, diagnostic),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::{Arc, Mutex};

    use crate::lexical_store::AttemptBinding;
    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticEmitV1, DiagnosticEventKindV1,
        DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
    };

    use super::*;

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

    fn set(values: &[u64]) -> BTreeSet<ManifestGeneration> {
        values
            .iter()
            .copied()
            .map(ManifestGeneration::new)
            .collect()
    }

    #[test]
    fn closure_trace_charges_only_new_authentication_on_shared_and_cached_paths()
    -> Result<(), Box<dyn Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let directory = tempfile::tempdir()?;
        let store = ObjectStore::open(directory.path())?;
        let lease = store.begin_attempt(&binding(), &[])?;
        let shared = store.put_blob(&lease, ObjectKind::Record, &[], b"shared")?;
        let left = store.put_blob(
            &lease,
            ObjectKind::Record,
            std::slice::from_ref(&shared.root),
            b"left",
        )?;
        let right = store.put_blob(
            &lease,
            ObjectKind::Record,
            std::slice::from_ref(&shared.root),
            b"right",
        )?;
        let sink = Arc::new(RecordingSink::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([9; 16]),
            Arc::new(FixedClock),
            sink.clone(),
        );
        let root = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| format!("trace root was not admitted: {error:?}"))?;
        let child = context.child_context(&root);
        let membership = ClosureMembership::open(1024 * 1024)?;
        membership.record_generation(
            Some(&store),
            ManifestGeneration::new(1),
            "g1",
            std::slice::from_ref(&left.root),
            Some(&child),
        )?;
        membership.record_generation(
            Some(&store),
            ManifestGeneration::new(2),
            "g2",
            std::slice::from_ref(&right.root),
            Some(&child),
        )?;
        let fresh = store.put_blob(
            &lease,
            ObjectKind::Record,
            std::slice::from_ref(&shared.root),
            b"fresh before missing",
        )?;
        let missing = ObjectRef {
            kind: ObjectKind::Record,
            bytes: 1,
            sha256: [u8::MAX; 32],
        };
        check!(fresh.root.sha256 < missing.sha256);
        check!(
            membership
                .record_generation(
                    Some(&store),
                    ManifestGeneration::new(3),
                    "g3",
                    &[fresh.root.clone(), missing],
                    Some(&child),
                )
                .is_err()
        );
        check!(!membership.cached(ManifestGeneration::new(3), "g3")?);
        membership.record_generation(
            Some(&store),
            ManifestGeneration::new(2),
            "g2",
            std::slice::from_ref(&right.root),
            Some(&child),
        )?;
        root.finish(DiagnosticTerminalV1::Ok)?;
        let events = sink
            .0
            .lock()
            .map_err(|error| format!("trace sink lock: {error}"))?;
        let values = |key| {
            events
                .iter()
                .filter_map(|event| {
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
                .collect::<Vec<_>>()
        };
        check_eq!(values(DiagnosticCounterV1::UniqueClosureRefs), [2, 1, 1, 0]);
        check_eq!(
            values(DiagnosticCounterV1::ClosureAuthenticatedBytes),
            [
                left.root.bytes + shared.root.bytes,
                right.root.bytes,
                fresh.root.bytes,
                0
            ]
        );
        check_eq!(values(DiagnosticCounterV1::FilesRead), [2, 1, 1, 0]);
        check_eq!(values(DiagnosticCounterV1::SqliteBatches), [3, 3, 3, 0]);
        check_eq!(values(DiagnosticCounterV1::SqliteRows), [2, 2, 0, 0]);
        lease.discard_unsealed()?;
        Ok(())
    }

    #[test]
    fn shared_dag_subsets_are_exact_and_failed_probe_can_retry() -> Result<(), Box<dyn Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let directory = tempfile::tempdir()?;
        let store = ObjectStore::open(directory.path())?;
        let lease = store.begin_attempt(&binding(), &[])?;
        let shared = store.put_blob(&lease, ObjectKind::Record, &[], b"shared")?;
        let left = store.put_blob(
            &lease,
            ObjectKind::Record,
            std::slice::from_ref(&shared.root),
            b"left",
        )?;
        let right = store.put_blob(
            &lease,
            ObjectKind::Record,
            std::slice::from_ref(&shared.root),
            b"right",
        )?;
        let mut membership = ClosureMembership::open(1024 * 1024)?;
        membership.record_generation(
            Some(&store),
            ManifestGeneration::new(1),
            "g1",
            std::slice::from_ref(&left.root),
            None,
        )?;
        membership.record_generation(
            Some(&store),
            ManifestGeneration::new(2),
            "g2",
            std::slice::from_ref(&right.root),
            None,
        )?;
        let left_gold = left.root.bytes + shared.root.bytes;
        let right_gold = right.root.bytes + shared.root.bytes;
        let both_gold = left.root.bytes + right.root.bytes + shared.root.bytes;
        check_eq!(membership.union_bytes(&set(&[1]))?, left_gold);
        check_eq!(membership.union_bytes(&set(&[1, 2]))?, both_gold);
        check_eq!(membership.union_bytes(&set(&[2]))?, right_gold);
        check_eq!(membership.union_bytes(&set(&[]))?, 0);
        check_eq!(membership.union_bytes(&set(&[1, 2]))?, both_gold);

        let missing = ObjectRef {
            kind: ObjectKind::Record,
            bytes: 1,
            sha256: [u8::MAX; 32],
        };
        check!(
            membership
                .record_generation(
                    Some(&store),
                    ManifestGeneration::new(3),
                    "g3",
                    &[right.root.clone(), missing],
                    None,
                )
                .is_err()
        );
        check!(!membership.cached(ManifestGeneration::new(3), "g3")?);
        check_eq!(membership.union_bytes(&set(&[1, 2]))?, both_gold);
        membership.record_generation(
            Some(&store),
            ManifestGeneration::new(3),
            "g3",
            std::slice::from_ref(&right.root),
            None,
        )?;
        let high_generation = ManifestGeneration::new(
            u64::try_from(i64::MAX)?
                .checked_add(1)
                .ok_or("generation overflow")?,
        );
        membership.record_generation(
            Some(&store),
            high_generation,
            "high",
            std::slice::from_ref(&shared.root),
            None,
        )?;
        check_eq!(membership.union_bytes(&set(&[1, 3]))?, both_gold);
        check_eq!(
            membership.union_bytes(&BTreeSet::from([high_generation]))?,
            shared.root.bytes
        );
        check!(membership.cached(ManifestGeneration::new(3), "g3")?);
        check!(
            membership
                .cached(ManifestGeneration::new(3), "forged")
                .is_err()
        );
        lease.discard_unsealed()?;
        Ok(())
    }

    #[test]
    fn inode_membership_counts_hardlink_once_and_reprobes_absence() -> Result<(), Box<dyn Error>> {
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let directory = tempfile::tempdir()?;
        let track = directory.path().join("track");
        let family = track.join("family");
        std::fs::create_dir_all(family.join("g1"))?;
        std::fs::create_dir(family.join("g2"))?;
        std::fs::write(family.join("g1/shared"), b"12345")?;
        std::fs::hard_link(family.join("g1/shared"), family.join("g2/shared"))?;
        std::fs::write(family.join("g1/one"), b"12")?;
        std::fs::write(family.join("g2/two"), b"123")?;
        let mut inodes = RetainedInodeMembership::open(1024 * 1024)?;
        let path = |generation: ManifestGeneration| family.join(format!("g{}", generation.get()));
        let skip = |_name: &str| false;
        let first = inodes.measure(&track, &set(&[1, 3]), &path, &skip, None)?;
        check_eq!(first.bytes, 7);
        check_eq!(first.absent, set(&[3]));
        check_eq!(
            inodes
                .measure(&track, &set(&[1, 2]), &path, &skip, None)?
                .bytes,
            10
        );
        check_eq!(
            inodes
                .measure(&track, &set(&[2]), &path, &skip, None)?
                .bytes,
            8
        );
        std::fs::create_dir(family.join("g3"))?;
        std::fs::write(family.join("g3/new"), b"1234")?;
        let appeared = inodes.measure(&track, &set(&[1, 3]), &path, &skip, None)?;
        check_eq!(appeared.absent, set(&[]));
        check_eq!(appeared.bytes, 11);
        Ok(())
    }

    #[test]
    fn exact_scratch_cap_refuses_and_failed_probe_rolls_back() -> Result<(), Box<dyn Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let mut membership = ClosureMembership::open(128 * 1024)?;
        let roots: Vec<ObjectRef> = (0_u64..10_000)
            .map(|value| {
                let mut sha256 = [0_u8; 32];
                sha256[..8].copy_from_slice(&value.to_be_bytes());
                ObjectRef {
                    kind: ObjectKind::Record,
                    bytes: 10,
                    sha256,
                }
            })
            .collect();
        let failed =
            membership.record_generation(None, ManifestGeneration::new(1), "bounded", &roots, None);
        check!(matches!(failed, Err(CoreError::Storage(_))));
        check!(!membership.cached(ManifestGeneration::new(1), "bounded")?);
        membership.record_generation(None, ManifestGeneration::new(1), "bounded", &[], None)?;
        check_eq!(membership.union_bytes(&set(&[1]))?, 0);
        Ok(())
    }

    #[test]
    fn present_generation_pin_is_nonblocking_and_released_with_session()
    -> Result<(), Box<dyn Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let directory = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(directory.path().join("lexical"));
        let repo_id = RepoId::new("repo")?;
        let revision_id = RevisionId::new("revision")?;
        let generation = ManifestGeneration::new(1);
        let key = crate::GenKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
        };
        let path = adapter.index_path(&key);
        std::fs::create_dir_all(&path)?;
        crate::index_store::persist_lexical_sealed_identity(
            &path,
            &quanta_index_contract::GenerationSnapshot {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: generation,
                manifest_digest: "fixed-test-digest".into(),
            },
        )?;
        let writer = adapter.generation_mutation_guard(&key)?;
        let mut session = LexicalRetentionSession::begin(
            &adapter,
            &repo_id,
            &revision_id,
            RequestBudgetV1::unbounded(),
            None,
        )?;
        check!(matches!(
            session.pinned_sealed_digest(generation),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusRetentionMeasurementBusy,
                ..
            })
        ));
        drop(writer);
        check_eq!(
            session.pinned_sealed_digest(generation)?,
            Some("fixed-test-digest".into())
        );
        let stripe = adapter.generation_mutation_stripe(&key)?;
        check!(
            adapter
                .generation_mutations
                .get(stripe)
                .ok_or("missing mutation stripe")?
                .try_lock()
                .is_err()
        );
        drop(session);
        check!(
            adapter
                .generation_mutations
                .get(stripe)
                .ok_or("missing mutation stripe")?
                .try_lock()
                .is_ok()
        );
        Ok(())
    }

    #[test]
    fn absent_generation_is_not_pinned_and_can_appear_on_retry() -> Result<(), Box<dyn Error>> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition))).into()); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(CoreError::Storage(format!($($message)+)).into()); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!())).into()); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(CoreError::Storage(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+))).into()); }
            }};
        }

        let directory = tempfile::tempdir()?;
        let adapter = LexicalAdapter::with_state_root(directory.path().join("lexical"));
        let repo_id = RepoId::new("repo")?;
        let revision_id = RevisionId::new("revision")?;
        let generation = ManifestGeneration::new(9);
        let key = crate::GenKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
        };
        let mut session = LexicalRetentionSession::begin(
            &adapter,
            &repo_id,
            &revision_id,
            RequestBudgetV1::unbounded(),
            None,
        )?;
        check_eq!(session.pinned_sealed_digest(generation)?, None);
        let stripe = adapter.generation_mutation_stripe(&key)?;
        check!(
            adapter
                .generation_mutations
                .get(stripe)
                .ok_or("missing mutation stripe")?
                .try_lock()
                .is_ok()
        );
        let path = adapter.index_path(&key);
        std::fs::create_dir_all(&path)?;
        crate::index_store::persist_lexical_sealed_identity(
            &path,
            &quanta_index_contract::GenerationSnapshot {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: generation,
                manifest_digest: "appeared".into(),
            },
        )?;
        check_eq!(
            session.pinned_sealed_digest(generation)?,
            Some("appeared".into())
        );
        drop(session);
        Ok(())
    }
}
