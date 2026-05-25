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

pub mod filters;
pub mod phrase;
pub mod plan;
pub mod planner;
pub mod regex;
pub mod symbol;
pub mod trigram_plan;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use quanta_index_contract::lex::SymbolRecord;
use quanta_index_contract::{
    ChunkRecord, LexicalCandidate, LexicalChannelOp, LexicalRepoMetadataRecord, LqExpr,
    LqFileScope, LqFilter, LqLeaf, LqPatternType, LqPredicateArg, LqQuery, LqSelect, LqType,
    LqVisibility, LqYesNoOnly, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
};
use quanta_index_core::{
    CoreError, LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalSearcher,
    domains::lexical::LexicalPolicy,
};

use crate::regex::RegexPolicy;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser, RegexQuery, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, OwnedValue, STORED, STRING, Schema, TEXT, TantivyDocument, Value,
};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Term};

/// Memory budget for a Tantivy `IndexWriter`. Pinned to the upstream-documented
/// minimum so adapter setup is bounded and reproducible across test runs.
const WRITER_MEMORY_BUDGET_BYTES: usize = 15_000_000;
const TEXT_DOC_KIND: &str = "text";
const SYMBOL_DOC_KIND: &str = "symbol";
const REPO_METADATA_FILE_NAME: &str = "repo-metadata.cbor";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueryDocKind {
    Text,
    Symbol,
}

impl QueryDocKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Text => TEXT_DOC_KIND,
            Self::Symbol => SYMBOL_DOC_KIND,
        }
    }
}

/// Maximum number of open `GenerationWriter` entries cached in memory at once.
///
/// Each `GenerationWriter` holds a Tantivy `IndexWriter` (~15 MiB heap budget per
/// `WRITER_MEMORY_BUDGET_BYTES`) plus an mmap-backed `Index` handle, so an
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
    doc_kind: Field,
    repo_relative_path: Field,
    file_name: Field,
    language: Field,
    start_line: Field,
    end_line: Field,
    chunk_text: Field,
}

impl SchemaFields {
    fn build() -> Self {
        let mut builder = Schema::builder();
        let candidate_id = builder.add_text_field("candidate_id", STRING | STORED);
        let repo_id = builder.add_text_field("repo_id", STRING | STORED);
        let revision_id = builder.add_text_field("revision_id", STRING | STORED);
        let doc_kind = builder.add_text_field("doc_kind", STRING | STORED);
        let repo_relative_path = builder.add_text_field("repo_relative_path", STRING | STORED);
        let file_name = builder.add_text_field("file_name", STRING);
        let language = builder.add_text_field("language", STRING);
        let start_line = builder.add_u64_field("start_line", STORED);
        let end_line = builder.add_u64_field("end_line", STORED);
        let chunk_text = builder.add_text_field("chunk_text", TEXT | STORED);
        let schema = builder.build();
        Self {
            schema,
            candidate_id,
            repo_id,
            revision_id,
            doc_kind,
            repo_relative_path,
            file_name,
            language,
            start_line,
            end_line,
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

fn decode_chunk_payload(bytes: &[u8]) -> Result<ChunkRecord, CoreError> {
    ciborium::from_reader::<ChunkRecord, _>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: chunk payload decode: {err}")))
}

fn decode_symbol_payload(bytes: &[u8]) -> Result<SymbolRecord, CoreError> {
    ciborium::from_reader::<SymbolRecord, _>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: symbol payload decode: {err}")))
}

fn decode_repo_metadata_payload(bytes: &[u8]) -> Option<LexicalRepoMetadataRecord> {
    if bytes.is_empty() {
        return None;
    }
    // Historical FullBundle payloads were opaque producer-side blobs and many
    // existing tests still write `b"manifest"` here. Treat undecodable bytes
    // as legacy/no-metadata instead of breaking unrelated lexical flows.
    ciborium::from_reader::<LexicalRepoMetadataRecord, _>(bytes)
        .into_iter()
        .next()
}

fn persist_repo_metadata_snapshot(
    path: &Path,
    metadata: Option<&LexicalRepoMetadataRecord>,
) -> Result<(), CoreError> {
    match metadata {
        Some(metadata) => {
            let mut payload = Vec::new();
            ciborium::into_writer(metadata, &mut payload).map_err(|err| {
                CoreError::InvalidContract(format!("lexical: repo metadata encode: {err}"))
            })?;
            std::fs::write(path, payload).map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: write repo metadata snapshot {}: {err}",
                    path.display()
                ))
            })?;
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(CoreError::Storage(format!(
                    "lexical: remove repo metadata snapshot {}: {err}",
                    path.display()
                )));
            }
        },
    }
    Ok(())
}

fn load_repo_metadata_snapshot(
    path: &Path,
) -> Result<Option<LexicalRepoMetadataRecord>, CoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let payload = std::fs::read(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: read repo metadata snapshot {}: {err}",
            path.display()
        ))
    })?;
    ciborium::from_reader::<LexicalRepoMetadataRecord, _>(payload.as_slice())
        .map(Some)
        .map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: repo metadata decode {}: {err}",
                path.display()
            ))
        })
}

fn normalize_language(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn file_name_for_path(path: &str) -> Option<&str> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
}

fn strip_regex_delimiters(text: &str) -> Option<&str> {
    text.strip_prefix('/')
        .and_then(|trimmed| trimmed.strip_suffix('/'))
}

fn collapse_exprs(items: Vec<LqExpr>, all: bool) -> LqExpr {
    match items.len() {
        0 => LqExpr::Empty,
        1 => items.into_iter().next().map_or(LqExpr::Empty, |item| item),
        _ if all => LqExpr::All(items),
        _ => LqExpr::Any(items),
    }
}

