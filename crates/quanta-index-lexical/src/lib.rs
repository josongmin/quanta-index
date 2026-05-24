//! Lexical adapter — Tantivy 0.22-backed inverted index.
//!
//! Implements [`LexicalIndexBuildPort`] and [`LexicalIndexOpenPort`] from
//! `quanta-index-core::domains::lexical`.
//!
//! The adapter materializes channel events into a
//! `(repo, revision, generation) -> Tantivy index` directory tree rooted at
//! the adapter's `state_root` and services queries via BM25 over the indexed
//! `chunk_text` field.
//!
//! Layout:
//!   `{state_root}/{repo_id}/{revision_id}/g{generation}/`
//!
//! The adapter caches one `IndexWriter` per generation to amortize the
//! per-commit cost across many ops, and one `IndexReader` per opened
//! generation.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "tantivy 0.22 pulls multiple transitive versions (rustix, linux-raw-sys, windows-sys) we cannot collapse; scoped allowance in deny.toml [bans] skip-tree."
)]

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    LexicalCandidate, LexicalChannelOp, LqExpr, LqQuery, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId,
};
use quanta_index_core::{
    CoreError, LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalSearcher,
    domains::lexical::LexicalPolicy,
};
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser};
use tantivy::schema::{Field, OwnedValue, STORED, STRING, Schema, TEXT, TantivyDocument, Value};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term};

/// Memory budget for a Tantivy `IndexWriter`. Pinned to the upstream-documented
/// minimum so adapter setup is bounded and reproducible across test runs.
const WRITER_MEMORY_BUDGET_BYTES: usize = 15_000_000;

/// Maximum number of open `GenerationWriter` entries cached in memory at once.
///
/// Each `GenerationWriter` holds a Tantivy `IndexWriter` (~15 MiB heap budget per
/// [`WRITER_MEMORY_BUDGET_BYTES`]) plus an mmap-backed `Index` handle, so an
/// unbounded cache would balloon to multi-GiB resident memory and exhaust the
/// open-file table once the producer streams thousands of generations through
/// the adapter. The bound here is a hard-coded const today; a future change can
/// promote it to a configurable adapter parameter.
pub const LEXICAL_WRITER_CACHE_MAX: usize = 16;

/// Schema field handles for the lexical index. Cloned cheaply into every
/// searcher; constructed once per adapter instance.
#[derive(Clone)]
struct SchemaFields {
    schema: Schema,
    candidate_id: Field,
    repo_id: Field,
    revision_id: Field,
    repo_relative_path: Field,
    chunk_text: Field,
}

impl SchemaFields {
    fn build() -> Self {
        let mut builder = Schema::builder();
        let candidate_id = builder.add_text_field("candidate_id", STRING | STORED);
        let repo_id = builder.add_text_field("repo_id", STRING | STORED);
        let revision_id = builder.add_text_field("revision_id", STRING | STORED);
        let repo_relative_path = builder.add_text_field("repo_relative_path", STRING | STORED);
        let chunk_text = builder.add_text_field("chunk_text", TEXT | STORED);
        let schema = builder.build();
        Self {
            schema,
            candidate_id,
            repo_id,
            revision_id,
            repo_relative_path,
            chunk_text,
        }
    }
}

/// Per-generation cache key.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GenKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
}

/// Open writer + index handle for an active generation.
///
/// Shared via `Arc<Mutex<_>>` so multiple `build` invocations for the same
/// generation serialize on a single Tantivy writer (Tantivy writers are not
/// Sync).
struct GenerationWriter {
    index: Index,
    writer: IndexWriter,
}

/// Bounded LRU cache of per-generation Tantivy writers.
///
/// # Capacity
///
/// At most [`LEXICAL_WRITER_CACHE_MAX`] entries are retained. When inserting
/// a new entry would exceed the cap, the least-recently-used entry is evicted.
///
/// # Eviction order
///
/// Recency is tracked via `order`: the back of the deque is most-recently-used,
/// the front is least-recently-used. Both `get_or_open` (on a hit) and the
/// insert path on the miss touch the deque so the freshly accessed key floats
/// to the back. Eviction pops from the front.
///
/// # Commit-on-eviction guarantee
///
/// Evicting an entry calls `IndexWriter::commit` on the writer it owns. The
/// adapter's invariant is that any uncommitted ops belong to an in-flight
/// `build` invocation that holds the entry's `Arc<Mutex<GenerationWriter>>`;
/// the cache only drops its own `Arc`, so an in-flight build is unaffected.
/// The commit on eviction ensures that *cached-but-idle* pending docs (the
/// dispatcher submits ops one at a time today) are not lost when the cache
/// drops its handle. A failing commit is surfaced as
/// `CoreError::Storage("lexical: evict commit: ...")` and aborts the insert.
struct WriterCache {
    entries: BTreeMap<GenKey, Arc<Mutex<GenerationWriter>>>,
    order: VecDeque<GenKey>,
}

impl WriterCache {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Move `key` to the most-recently-used end of `order`. Caller must hold
    /// the cache mutex. Silently no-ops when `key` is absent.
    fn touch(&mut self, key: &GenKey) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            let removed = self.order.remove(pos);
            if let Some(k) = removed {
                self.order.push_back(k);
            }
        }
    }

    /// Evict least-recently-used entries until `entries.len()` is strictly less
    /// than [`LEXICAL_WRITER_CACHE_MAX`]. Called from the insert path before
    /// pushing a new entry, so on return there is room for one more.
    fn evict_until_capacity(&mut self) -> Result<(), CoreError> {
        while self.entries.len() >= LEXICAL_WRITER_CACHE_MAX {
            let Some(victim_key) = self.order.pop_front() else {
                // entries and order are kept in lock-step; an empty order with
                // non-empty entries would be a structural bug.
                return Err(CoreError::Storage(
                    "lexical writer cache: order/entries desync during eviction".to_string(),
                ));
            };
            let Some(victim) = self.entries.remove(&victim_key) else {
                return Err(CoreError::Storage(
                    "lexical writer cache: order references missing entry".to_string(),
                ));
            };
            // Commit any pending docs before dropping the writer. If a
            // concurrent build still holds the Arc this lock contends; that is
            // acceptable because the cap is small (16) and contention only
            // happens during eviction, not on the hot path.
            let mut guarded = victim.lock().map_err(|err| {
                CoreError::Storage(format!("lexical: evict lock poisoned: {err}"))
            })?;
            let _opstamp = guarded
                .writer
                .commit()
                .map_err(|err| CoreError::Storage(format!("lexical: evict commit: {err}")))?;
            drop(guarded);
            drop(victim);
        }
        Ok(())
    }

    /// Returns the cached handle for `key`, opening and inserting a new one if
    /// absent. The returned handle is the entry's `Arc<Mutex<_>>`; the cache
    /// retains its own clone so subsequent calls hit the same writer.
    fn get_or_open(
        &mut self,
        key: &GenKey,
        fields: &SchemaFields,
        path: &Path,
    ) -> Result<Arc<Mutex<GenerationWriter>>, CoreError> {
        if let Some(existing) = self.entries.get(key) {
            let cloned = Arc::clone(existing);
            self.touch(key);
            return Ok(cloned);
        }
        self.evict_until_capacity()?;
        let index = open_or_create_index(fields, path)?;
        let writer: IndexWriter = index
            .writer(WRITER_MEMORY_BUDGET_BYTES)
            .map_err(|err| CoreError::Storage(format!("lexical: writer: {err}")))?;
        let handle = Arc::new(Mutex::new(GenerationWriter { index, writer }));
        let _prior = self.entries.insert(key.clone(), Arc::clone(&handle));
        self.order.push_back(key.clone());
        Ok(handle)
    }

    /// Test/inspection hook: number of cached writers. Exposed `pub(crate)`
    /// for the LRU eviction integration test.
    fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns a clone of the cached writer's `Index` handle for `key`, or
    /// `None` if no entry is cached. Does NOT update LRU recency: opening a
    /// searcher is observational and should not contend with the build-side
    /// LRU ordering. Locks the entry's inner `Mutex<GenerationWriter>` to read
    /// the `Index`; callers must not be holding the cache lock when this
    /// blocks for long.
    fn peek_index(&self, key: &GenKey) -> Result<Option<Index>, CoreError> {
        let Some(handle) = self.entries.get(key) else {
            return Ok(None);
        };
        let guarded = handle
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writer poisoned: {err}")))?;
        Ok(Some(guarded.index.clone()))
    }
}