fn add_metadata_fields(
    fields: &SchemaFields,
    doc: &mut TantivyDocument,
    repo_relative_path: &str,
    language: Option<&str>,
) {
    doc.add_text(fields.repo_relative_path, repo_relative_path);
    if let Some(file_name) = file_name_for_path(repo_relative_path) {
        doc.add_text(fields.file_name, file_name);
    }
    if let Some(language) = language.and_then(normalize_language) {
        doc.add_text(fields.language, &language);
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
    repo_metadata: Arc<Mutex<BTreeMap<GenKey, LexicalRepoMetadataRecord>>>,
    /// Per-deployment regex policy injected at construction time.
    ///
    /// Owned by the adapter (not fabricated at the leaf call site) so all
    /// content-side regex leaves see the same dialect/literal/trigram-cap
    /// configuration. The defaults from [`RegexPolicy::defaults`] are fine
    /// for the in-tree configuration; operator-facing tightening is plumbed
    /// here rather than at the call site.
    regex_policy: RegexPolicy,
}

impl LexicalAdapter {
    /// Construct an adapter rooted at the given directory with the default
    /// [`RegexPolicy`]. The directory will be created lazily as generations
    /// are materialized.
    #[must_use]
    pub fn with_state_root(state_root: PathBuf) -> Self {
        Self::with_state_root_and_regex_policy(state_root, RegexPolicy::defaults())
    }

    /// Construct an adapter rooted at the given directory with an explicit
    /// [`RegexPolicy`]. Use this constructor when the deployment needs to
    /// tighten or relax the regex dialect / candidate cap /
    /// trigram-missing threshold defaults.
    #[must_use]
    pub fn with_state_root_and_regex_policy(
        state_root: PathBuf,
        regex_policy: RegexPolicy,
    ) -> Self {
        Self {
            state_root,
            fields: SchemaFields::build(),
            writers: Arc::new(Mutex::new(WriterCache::new())),
            repo_metadata: Arc::new(Mutex::new(BTreeMap::new())),
            regex_policy,
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

    fn repo_metadata_path(&self, key: &GenKey) -> PathBuf {
        self.index_path(key).join(REPO_METADATA_FILE_NAME)
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

    fn repo_metadata_for_key(
        &self,
        key: &GenKey,
    ) -> Result<Option<LexicalRepoMetadataRecord>, CoreError> {
        let cached = self
            .repo_metadata
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical repo metadata poisoned: {err}")))?
            .get(key)
            .cloned();
        if cached.is_some() {
            return Ok(cached);
        }
        load_repo_metadata_snapshot(&self.repo_metadata_path(key))
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
                let chunk = decode_chunk_payload(&upsert.payload)?;
                let mut doc = TantivyDocument::new();
                doc.add_text(self.fields.candidate_id, candidate_id);
                doc.add_text(self.fields.repo_id, key.repo_id.as_str());
                doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                doc.add_text(self.fields.doc_kind, TEXT_DOC_KIND);
                add_metadata_fields(
                    &self.fields,
                    &mut doc,
                    chunk.repo_relative_path.as_str(),
                    Some(chunk.language.as_ref()),
                );
                doc.add_u64(self.fields.start_line, u64::from(chunk.start_line));
                doc.add_u64(self.fields.end_line, u64::from(chunk.end_line));
                doc.add_text(self.fields.chunk_text, chunk.snippet.as_ref());
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
            LexicalChannelOp::UpsertSymbol(upsert) => {
                let candidate_id = upsert.symbol_id.as_str();
                let term = Term::from_field_text(self.fields.candidate_id, candidate_id);
                let _opstamp = writer.delete_term(term);
                let symbol = decode_symbol_payload(&upsert.payload)?;
                let mut doc = TantivyDocument::new();
                doc.add_text(self.fields.candidate_id, candidate_id);
                doc.add_text(self.fields.repo_id, key.repo_id.as_str());
                doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                doc.add_text(self.fields.doc_kind, SYMBOL_DOC_KIND);
                add_metadata_fields(
                    &self.fields,
                    &mut doc,
                    symbol.span.path.as_ref(),
                    Some(symbol.lang.as_code_str()),
                );
                doc.add_u64(self.fields.start_line, u64::from(symbol.span.line_start));
                doc.add_u64(self.fields.end_line, u64::from(symbol.span.line_end));
                let snippet = match symbol.container_name.as_deref() {
                    Some(container) if !container.is_empty() => {
                        format!("{} {}", symbol.name.as_ref(), container)
                    }
                    _ => symbol.name.as_ref().to_string(),
                };
                doc.add_text(self.fields.chunk_text, &snippet);
                let _opstamp = writer
                    .add_document(doc)
                    .map_err(|err| CoreError::Storage(format!("lexical: add_document: {err}")))?;
                Ok(true)
            }
            LexicalChannelOp::DeleteSymbol(delete) => {
                let term =
                    Term::from_field_text(self.fields.candidate_id, delete.symbol_id.as_str());
                let _opstamp = writer.delete_term(term);
                Ok(true)
            }
            // FullBundle/Seal carry no document-level effect (dispatcher's
            // ledger update observes Seal).
            LexicalChannelOp::FullBundle(bundle) => {
                let metadata = decode_repo_metadata_payload(&bundle.payload);
                {
                    let mut guard = self.repo_metadata.lock().map_err(|err| {
                        CoreError::Storage(format!("lexical repo metadata poisoned: {err}"))
                    })?;
                    match metadata.clone() {
                        Some(metadata) => {
                            let _prior = guard.insert(key.clone(), metadata);
                        }
                        None => {
                            let _prior = guard.remove(key);
                        }
                    }
                }
                persist_repo_metadata_snapshot(&self.repo_metadata_path(key), metadata.as_ref())?;
                Ok(false)
            }
            LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertTag(_)
            | LexicalChannelOp::DeleteRef(_)
            | LexicalChannelOp::DeleteTag(_)
            | LexicalChannelOp::UpsertDirty(_)
            | LexicalChannelOp::EvictDirty(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::DeleteParseTree(_)
            | LexicalChannelOp::UpsertDiffHunk(_) => Ok(false),
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
        let repo_metadata = self.repo_metadata_for_key(&key)?;
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
            repo_metadata,
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
    repo_metadata: Option<LexicalRepoMetadataRecord>,
}

#[derive(Clone)]
enum RepoHasFileMatcher {
    Path(String),
    Name(String),
}

struct RepoHasFileConstraint {
    matchers: Vec<RepoHasFileMatcher>,
}

struct PreparedPredicatePlan {
    expr: LqExpr,
    allowed_paths: Option<BTreeSet<String>>,
    force_empty: bool,
}

impl TantivySearcher {
    fn missing_repo_metadata_error(filter_name: &str) -> CoreError {
        CoreError::NotReady(format!(
            "lexical: repo metadata snapshot missing for filter `{filter_name}` in LexicalFullBundle.payload"
        ))
    }

    fn exact_text_query(&self, field: Field, value: &str) -> Box<dyn Query> {
        Box::new(TermQuery::new(
            Term::from_field_text(field, value),
            IndexRecordOption::Basic,
        ))
    }

    fn regex_text_query(&self, field: Field, pattern: &str) -> Result<Box<dyn Query>, CoreError> {
        let query = RegexQuery::from_pattern(pattern, field).map_err(|err| {
            CoreError::InvalidContract(format!("lexical: regex filter compile: {err}"))
        })?;
        Ok(Box::new(query))
    }

    fn compile_file_filter(
        &self,
        pattern: &str,
        scope: LqFileScope,
    ) -> Result<Box<dyn Query>, CoreError> {
        let path_query = self.regex_text_query(self.fields.repo_relative_path, pattern)?;
        match scope {
            LqFileScope::PathOnly => Ok(path_query),
            LqFileScope::NameAndPath => Ok(Box::new(BooleanQuery::new(vec![
                (Occur::Should, path_query),
                (
                    Occur::Should,
                    self.regex_text_query(self.fields.file_name, pattern)?,
                ),
            ]))),
        }
    }

    fn query_parser(&self) -> QueryParser {
        QueryParser::for_index(
            &self.index,
            vec![self.fields.chunk_text, self.fields.repo_relative_path],
        )
    }

    fn expr_contains_predicate(expr: &LqExpr) -> bool {
        match expr {
            LqExpr::Leaf(LqLeaf::Predicate { .. }) => true,
            LqExpr::All(parts) | LqExpr::Any(parts) => {
                parts.iter().any(Self::expr_contains_predicate)
            }
            LqExpr::Not(inner) => Self::expr_contains_predicate(inner),
            LqExpr::Empty | LqExpr::Leaf(_) | LqExpr::SemanticVector { .. } => false,
        }
    }

    fn path_restriction_query(&self, paths: &BTreeSet<String>) -> Box<dyn Query> {
        if paths.len() == 1
            && let Some(path) = paths.iter().next()
        {
            return self.exact_text_query(self.fields.repo_relative_path, path);
        }
        Box::new(BooleanQuery::new(
            paths
                .iter()
                .map(|path| {
                    (
                        Occur::Should,
                        self.exact_text_query(self.fields.repo_relative_path, path),
                    )
                })
                .collect(),
        ))
    }

    fn with_doc_kind(&self, query: Box<dyn Query>, doc_kind: &str) -> Box<dyn Query> {
        let doc_kind_term = Term::from_field_text(self.fields.doc_kind, doc_kind);
        Box::new(BooleanQuery::new(vec![
            (Occur::Must, query),
            (
                Occur::Must,
                Box::new(TermQuery::new(doc_kind_term, IndexRecordOption::Basic)),
            ),
        ]))
    }

    fn collect_matching_paths_for_leaf(
        &self,
        leaf: &LqLeaf,
    ) -> Result<BTreeSet<String>, CoreError> {
        let compiled = self.with_doc_kind(
            self.compile_leaf(leaf, LqPatternType::Standard)?,
            TEXT_DOC_KIND,
        );
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting predicate scope: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(BTreeSet::new());
        }
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(limit))
            .map_err(|err| CoreError::Storage(format!("lexical: predicate scope search: {err}")))?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (_, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if let Some(path) = stored_text(&doc, self.fields.repo_relative_path) {
                let _inserted: bool = out.insert(path);
            }
        }
        Ok(out)
    }

    fn repo_has_file_matches(&self, constraint: &RepoHasFileConstraint) -> Result<bool, CoreError> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> =
            Vec::with_capacity(constraint.matchers.len());
        for matcher in &constraint.matchers {
            let query = match matcher {
                RepoHasFileMatcher::Path(pattern) => {
                    self.regex_text_query(self.fields.repo_relative_path, pattern)?
                }
                RepoHasFileMatcher::Name(pattern) => {
                    self.regex_text_query(self.fields.file_name, pattern)?
                }
            };
            clauses.push((Occur::Must, query));
        }
        let compiled = self.with_doc_kind(Box::new(BooleanQuery::new(clauses)), TEXT_DOC_KIND);
        let searcher = self.reader.searcher();
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(1))
            .map_err(|err| CoreError::Storage(format!("lexical: repo.has.file search: {err}")))?;
        Ok(!hits.is_empty())
    }

    fn predicate_content_leaf(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<LqLeaf, CoreError> {
        if args.len() != 1 {
            return Err(CoreError::Typed {
                code: "LEX_PREDICATE_UNIMPLEMENTED".to_string(),
                message: format!(
                    "lexical: predicate leaf `{name}` requires exactly one scalar argument (owner: LXE-03-predicate-extensions)"
                ),
            });
        }
        let Some(arg) = args.first() else {
            return Err(CoreError::Typed {
                code: "LEX_PREDICATE_UNIMPLEMENTED".to_string(),
                message: format!(
                    "lexical: predicate leaf `{name}` requires exactly one scalar argument (owner: LXE-03-predicate-extensions)"
                ),
            });
        };
        match arg {
            LqPredicateArg::Keyword(value) => {
                if let Some(regex) = strip_regex_delimiters(value) {
                    return Ok(LqLeaf::Regex(regex.to_string()));
                }
                Ok(LqLeaf::Keyword(value.clone()))
            }
            LqPredicateArg::Phrase(value) => Ok(LqLeaf::Phrase(value.clone())),
            LqPredicateArg::RawString(value) => {
                if let Some(regex) = strip_regex_delimiters(value) {
                    return Ok(LqLeaf::Regex(regex.to_string()));
                }
                Ok(LqLeaf::RawString(value.clone()))
            }
            LqPredicateArg::Number(value) => Ok(LqLeaf::Keyword(value.to_string())),
            LqPredicateArg::Filter { .. } => Err(CoreError::Typed {
                code: "LEX_PREDICATE_UNIMPLEMENTED".to_string(),
                message: format!(
                    "lexical: predicate leaf `{name}` filter arguments are not executable on Tantivy adapter (owner: LXE-03-predicate-extensions)"
                ),
            }),
        }
    }

    fn repo_has_file_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<RepoHasFileConstraint, CoreError> {
        let mut matchers: Vec<RepoHasFileMatcher> = Vec::new();
        for arg in args {
            match arg {
                LqPredicateArg::Filter { name, value } if name == "path" => {
                    matchers.push(RepoHasFileMatcher::Path(value.clone()));
                }
                LqPredicateArg::Filter { name, value } if name == "name" => {
                    matchers.push(RepoHasFileMatcher::Name(value.clone()));
                }
                LqPredicateArg::Keyword(_)
                | LqPredicateArg::Phrase(_)
                | LqPredicateArg::RawString(_)
                | LqPredicateArg::Number(_)
                | LqPredicateArg::Filter { .. } => {
                    return Err(CoreError::Typed {
                        code: "LEX_PREDICATE_UNIMPLEMENTED".to_string(),
                        message: format!(
                            "lexical: predicate leaf `{name}` only supports path:/name: filter arguments (owner: LXE-03-predicate-extensions)"
                        ),
                    });
                }
            }
        }
        if matchers.is_empty() {
            return Err(CoreError::Typed {
                code: "LEX_PREDICATE_UNIMPLEMENTED".to_string(),
                message: format!(
                    "lexical: predicate leaf `{name}` requires at least one path:/name: argument (owner: LXE-03-predicate-extensions)"
                ),
            });
        }
        Ok(RepoHasFileConstraint { matchers })
    }

    fn extract_predicate_plan(
        &self,
        expr: &LqExpr,
    ) -> Result<(LqExpr, Vec<RepoHasFileConstraint>, Vec<LqLeaf>), CoreError> {
        match expr {
            LqExpr::Empty => Ok((LqExpr::Empty, Vec::new(), Vec::new())),
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => match name.as_str() {
                "repo.has.file" => Ok((
                    LqExpr::Empty,
                    vec![self.repo_has_file_constraint(name, args)?],
                    Vec::new(),
                )),
                "file.contains" | "file.has.content" => Ok((
                    LqExpr::Empty,
                    Vec::new(),
                    vec![self.predicate_content_leaf(name, args)?],
                )),
                _ => Err(CoreError::Typed {
                    code: "LEX_PREDICATE_UNIMPLEMENTED".to_string(),
                    message: format!(
                        "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: LXE-03-predicate-extensions)"
                    ),
                }),
            },
            LqExpr::Leaf(_) | LqExpr::SemanticVector { .. } => {
                Ok((expr.clone(), Vec::new(), Vec::new()))
            }
            LqExpr::All(parts) => {
                let mut exprs: Vec<LqExpr> = Vec::new();
                let mut repo_predicates: Vec<RepoHasFileConstraint> = Vec::new();
                let mut file_predicates: Vec<LqLeaf> = Vec::new();
                for part in parts {
                    let (lowered, repo_parts, file_parts) = self.extract_predicate_plan(part)?;
                    if !matches!(lowered, LqExpr::Empty) {
                        exprs.push(lowered);
                    }
                    repo_predicates.extend(repo_parts);
                    file_predicates.extend(file_parts);
                }
                Ok((
                    collapse_exprs(exprs, true),
                    repo_predicates,
                    file_predicates,
                ))
            }
            LqExpr::Any(_) | LqExpr::Not(_) => {
                if Self::expr_contains_predicate(expr) {
                    return Err(CoreError::Typed {
                        code: "LEX_PREDICATE_UNIMPLEMENTED".to_string(),
                        message:
                            "lexical: predicate leaves under OR/NOT are not executable on Tantivy adapter (owner: LXE-03-predicate-extensions)"
                                .to_string(),
                    });
                }
                Ok((expr.clone(), Vec::new(), Vec::new()))
            }
        }
    }

    fn prepare_predicate_plan(&self, query: &LqQuery) -> Result<PreparedPredicatePlan, CoreError> {
        let (expr, repo_constraints, file_predicates) = self.extract_predicate_plan(&query.expr)?;
        for constraint in &repo_constraints {
            if !self.repo_has_file_matches(constraint)? {
                return Ok(PreparedPredicatePlan {
                    expr: LqExpr::Empty,
                    allowed_paths: None,
                    force_empty: true,
                });
            }
        }
        let mut allowed_paths: Option<BTreeSet<String>> = None;
        for predicate_leaf in &file_predicates {
            let paths = self.collect_matching_paths_for_leaf(predicate_leaf)?;
            if paths.is_empty() {
                return Ok(PreparedPredicatePlan {
                    expr: LqExpr::Empty,
                    allowed_paths: None,
                    force_empty: true,
                });
            }
            allowed_paths = Some(match allowed_paths.take() {
                Some(existing) => existing.intersection(&paths).cloned().collect(),
                None => paths,
            });
        }
        if allowed_paths.as_ref().is_some_and(BTreeSet::is_empty) {
            return Ok(PreparedPredicatePlan {
                expr: LqExpr::Empty,
                allowed_paths: None,
                force_empty: true,
            });
        }
        Ok(PreparedPredicatePlan {
            expr,
            allowed_paths,
            force_empty: false,
        })
    }

    fn repo_filter_matches(&self, filter: &LqFilter) -> Result<Option<bool>, CoreError> {
        match filter {
            LqFilter::Fork { mode } => {
                if *mode == LqYesNoOnly::Yes {
                    return Ok(Some(true));
                }
                let Some(metadata) = self.repo_metadata.as_ref() else {
                    return Err(Self::missing_repo_metadata_error("fork"));
                };
                let matched = match mode {
                    LqYesNoOnly::Yes => true,
                    LqYesNoOnly::No => !metadata.fork,
                    LqYesNoOnly::Only => metadata.fork,
                };
                Ok(Some(matched))
            }
            LqFilter::Archived { mode } => {
                if *mode == LqYesNoOnly::Yes {
                    return Ok(Some(true));
                }
                let Some(metadata) = self.repo_metadata.as_ref() else {
                    return Err(Self::missing_repo_metadata_error("archived"));
                };
                let matched = match mode {
                    LqYesNoOnly::Yes => true,
                    LqYesNoOnly::No => !metadata.archived,
                    LqYesNoOnly::Only => metadata.archived,
                };
                Ok(Some(matched))
            }
            LqFilter::Visibility { mode } => {
                if *mode == LqVisibility::Any {
                    return Ok(Some(true));
                }
                let Some(metadata) = self.repo_metadata.as_ref() else {
                    return Err(Self::missing_repo_metadata_error("visibility"));
                };
                let matched = match mode {
                    LqVisibility::Any => true,
                    LqVisibility::Public => metadata.visibility == LqVisibility::Public,
                    LqVisibility::Private => metadata.visibility == LqVisibility::Private,
                };
                Ok(Some(matched))
            }
            LqFilter::Context { name } => {
                let Some(metadata) = self.repo_metadata.as_ref() else {
                    return Err(Self::missing_repo_metadata_error("context"));
                };
                let matched = metadata.contexts.iter().any(|context| context == name);
                Ok(Some(matched))
            }
            LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Content { .. } => Ok(None),
        }
    }

    fn repo_filters_allow(&self, query: &LqQuery) -> Result<bool, CoreError> {
        for filter in &query.filters {
            if let Some(matched) = self.repo_filter_matches(filter)?
                && !matched
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn doc_kind_for_type(kind: LqType) -> Result<QueryDocKind, CoreError> {
        match kind {
            LqType::File | LqType::Path => Ok(QueryDocKind::Text),
            LqType::Symbol => Ok(QueryDocKind::Symbol),
            // The planner pre-flight surfaces this as typed
            // `HISTORY_PRODUCER_UNAVAILABLE` before `search()` reaches the
            // doc-kind routing path. The defensive arm here preserves the
            // same typed code for callers that bypass the planner (today
            // there are none on the live rail).
            LqType::Commit | LqType::Diff | LqType::Repo => Err(CoreError::Typed {
                code: crate::filters::codes::HISTORY_PRODUCER_UNAVAILABLE.to_string(),
                message: format!(
                    "lexical: type filter `{}` targets a surface with no producer on the lexical rail",
                    kind.as_str()
                ),
            }),
        }
    }

    fn doc_kind_for_select(dim: LqSelect) -> Result<QueryDocKind, CoreError> {
        match dim {
            LqSelect::File | LqSelect::Path | LqSelect::Content | LqSelect::ContentMatch => {
                Ok(QueryDocKind::Text)
            }
            LqSelect::Symbol => Ok(QueryDocKind::Symbol),
            // Repo surface routes through the same producer-unavailable code
            // as `type:repo`; planner pre-flight catches this for callers
            // that supply the conflicting surface via the surface resolver.
            LqSelect::Repo => Err(CoreError::Typed {
                code: crate::filters::codes::HISTORY_PRODUCER_UNAVAILABLE.to_string(),
                message:
                    "lexical: select filter `repo` targets a surface with no producer on the lexical rail"
                        .to_string(),
            }),
        }
    }

    fn merge_doc_kind(
        current: Option<QueryDocKind>,
        next: QueryDocKind,
        source: &str,
    ) -> Result<Option<QueryDocKind>, CoreError> {
        match current {
            Some(existing) if existing != next => Err(CoreError::InvalidContract(format!(
                "lexical: incompatible doc domain constraint from `{source}`"
            ))),
            Some(existing) => Ok(Some(existing)),
            None => Ok(Some(next)),
        }
    }

    fn prepare_query_for_doc_kind(
        &self,
        query: &LqQuery,
        default_doc_kind: QueryDocKind,
    ) -> Result<(LqQuery, QueryDocKind), CoreError> {
        let mut doc_kind: Option<QueryDocKind> = None;
        let mut filters: Vec<LqFilter> = Vec::with_capacity(query.filters.len());
        for filter in &query.filters {
            match filter {
                LqFilter::Type { kind } => {
                    let next = Self::doc_kind_for_type(*kind)?;
                    doc_kind = Self::merge_doc_kind(doc_kind, next, "type")?;
                }
                LqFilter::Select { dim } => {
                    let next = Self::doc_kind_for_select(*dim)?;
                    doc_kind = Self::merge_doc_kind(doc_kind, next, "select")?;
                }
                LqFilter::Rev { .. } => {
                    // Planner pre-flight surfaces this as
                    // `LEX_FILTER_REV_UNAVAILABLE` before reaching here on
                    // the live `search` path; this arm preserves the same
                    // typed code as a defense-in-depth for any future caller
                    // that bypasses the planner.
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::REV_UNAVAILABLE.to_string(),
                        message: "lexical: rev filter requires history producer".to_string(),
                    });
                }
                other @ (LqFilter::Repo { .. }
                | LqFilter::File { .. }
                | LqFilter::Lang { .. }
                | LqFilter::Content { .. }
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. }) => filters.push(other.clone()),
            }
        }
        let mut prepared = query.clone();
        prepared.filters = filters;
        Ok((prepared, doc_kind.unwrap_or(default_doc_kind)))
    }

    fn compile_expr(
        &self,
        expr: &LqExpr,
        pattern_type: LqPatternType,
    ) -> Result<Box<dyn Query>, CoreError> {
        match expr {
            LqExpr::Empty => Ok(Box::new(AllQuery)),
            LqExpr::Leaf(leaf) => self.compile_leaf(leaf, pattern_type),
            LqExpr::All(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((Occur::Must, self.compile_expr(part, pattern_type)?));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Any(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((Occur::Should, self.compile_expr(part, pattern_type)?));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Not(inner) => {
                let inner_q = self.compile_expr(inner, pattern_type)?;
                let clauses: Vec<(Occur, Box<dyn Query>)> =
                    vec![(Occur::Must, Box::new(AllQuery)), (Occur::MustNot, inner_q)];
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            // Semantic-vector leaves are owned by the semantic engine, not
            // the lexical adapter. Surface as typed so the dispatcher can
            // route the failure deterministically rather than treating it as
            // a transient `NotImplemented`.
            LqExpr::SemanticVector { .. } => Err(CoreError::Typed {
                code: "LEX_LEAF_SEMANTIC_VECTOR_NOT_OWNED".to_string(),
                message:
                    "lexical: SemanticVector is owned by the semantic engine; lexical adapter must not be asked to execute it"
                        .to_string(),
            }),
        }
    }

    /// Compile a content-side regex leaf via the LXE-04 planner pipeline.
    ///
    /// Pipeline:
    /// 1. plan the regex with `crate::regex::plan_regex` (dialect filter +
    ///    literal extraction). Typed regex failures (lookbehind, possessive,
    ///    pattern budget) surface as `CoreError::Typed { code: "LEX_REGEX_*",
    ///    .. }` with a stable code per dialect-rejection kind.
    /// 2. if the indexed corpus exceeds
    ///    [`REGEX_TRIGRAM_INDEX_MISSING_THRESHOLD`] documents, surface
    ///    `LEX_REGEX_TRIGRAM_INDEX_MISSING` — we honestly admit the trigram
    ///    posting index is not yet wired here rather than silently
    ///    full-scanning a large corpus.
    /// 3. otherwise, fall back to the Tantivy `RegexQuery` over the
    ///    `chunk_text` field. This is the verify-only / full-scan path
    ///    relative to the eventual trigram-prefilter pipeline; it is
    ///    deterministic and correct, just unindexed.
    ///
    /// Vendor tokens (`tantivy::*`, `regex::*`) are kept inside this method;
    /// callers see only typed `CoreError`s and `Box<dyn Query>`.
    fn compile_regex_content_leaf(&self, source: &str) -> Result<Box<dyn Query>, CoreError> {
        let _plan = crate::regex::plan_regex(
            source,
            &quanta_index_contract::LqOptions::defaults(),
            &crate::regex::RegexPolicy::defaults(),
        )
        .map_err(map_regex_plan_error)?;
        let searcher = self.reader.searcher();
        if searcher.num_docs() > REGEX_TRIGRAM_INDEX_MISSING_THRESHOLD {
            return Err(CoreError::Typed {
                code: "LEX_REGEX_TRIGRAM_INDEX_MISSING".to_string(),
                message:
                    "lexical: regex execution would require a trigram prefilter index that is not wired on the current adapter; refusing to full-scan a corpus over the threshold"
                        .to_string(),
            });
        }
        self.regex_text_query(self.fields.chunk_text, source)
    }

    fn compile_leaf(
        &self,
        leaf: &LqLeaf,
        pattern_type: LqPatternType,
    ) -> Result<Box<dyn Query>, CoreError> {
        let parser = self.query_parser();
        let query_text = match leaf {
            LqLeaf::Keyword(text) | LqLeaf::RawString(text) => {
                if pattern_type == LqPatternType::Regexp {
                    return self.regex_text_query(self.fields.chunk_text, text);
                }
                text.clone()
            }
            LqLeaf::Phrase(text) => format!("\"{text}\""),
            LqLeaf::Regex(text) => {
                return self.compile_regex_content_leaf(text);
            }
            LqLeaf::StructuralBlock(_) => {
                return Err(CoreError::Typed {
                    code: "STR_PRODUCER_PARSE_TREE_UNAVAILABLE".to_string(),
                    message:
                        "lexical: structural leaf cannot compile without producer parse-tree ops"
                            .to_string(),
                });
            }
            LqLeaf::Predicate { name, .. } => {
                return Err(CoreError::Typed {
                    code: "LEX_PREDICATE_UNIMPLEMENTED".to_string(),
                    message: format!(
                        "lexical: predicate leaf `{name}` is not yet executable on Tantivy adapter (owner: LXE-03-predicate-extensions)"
                    ),
                });
            }
        };
        parser
            .parse_query(&query_text)
            .map_err(|err| CoreError::InvalidContract(format!("lexical: parse: {err}")))
    }

    fn compile_filter(
        &self,
        filter: &LqFilter,
        pattern_type: LqPatternType,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        match filter {
            LqFilter::Repo { pattern, revs } => {
                if !revs.is_empty() {
                    // Repo `revs:` argument is the history-producer surface;
                    // the planner records `REV_UNAVAILABLE` for top-level
                    // `Rev { .. }` filters, and this defense-in-depth arm
                    // covers the nested case (`Repo { revs: [...] }`).
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::REV_UNAVAILABLE.to_string(),
                        message: "lexical: repo filter revisions require a history producer"
                            .to_string(),
                    });
                }
                Ok(Some(
                    self.regex_text_query(self.fields.repo_id, pattern.as_str())?,
                ))
            }
            LqFilter::File { pattern, scope } => {
                Ok(Some(self.compile_file_filter(pattern.as_str(), *scope)?))
            }
            LqFilter::Content { leaf } => Ok(Some(self.compile_leaf(leaf, pattern_type)?)),
            LqFilter::Lang { id } => {
                let Some(language) = normalize_language(id.as_str()) else {
                    return Err(CoreError::InvalidContract(
                        "lexical: lang filter value cannot be empty".to_string(),
                    ));
                };
                Ok(Some(self.exact_text_query(self.fields.language, &language)))
            }
            LqFilter::Rev { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::REV_UNAVAILABLE.to_string(),
                message: "lexical: rev filter requires history producer".to_string(),
            }),
            // Type/Select are doc-kind routing concerns handled in
            // `prepare_query_for_doc_kind`; they should never reach
            // `compile_filter`. If a future caller bypasses that pipeline,
            // surface `LEX_FILTER_UNROUTED` so the bug is visible rather
            // than emerging as a silent empty result.
            LqFilter::Type { .. } | LqFilter::Select { .. } => Err(CoreError::Typed {
                code: "LEX_FILTER_UNROUTED".to_string(),
                message: format!(
                    "lexical: type/select filters must be routed through doc-kind preparation, got `{filter:?}`"
                ),
            }),
            LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => Ok(None),
        }
    }

    fn compile_query(&self, query: &LqQuery) -> Result<Option<Box<dyn Query>>, CoreError> {
        let prepared = self.prepare_predicate_plan(query)?;
        if prepared.force_empty {
            return Ok(None);
        }
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if !matches!(prepared.expr, LqExpr::Empty) {
            clauses.push((
                Occur::Must,
                self.compile_expr(&prepared.expr, query.options.pattern_type)?,
            ));
        }
        for filter in &query.filters {
            if let Some(compiled_filter) =
                self.compile_filter(filter, query.options.pattern_type)?
            {
                clauses.push((Occur::Must, compiled_filter));
            }
        }
        if let Some(paths) = prepared.allowed_paths.as_ref() {
            clauses.push((Occur::Must, self.path_restriction_query(paths)));
        }
        match clauses.len() {
            0 => Err(CoreError::InvalidContract(
                "lexical: query lowered to zero executable clauses".to_string(),
            )),
            1 => match clauses.into_iter().next() {
                Some((_, only)) => Ok(Some(only)),
                None => Err(CoreError::InvalidContract(
                    "lexical: query lowered to zero executable clauses".to_string(),
                )),
            },
            _ => Ok(Some(Box::new(BooleanQuery::new(clauses)))),
        }
    }

    fn compile_query_for_doc_kind(
        &self,
        query: &LqQuery,
        default_doc_kind: QueryDocKind,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        let (prepared_query, doc_kind) =
            self.prepare_query_for_doc_kind(query, default_doc_kind)?;
        let Some(base) = self.compile_query(&prepared_query)? else {
            return Ok(None);
        };
        Ok(Some(self.with_doc_kind(base, doc_kind.as_str())))
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
        let start_line = stored_u32(doc, self.fields.start_line)?.unwrap_or(0);
        let end_line = stored_u32(doc, self.fields.end_line)?.unwrap_or(0);
        Ok(LexicalCandidate {
            candidate_id,
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            manifest_generation: self.generation,
            repo_relative_path: RepoRelativePath::new(repo_relative_path),
            start_line,
            end_line,
            score,
            snippet,
        })
    }
}

fn stored_text(doc: &TantivyDocument, field: Field) -> Option<String> {
    let value: &OwnedValue = doc.get_first(field)?;
    Value::as_str(&value).map(str::to_owned)
}

fn stored_u32(doc: &TantivyDocument, field: Field) -> Result<Option<u32>, CoreError> {
    let Some(value) = doc.get_first(field) else {
        return Ok(None);
    };
    let Some(raw) = Value::as_u64(&value) else {
        return Ok(None);
    };
    u32::try_from(raw)
        .map(Some)
        .map_err(|err| CoreError::Storage(format!("lexical: stored u32 exceeds range: {err}")))
}

/// Filter the planner's typed-unavailable list against adapter state.
///
/// The planner is stateless — it does not know which producers this
/// particular `TantivySearcher` actually has wired. The
/// repo-metadata-dependent codes (FORK/ARCHIVED/VISIBILITY/CONTEXT) drop
/// out of the typed-unavailable surface when the adapter has loaded a
/// `LexicalRepoMetadataRecord` from the bundle payload, because the live
/// `repo_filter_matches` path then handles those filters correctly.
///
/// The `HISTORY_PRODUCER_UNAVAILABLE` and `REV_UNAVAILABLE` codes are
/// never suppressed: no commit/diff/repo producer or history producer is
/// wired on any current configuration of the lexical rail.
fn is_unavailable_suppressed_by_metadata(code: &str, has_repo_metadata: bool) -> bool {
    if !has_repo_metadata {
        return false;
    }
    matches!(
        code,
        crate::filters::codes::FORK_UNAVAILABLE
            | crate::filters::codes::ARCHIVED_UNAVAILABLE
            | crate::filters::codes::VISIBILITY_UNAVAILABLE
            | crate::filters::codes::CONTEXT_UNAVAILABLE
    )
}

/// Run the planner pre-flight and surface its outcome as a typed result.
///
/// Returns `Ok(())` if the plan is executable on the live Tantivy rail.
/// Returns `Err(CoreError::Typed { .. })` for typed-unavailable filters
/// (deterministic first-wins ordering) and for planner errors that lower
/// to typed failures (e.g. `count:0` → `LEX_FILTER_INVALID_COUNT`).
///
/// Ordering rule for `typed_unavailable`: the planner records typed-
/// unavailable filters in the order they appeared in the input
/// `LqQuery::filters`. The executor surfaces the **first** such entry that
/// is not suppressed by adapter-side producer state — this gives
/// operator-facing diagnostics a deterministic single cause rather than a
/// multi-line laundry list.
///
/// Planner [`Unimplemented`](crate::planner::LexicalPlannerError::Unimplemented)
/// shapes (top-level `All`/`Any`/`Not` IR nodes) are NOT a hard rejection
/// today: the legacy executor still handles them. The planner pre-flight
/// therefore swallows `Unimplemented` and `UnsupportedNotScope` /
/// `UnsupportedOrScope` arms; only the typed planner errors surface here.
fn planner_preflight(query: &LqQuery, has_repo_metadata: bool) -> Result<(), CoreError> {
    let plan = match crate::planner::LexicalPlanner::plan(query) {
        Ok(plan) => plan,
        Err(err) => {
            use crate::planner::LexicalPlannerError;
            match err {
                // Filter planning failed up-front (count:0, conflicting
                // surface, …) → typed.
                LexicalPlannerError::FilterPlan(_)
                | LexicalPlannerError::RegexPlan(_)
                | LexicalPlannerError::TrigramPlan(_)
                | LexicalPlannerError::PhrasePlan(_)
                | LexicalPlannerError::SymbolPlan(_) => return Err(map_planner_error(&err)),
                // IR shapes the planner has not lowered yet (boolean
                // composition, predicate extensions). The legacy executor
                // still handles these; the planner's role here is filter
                // pre-flight, not full execution authority.
                LexicalPlannerError::UnsupportedNotScope
                | LexicalPlannerError::UnsupportedOrScope
                | LexicalPlannerError::UnsupportedFilterCombo
                | LexicalPlannerError::Unimplemented { .. } => return Ok(()),
            }
        }
    };
    for entry in &plan.filters.typed_unavailable {
        if is_unavailable_suppressed_by_metadata(entry.code, has_repo_metadata) {
            continue;
        }
        return Err(CoreError::Typed {
            code: entry.code.to_string(),
            message: entry.reason.to_string(),
        });
    }
    Ok(())
}

/// Lower a [`crate::regex::RegexPlannerError`] into a typed [`CoreError`].
///
/// Each variant maps to a stable wire code so the search-plane and producer
/// can attribute regex rejections without parsing free-form text. The
/// `LEX_REGEX_DIALECT_` prefix mirrors the `regex_*` namespace already used
/// elsewhere in the workspace (e.g. `LEX_REGEX_TRIGRAM_INDEX_MISSING`).
fn map_regex_plan_error(err: crate::regex::RegexPlannerError) -> CoreError {
    use crate::regex::RegexPlannerError;
    match err {
        RegexPlannerError::ParseError { source, detail } => CoreError::Typed {
            code: "LEX_REGEX_DIALECT_PARSE_ERROR".to_string(),
            message: format!("lexical: regex parse error for {source:?}: {detail}"),
        },
        RegexPlannerError::UnsupportedFeature { feature } => CoreError::Typed {
            code: "LEX_REGEX_DIALECT_UNSUPPORTED".to_string(),
            message: format!("lexical: regex unsupported feature `{feature}`"),
        },
        RegexPlannerError::UnboundedCandidatePlan {
            estimated_states,
            budget,
        } => CoreError::Typed {
            code: "LEX_REGEX_BUDGET_EXCEEDED".to_string(),
            message: format!(
                "lexical: regex NFA budget exceeded (estimated {estimated_states} states, budget {budget})"
            ),
        },
    }
}

/// Lower a [`crate::planner::LexicalPlannerError`] into a [`CoreError`].
///
/// Planner errors are either typed failures (count:0, conflicting surfaces,
/// regex-dialect rejection) or "unimplemented" shapes the planner has not
/// learned yet. The former lower to [`CoreError::Typed`] so the caller can
/// see the stable code; the latter lower to [`CoreError::InvalidContract`]
/// so the search path never silently runs an unplanned query.
fn map_planner_error(err: &crate::planner::LexicalPlannerError) -> CoreError {
    use crate::planner::LexicalPlannerError;
    match err {
        LexicalPlannerError::FilterPlan(fpe) => match fpe {
            crate::filters::FilterPlannerError::InvalidCount { detail } => CoreError::Typed {
                code: "LEX_FILTER_INVALID_COUNT".to_string(),
                message: (*detail).to_string(),
            },
            crate::filters::FilterPlannerError::ConflictingResultSurface { detail } => {
                CoreError::Typed {
                    code: "LEX_FILTER_CONFLICTING_SURFACE".to_string(),
                    message: (*detail).to_string(),
                }
            }
            crate::filters::FilterPlannerError::UnsupportedFilterCombo { detail } => {
                CoreError::Typed {
                    code: "LEX_FILTER_UNSUPPORTED_COMBO".to_string(),
                    message: (*detail).to_string(),
                }
            }
        },
        // The remaining planner errors (`Unimplemented`,
        // `UnsupportedNotScope`, `UnsupportedOrScope`,
        // `UnsupportedFilterCombo`, `RegexPlan`, `TrigramPlan`,
        // `PhrasePlan`, `SymbolPlan`) are typed-but-not-filter shapes —
        // the planner pre-flight swallows IR-shape `Unimplemented` for
        // backwards compatibility with the legacy executor, and the
        // leaf planners surface their own typed errors via their
        // dedicated callers (e.g. `compile_regex_content_leaf`). Anything
        // that reaches here is therefore a true contract violation.
        LexicalPlannerError::UnsupportedNotScope
        | LexicalPlannerError::UnsupportedOrScope
        | LexicalPlannerError::UnsupportedFilterCombo
        | LexicalPlannerError::Unimplemented { .. }
        | LexicalPlannerError::RegexPlan(_)
        | LexicalPlannerError::TrigramPlan(_)
        | LexicalPlannerError::PhrasePlan(_)
        | LexicalPlannerError::SymbolPlan(_) => {
            CoreError::InvalidContract(format!("lexical: planner: {err}"))
        }
    }
}

impl LexicalSearcher for TantivySearcher {
    fn search(&self, query: &LqQuery, top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError> {
        // LXE-02/LXE-03 planner authority: the planner is the source of truth
        // for typed-unavailable filter surfacing. The first typed-unavailable
        // entry wins — search never runs with a silently-dropped filter.
        // The planner pre-flight runs BEFORE `LexicalPolicy::validate_query`
        // so its typed codes (HISTORY_PRODUCER_UNAVAILABLE, REV_UNAVAILABLE,
        // …) win over the core policy's `NotImplemented` returns. The
        // policy still gates structural / empty-query / predicate-
        // composition cases the planner does not yet cover.
        planner_preflight(query, self.repo_metadata.is_some())?;
        LexicalPolicy::validate_query(query)?;
        if !self.repo_filters_allow(query)? {
            return Ok(Vec::new());
        }
        let Some(compiled) = self.compile_query_for_doc_kind(query, QueryDocKind::Text)? else {
            return Ok(Vec::new());
        };
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

    fn search_symbols(
        &self,
        query: &LqQuery,
        top_k: u32,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        planner_preflight(query, self.repo_metadata.is_some())?;
        LexicalPolicy::validate_query(query)?;
        if !self.repo_filters_allow(query)? {
            return Ok(Vec::new());
        }
        let Some(compiled) = self.compile_query_for_doc_kind(query, QueryDocKind::Symbol)? else {
            return Ok(Vec::new());
        };
        let limit = usize::try_from(top_k)
            .map_err(|err| CoreError::InvalidContract(format!("lexical: top_k: {err}")))?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(limit))
            .map_err(|err| CoreError::Storage(format!("lexical: symbol search: {err}")))?;
        let mut out: Vec<LexicalCandidate> = Vec::with_capacity(hits.len());
        for (score, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_candidate(&doc, score)?);
        }
        Ok(out)
    }

    fn search_all(&self, query: &LqQuery) -> Result<Vec<LexicalCandidate>, CoreError> {
        planner_preflight(query, self.repo_metadata.is_some())?;
        LexicalPolicy::validate_query(query)?;
        if !self.repo_filters_allow(query)? {
            return Ok(Vec::new());
        }
        let Some(compiled) = self.compile_query_for_doc_kind(query, QueryDocKind::Text)? else {
            return Ok(Vec::new());
        };
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while materializing scope: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(limit))
            .map_err(|err| CoreError::Storage(format!("lexical: search_all: {err}")))?;
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