/// Open or create the Tantivy index at `path` under the adapter's schema.
///
/// Free function rather than a method so [`WriterCache`] can call it without
/// holding a reference to the adapter (which would require re-entering the
/// cache mutex).
fn open_or_create_index(fields: &SchemaFields, path: &Path) -> Result<Index, CoreError> {
    std::fs::create_dir_all(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: create generation directory {}: {err}",
            path.display()
        ))
    })?;
    let directory = tantivy::directory::MmapDirectory::open(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: open generation directory {}: {err}",
            path.display()
        ))
    })?;
    Index::builder()
        .schema(fields.schema.clone())
        .open_or_create(directory)
        .map_err(|err| CoreError::Storage(format!("lexical: open generation index: {err}")))
}

/// Tantivy-backed lexical adapter.
pub struct LexicalAdapter {
    state_root: PathBuf,
    fields: SchemaFields,
    writers: Arc<Mutex<WriterCache>>,
}

impl LexicalAdapter {
    /// Construct an adapter rooted at the given directory. The directory will
    /// be created lazily as generations are materialized.
    #[must_use]
    pub fn with_state_root(state_root: PathBuf) -> Self {
        Self {
            state_root,
            fields: SchemaFields::build(),
            writers: Arc::new(Mutex::new(WriterCache::new())),
        }
    }

    fn index_path(&self, key: &GenKey) -> PathBuf {
        self.state_root
            .join(key.repo_id.as_str())
            .join(key.revision_id.as_str())
            .join(format!("g{}", key.generation.get()))
    }

    fn writer_handle(&self, key: &GenKey) -> Result<Arc<Mutex<GenerationWriter>>, CoreError> {
        let path = self.index_path(key);
        let mut guard = self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
        guard.get_or_open(key, &self.fields, &path)
    }

    /// Returns the current number of cached writers. Exposed for the LRU
    /// eviction integration test; not part of the stable adapter surface.
    #[doc(hidden)]
    pub fn writer_cache_len(&self) -> Result<usize, CoreError> {
        let guard = self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
        Ok(guard.len())
    }

    fn apply_op(
        &self,
        writer: &IndexWriter,
        key: &GenKey,
        op: &LexicalChannelOp,
    ) -> Result<bool, CoreError> {
        match op {
            LexicalChannelOp::UpsertChunk(upsert) => {
                let candidate_id = upsert.chunk_id.as_str();
                let term = Term::from_field_text(self.fields.candidate_id, candidate_id);
                let _opstamp = writer.delete_term(term);
                let text = String::from_utf8_lossy(&upsert.payload).into_owned();
                let mut doc = TantivyDocument::new();
                doc.add_text(self.fields.candidate_id, candidate_id);
                doc.add_text(self.fields.repo_id, key.repo_id.as_str());
                doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                doc.add_text(self.fields.repo_relative_path, "");
                doc.add_text(self.fields.chunk_text, text);
                let _opstamp = writer
                    .add_document(doc)
                    .map_err(|err| CoreError::Storage(format!("lexical: add_document: {err}")))?;
                Ok(true)
            }
            LexicalChannelOp::DeleteChunk(delete) => {
                let term =
                    Term::from_field_text(self.fields.candidate_id, delete.chunk_id.as_str());
                let _opstamp = writer.delete_term(term);
                Ok(true)
            }
            // FullBundle/Seal carry no document-level effect (dispatcher's
            // ledger update observes Seal). Symbol ops are out-of-scope for
            // the lexical chunk index.
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::DeleteSymbol(_) => Ok(false),
        }
    }
}

impl LexicalIndexBuildPort for LexicalAdapter {
    fn build(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        if ops.is_empty() {
            return Ok(());
        }
        // All ops in a single `build` invocation must share the (repo, rev, gen)
        // triple. The dispatcher feeds us one op per call today; defending the
        // invariant here keeps the adapter safe if that changes.
        for op in ops {
            if op.repo_id() != repo || op.revision_id() != revision || op.generation() != generation
            {
                return Err(CoreError::InvalidContract(
                    "lexical: op (repo, revision, generation) mismatch with batch key".to_string(),
                ));
            }
        }
        let key = GenKey {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        };
        let handle = self.writer_handle(&key)?;
        self.commit_ops_under_lock(&handle, &key, ops)
    }
}

impl LexicalAdapter {
    #[expect(
        clippy::significant_drop_tightening,
        reason = "writer guard must span the full op-apply + commit so partial commits cannot interleave with sibling builds for the same generation"
    )]
    fn commit_ops_under_lock(
        &self,
        handle: &Arc<Mutex<GenerationWriter>>,
        key: &GenKey,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        let mut guarded = handle
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writer poisoned: {err}")))?;
        let mut needs_commit = false;
        for op in ops {
            if self.apply_op(&guarded.writer, key, op)? {
                needs_commit = true;
            }
        }
        if needs_commit {
            let _opstamp = guarded
                .writer
                .commit()
                .map_err(|err| CoreError::Storage(format!("lexical: commit: {err}")))?;
        }
        Ok(())
    }
}

impl LexicalIndexOpenPort for LexicalAdapter {
    fn open(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let key = GenKey {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        };
        let path = self.index_path(&key);
        if !path.exists() {
            return Err(CoreError::NotFound(format!(
                "lexical: no index at {}",
                path.display()
            )));
        }
        // Prefer the cached writer's index handle when present (it reflects
        // commits that may not yet be visible to a freshly-opened reader
        // before its first reload). The peek does NOT touch LRU recency:
        // opening a searcher is observational, not a build-side access.
        let cached_index: Option<Index> = {
            let guard = self
                .writers
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
            guard.peek_index(&key)?
        };
        let index = match cached_index {
            Some(idx) => idx,
            None => open_or_create_index(&self.fields, &path)?,
        };
        let reader: IndexReader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|err| CoreError::Storage(format!("lexical: reader: {err}")))?;
        reader
            .reload()
            .map_err(|err| CoreError::Storage(format!("lexical: reader reload: {err}")))?;
        Ok(Box::new(TantivySearcher {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
            fields: self.fields.clone(),
            index,
            reader,
        }))
    }
}

struct TantivySearcher {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    fields: SchemaFields,
    index: Index,
    reader: IndexReader,
}

impl TantivySearcher {
    fn compile(&self, expr: &LqExpr) -> Result<Box<dyn Query>, CoreError> {
        match expr {
            LqExpr::MatchAll => Err(CoreError::InvalidContract(
                "lexical: MatchAll not compilable (must be rejected by policy)".to_string(),
            )),
            LqExpr::Raw(text) => {
                let parser = QueryParser::for_index(&self.index, vec![self.fields.chunk_text]);
                let parsed = parser
                    .parse_query(text)
                    .map_err(|err| CoreError::InvalidContract(format!("lexical: parse: {err}")))?;
                Ok(parsed)
            }
            LqExpr::All(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((Occur::Must, self.compile(part)?));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Any(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((Occur::Should, self.compile(part)?));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Not(inner) => {
                let inner_q = self.compile(inner)?;
                let clauses: Vec<(Occur, Box<dyn Query>)> =
                    vec![(Occur::Must, Box::new(AllQuery)), (Occur::MustNot, inner_q)];
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
        }
    }

    fn document_to_candidate(
        &self,
        doc: &TantivyDocument,
        score: f32,
    ) -> Result<LexicalCandidate, CoreError> {
        let candidate_id = stored_text(doc, self.fields.candidate_id).ok_or_else(|| {
            CoreError::Storage("lexical: stored doc missing candidate_id field".to_string())
        })?;
        let snippet = stored_text(doc, self.fields.chunk_text).unwrap_or_default();
        let repo_relative_path =
            stored_text(doc, self.fields.repo_relative_path).unwrap_or_default();
        Ok(LexicalCandidate {
            candidate_id,
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            manifest_generation: self.generation,
            repo_relative_path: RepoRelativePath::new(repo_relative_path),
            start_line: 0,
            end_line: 0,
            score,
            snippet,
        })
    }
}

fn stored_text(doc: &TantivyDocument, field: Field) -> Option<String> {
    let value: &OwnedValue = doc.get_first(field)?;
    Value::as_str(&value).map(str::to_owned)
}

impl LexicalSearcher for TantivySearcher {
    fn search(&self, query: &LqQuery, top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError> {
        LexicalPolicy::validate_query(query)?;
        let compiled = self.compile(&query.expr)?;
        let limit = usize::try_from(top_k)
            .map_err(|err| CoreError::InvalidContract(format!("lexical: top_k: {err}")))?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(limit))
            .map_err(|err| CoreError::Storage(format!("lexical: search: {err}")))?;
        let mut out: Vec<LexicalCandidate> = Vec::with_capacity(hits.len());
        for (score, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_candidate(&doc, score)?);
        }
        Ok(out)
    }
}
