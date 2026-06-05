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
mod predicate_registry;
pub mod regex;
pub mod symbol;
pub mod trigram_plan;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ciborium::Value as CborValue;
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{
    LexicalErrorCode, SymbolKindCode, SymbolKindFamily, SymbolRecord,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkRecord, FileContributorIngestBatch, FileOwnerProjectionRow,
    FileOwnershipIngestBatch, LexicalCandidate, LexicalFullBundle, LexicalIngestBatch, LexicalSeal,
    LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions, LqPatternType, LqPredicateArg, LqQuery,
    LqSelect, LqType, LqVisibility, LqYesNoOnly, ManifestGeneration, ReplaceLexicalScope,
    RepoCommitRecencyIngestBatch, RepoDescriptionIngestBatch, RepoId, RepoMetaIngestBatch,
    RepoRelativePath, RepoTopicIngestBatch, RevisionId, SymbolCandidate, TombstoneLexicalScope,
};
use quanta_index_core::{
    CoreError, FileContributorIngestPort, FileOwnershipIngestPort, LexicalBatchBuildPort,
    LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalSearcher, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMetaIngestPort, RepoTopicIngestPort,
    domains::lexical::LexicalPolicy,
    timeref::{is_rev_at_time_spec, parse_search_timeref_ms},
};
use quanta_index_lq_positions::{
    DocId as PositionsDocId, NormalizerVersion, Position, PositionsBuilder, PositionsError,
    PositionsErrorCode, PositionsIndex, query_phrase,
};
use quanta_index_lq_regex::RegexExecutor;
use quanta_index_lq_trigram::{
    DocId as TrigramDocId, DocResolver, TrigramError, TrigramErrorCode, TrigramIndex,
    TrigramIndexBuilder, query_raw_substring, regex_prefilter,
};

use crate::phrase::{PhraseField, PhrasePolicy, plan_phrase, tokenize_phrase_terms};
use crate::predicate_registry::{
    ContentPathScope, ContentPredicateArgError, ContentPredicateConstraint, ContentScalarArg,
    ContentScalarArgError, FileContributorArg, FileContributorArgError, FileOwnerArg,
    FileOwnerArgError, PREDICATE_OWNER, PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED_CODE, PredicateKind,
    RepoDescriptionArg, RepoDescriptionArgError, RepoFileArgError, RepoFileConstraint,
    RepoFileMatcher, RepoMetaArg, RepoMetaArgError, RepoTopicArg, RepoTopicArgError,
    TimerefScalarArgError, canonicalize_predicate_call, kind_of,
    parse_content_predicate_constraint, parse_content_scalar_arg, parse_file_contributor_arg,
    parse_file_owner_arg, parse_repo_description_arg, parse_repo_file_matchers,
    parse_repo_meta_arg, parse_repo_topic_arg, parse_timeref_scalar_arg, unimplemented_predicate,
};
use crate::regex::RegexPolicy;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser, RegexQuery, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, OwnedValue, STORED, STRING, Schema, TEXT, TantivyDocument,
    TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{RemoveLongFilter, SimpleTokenizer, TextAnalyzer};
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Term};

/// Memory budget for a Tantivy `IndexWriter`. Pinned to the upstream-documented
/// minimum so adapter setup is bounded and reproducible across test runs.
const WRITER_MEMORY_BUDGET_BYTES: usize = 15_000_000;
const TEXT_DOC_KIND: &str = "text";
const SYMBOL_DOC_KIND: &str = "symbol";
const REPO_METADATA_FILE_NAME: &str = "repo-metadata.cbor";
const REPO_COMMIT_RECENCY_FILE_NAME: &str = "repo-commit-recency.cbor";
const REPO_META_FILE_NAME: &str = "repo-meta.cbor";
const REPO_TOPIC_FILE_NAME: &str = "repo-topic.cbor";
const REPO_DESCRIPTION_FILE_NAME: &str = "repo-description.cbor";
const FILE_OWNERSHIP_FILE_NAME: &str = "file-ownership.cbor";
const FILE_CONTRIBUTOR_FILE_NAME: &str = "file-contributor.cbor";
const CASE_SENSITIVE_TOKENIZER_NAME: &str = "qi_case_sensitive";
const TEXT_AUTHORITY_DOC_TABLE_FILE_NAME: &str = "text-authority-docs.cbor";
const TEXT_AUTHORITY_TRIGRAM_FILE_NAME: &str = "text-authority-trigram.cbor";
const TEXT_AUTHORITY_TRIGRAM_FOLDED_FILE_NAME: &str = "text-authority-trigram-folded.cbor";
const TEXT_AUTHORITY_POSITIONS_FILE_NAME: &str = "text-authority-positions.cbor";
const TEXT_AUTHORITY_POSITIONS_FOLDED_FILE_NAME: &str = "text-authority-positions-folded.cbor";
const POSITIONS_NORMALIZER_VERSION: NormalizerVersion = NormalizerVersion::new(1, 0);

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
    repo_relative_path_query: Field,
    repo_relative_path_case: Field,
    file_name: Field,
    language: Field,
    start_line: Field,
    end_line: Field,
    snippet: Field,
    chunk_text: Field,
    chunk_text_case: Field,
    symbol_kind: Field,
    symbol_kind_family: Field,
}

impl SchemaFields {
    fn build() -> Self {
        let mut builder = Schema::builder();
        let candidate_id = builder.add_text_field("candidate_id", STRING | STORED);
        let repo_id = builder.add_text_field("repo_id", STRING | STORED);
        let revision_id = builder.add_text_field("revision_id", STRING | STORED);
        let doc_kind = builder.add_text_field("doc_kind", STRING | STORED);
        let repo_relative_path = builder.add_text_field("repo_relative_path", STRING | STORED);
        let repo_relative_path_query = builder.add_text_field("repo_relative_path_query", TEXT);
        let repo_relative_path_case =
            builder.add_text_field("repo_relative_path_case", case_sensitive_text_options());
        let file_name = builder.add_text_field("file_name", STRING);
        let language = builder.add_text_field("language", STRING);
        let start_line = builder.add_u64_field("start_line", STORED);
        let end_line = builder.add_u64_field("end_line", STORED);
        let snippet = builder.add_text_field("snippet", STORED);
        let chunk_text = builder.add_text_field("chunk_text", TEXT | STORED);
        let chunk_text_case =
            builder.add_text_field("chunk_text_case", case_sensitive_text_options());
        let symbol_kind = builder.add_text_field("symbol_kind", STRING | STORED);
        let symbol_kind_family = builder.add_text_field("symbol_kind_family", STRING | STORED);
        let schema = builder.build();
        Self {
            schema,
            candidate_id,
            repo_id,
            revision_id,
            doc_kind,
            repo_relative_path,
            repo_relative_path_query,
            repo_relative_path_case,
            file_name,
            language,
            start_line,
            end_line,
            snippet,
            chunk_text,
            chunk_text_case,
            symbol_kind,
            symbol_kind_family,
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

/// Local decode/encode for the legacy repo-metadata sidecar.
///
/// Producer-facing lexical publish has moved away from a public root contract
/// type; the adapter still consumes and snapshots the residual CBOR map so
/// repo-level filters remain wired for older bundle flows.
#[derive(Clone, Debug, Eq, PartialEq)]
struct LexicalRepoMetadataPayload {
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: Vec<String>,
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

fn encode_cbor<T>(value: &T, label: &str) -> Result<Vec<u8>, CoreError>
where
    T: serde::Serialize,
{
    let mut payload = Vec::new();
    ciborium::into_writer(value, &mut payload)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: encode {label}: {err}")))?;
    Ok(payload)
}

fn decode_replace_scope_payload(
    bytes: &[u8],
) -> Result<
    (
        BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::LexicalReplaceScope,
    ),
    CoreError,
> {
    ciborium::from_reader::<
        (
            BatchIngestMode,
            Option<ManifestGeneration>,
            quanta_index_contract::LexicalReplaceScope,
        ),
        _,
    >(bytes)
    .map_err(|err| {
        CoreError::InvalidContract(format!("lexical: replace scope payload decode: {err}"))
    })
}

fn decode_tombstone_scope_payload(
    bytes: &[u8],
) -> Result<
    (
        BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::LexicalTombstoneScope,
    ),
    CoreError,
> {
    ciborium::from_reader::<
        (
            BatchIngestMode,
            Option<ManifestGeneration>,
            quanta_index_contract::LexicalTombstoneScope,
        ),
        _,
    >(bytes)
    .map_err(|err| {
        CoreError::InvalidContract(format!("lexical: tombstone scope payload decode: {err}"))
    })
}

/// Legacy `FullBundle` placeholder payload.
///
/// Older producers used the literal `b"manifest"` as an opaque marker.
/// Later producers emit a CBOR map with repo metadata. The two are
/// distinguished here explicitly so that real decode failures surface as
/// `InvalidContract` rather than silently routing to "no metadata".
const LEGACY_FULL_BUNDLE_PAYLOAD: &[u8] = b"manifest";

fn encode_repo_metadata_visibility(visibility: &LqVisibility) -> Result<CborValue, CoreError> {
    let mut payload = Vec::new();
    ciborium::into_writer(visibility, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo metadata encode visibility: {err}"))
    })?;
    ciborium::from_reader::<CborValue, _>(payload.as_slice()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata visibility wire decode: {err}"
        ))
    })
}

fn decode_repo_metadata_bool(field_name: &str, value: &CborValue) -> Result<bool, CoreError> {
    let mut payload = Vec::new();
    ciborium::into_writer(&value, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` re-encode: {err}"
        ))
    })?;
    ciborium::from_reader::<bool, _>(payload.as_slice()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` decode: {err}"
        ))
    })
}

fn decode_repo_metadata_visibility(
    field_name: &str,
    value: &CborValue,
) -> Result<LqVisibility, CoreError> {
    let mut payload = Vec::new();
    ciborium::into_writer(&value, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` re-encode: {err}"
        ))
    })?;
    ciborium::from_reader::<LqVisibility, _>(payload.as_slice()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` decode: {err}"
        ))
    })
}

fn decode_repo_metadata_contexts(
    field_name: &str,
    value: &CborValue,
) -> Result<Vec<String>, CoreError> {
    let mut payload = Vec::new();
    ciborium::into_writer(&value, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` re-encode: {err}"
        ))
    })?;
    let contexts = ciborium::from_reader::<Vec<String>, _>(payload.as_slice()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: repo metadata field `{field_name}` decode: {err}"
        ))
    })?;
    if contexts.iter().any(String::is_empty) {
        return Err(CoreError::InvalidContract(
            "lexical: repo metadata field `contexts` must not contain empty names".to_string(),
        ));
    }
    Ok(contexts)
}

fn encode_repo_metadata_payload(
    metadata: &LexicalRepoMetadataPayload,
) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let wire = CborValue::Map(vec![
        (
            CborValue::Text("fork".to_string()),
            CborValue::Bool(metadata.fork),
        ),
        (
            CborValue::Text("archived".to_string()),
            CborValue::Bool(metadata.archived),
        ),
        (
            CborValue::Text("visibility".to_string()),
            encode_repo_metadata_visibility(&metadata.visibility)?,
        ),
        (
            CborValue::Text("contexts".to_string()),
            CborValue::Array(
                metadata
                    .contexts
                    .iter()
                    .cloned()
                    .map(CborValue::Text)
                    .collect(),
            ),
        ),
    ]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo metadata encode: {err}"))
    })?;
    Ok(payload)
}

fn decode_repo_metadata_payload(
    bytes: &[u8],
) -> Result<Option<LexicalRepoMetadataPayload>, CoreError> {
    if bytes.is_empty() || bytes == LEGACY_FULL_BUNDLE_PAYLOAD {
        return Ok(None);
    }
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo metadata payload decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo metadata payload decode: expected map".to_string(),
        ));
    };
    let mut fork: Option<bool> = None;
    let mut archived: Option<bool> = None;
    let mut visibility: Option<LqVisibility> = None;
    let mut contexts: Option<Vec<String>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo metadata payload decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "fork" => {
                if fork.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo metadata payload decode: duplicate field `fork`".to_string(),
                    ));
                }
                fork = Some(decode_repo_metadata_bool("fork", &value)?);
            }
            "archived" => {
                if archived.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo metadata payload decode: duplicate field `archived`"
                            .to_string(),
                    ));
                }
                archived = Some(decode_repo_metadata_bool("archived", &value)?);
            }
            "visibility" => {
                if visibility.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo metadata payload decode: duplicate field `visibility`"
                            .to_string(),
                    ));
                }
                visibility = Some(decode_repo_metadata_visibility("visibility", &value)?);
            }
            "contexts" => {
                if contexts.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo metadata payload decode: duplicate field `contexts`"
                            .to_string(),
                    ));
                }
                contexts = Some(decode_repo_metadata_contexts("contexts", &value)?);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo metadata payload decode: unknown field `{other}`"
                )));
            }
        }
    }
    Ok(Some(LexicalRepoMetadataPayload {
        fork: fork.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo metadata payload decode: missing field `fork`".to_string(),
            )
        })?,
        archived: archived.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo metadata payload decode: missing field `archived`".to_string(),
            )
        })?,
        visibility: visibility.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo metadata payload decode: missing field `visibility`".to_string(),
            )
        })?,
        contexts: contexts.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo metadata payload decode: missing field `contexts`".to_string(),
            )
        })?,
    }))
}

fn persist_repo_metadata_snapshot(
    path: &Path,
    metadata: Option<&LexicalRepoMetadataPayload>,
) -> Result<(), CoreError> {
    match metadata {
        Some(metadata) => {
            let payload = encode_repo_metadata_payload(metadata)?;
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
) -> Result<Option<LexicalRepoMetadataPayload>, CoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let payload = std::fs::read(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: read repo metadata snapshot {}: {err}",
            path.display()
        ))
    })?;
    decode_repo_metadata_payload(payload.as_slice()).map_err(|err| match err {
        CoreError::InvalidContract(message) => CoreError::InvalidContract(format!(
            "lexical: repo metadata decode {}: {message}",
            path.display()
        )),
        other @ (CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => other,
    })
}

fn encode_repo_commit_recency_snapshot(
    shard: &RepoCommitRecencyShard,
) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let entries = shard
        .latest_committer_time_ms_by_repo_id
        .iter()
        .map(|(source_repo_id, latest_committer_time_ms)| {
            CborValue::Map(vec![
                (
                    CborValue::Text("source_repo_id".to_string()),
                    CborValue::Text(source_repo_id.clone()),
                ),
                (
                    CborValue::Text("latest_committer_time_ms".to_string()),
                    CborValue::Text(latest_committer_time_ms.to_string()),
                ),
            ])
        })
        .collect::<Vec<_>>();
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo commit recency encode: {err}"))
    })?;
    Ok(payload)
}

fn decode_repo_commit_recency_snapshot(bytes: &[u8]) -> Result<RepoCommitRecencyShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo commit recency decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo commit recency decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo commit recency decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo commit recency decode: duplicate field `entries`"
                            .to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo commit recency decode: `entries` must be an array"
                            .to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo commit recency decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut latest_committer_time_ms_by_repo_id = BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: repo commit recency decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: repo commit recency decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut latest_committer_time_ms: Option<u64> = None;
        for (key, value) in fields {
            let CborValue::Text(field_name) = key else {
                return Err(CoreError::InvalidContract(
                    "lexical: repo commit recency decode: entry field name must be text"
                        .to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo commit recency decode: `source_repo_id` must be text"
                                .to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "latest_committer_time_ms" => {
                    let CborValue::Text(text) = value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo commit recency decode: `latest_committer_time_ms` must be text"
                                .to_string(),
                        ));
                    };
                    latest_committer_time_ms = Some(text.parse().map_err(|err| {
                        CoreError::InvalidContract(format!(
                            "lexical: repo commit recency decode: `latest_committer_time_ms` must be a u64 decimal string: {err}"
                        ))
                    })?);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: repo commit recency decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo commit recency decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let latest_committer_time_ms = latest_committer_time_ms.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo commit recency decode: missing field `latest_committer_time_ms`"
                    .to_string(),
            )
        })?;
        let _prior =
            latest_committer_time_ms_by_repo_id.insert(source_repo_id, latest_committer_time_ms);
    }
    Ok(RepoCommitRecencyShard {
        latest_committer_time_ms_by_repo_id,
    })
}

fn persist_repo_commit_recency_snapshot(
    path: &Path,
    batch: &RepoCommitRecencyIngestBatch,
) -> Result<(), CoreError> {
    let mut latest_committer_time_ms_by_repo_id = BTreeMap::new();
    for entry in &batch.entries {
        let _prior = latest_committer_time_ms_by_repo_id.insert(
            entry.source_repo_id.as_str().to_string(),
            entry.latest_committer_time_ms,
        );
    }
    let payload = encode_repo_commit_recency_snapshot(&RepoCommitRecencyShard {
        latest_committer_time_ms_by_repo_id,
    })?;
    std::fs::write(path, payload).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: write repo commit recency snapshot {}: {err}",
            path.display()
        ))
    })?;
    Ok(())
}

fn load_repo_commit_recency_snapshot(
    path: &Path,
) -> Result<Option<RepoCommitRecencyShard>, CoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let payload = std::fs::read(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: read repo commit recency snapshot {}: {err}",
            path.display()
        ))
    })?;
    decode_repo_commit_recency_snapshot(payload.as_slice())
        .map(Some)
        .map_err(|err| match err {
            CoreError::InvalidContract(message) => CoreError::InvalidContract(format!(
                "lexical: repo commit recency decode {}: {message}",
                path.display()
            )),
            other @ (CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => other,
        })
}

fn encode_repo_meta_snapshot(shard: &RepoMetaShard) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, by_key) in &shard.meta_by_repo_id {
        for (key, value) in by_key {
            entries.push(CborValue::Map(vec![
                (
                    CborValue::Text("source_repo_id".to_string()),
                    CborValue::Text(source_repo_id.clone()),
                ),
                (
                    CborValue::Text("key".to_string()),
                    CborValue::Text(key.clone()),
                ),
                (
                    CborValue::Text("value".to_string()),
                    CborValue::Text(value.clone()),
                ),
            ]));
        }
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: repo meta encode: {err}")))?;
    Ok(payload)
}

fn decode_repo_meta_snapshot(bytes: &[u8]) -> Result<RepoMetaShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: repo meta decode: {err}")))?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo meta decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo meta decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo meta decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo meta decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo meta decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut meta_by_repo_id: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract("lexical: repo meta decode: missing field `entries`".to_string())
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: repo meta decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut key: Option<String> = None;
        let mut value: Option<String> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: repo meta decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo meta decode: `source_repo_id` must be text".to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "key" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo meta decode: `key` must be text".to_string(),
                        ));
                    };
                    key = Some(text);
                }
                "value" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo meta decode: `value` must be text".to_string(),
                        ));
                    };
                    value = Some(text);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: repo meta decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo meta decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let key = key.ok_or_else(|| {
            CoreError::InvalidContract("lexical: repo meta decode: missing field `key`".to_string())
        })?;
        let value = value.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo meta decode: missing field `value`".to_string(),
            )
        })?;
        let _prior = meta_by_repo_id
            .entry(source_repo_id)
            .or_default()
            .insert(key, value);
    }
    Ok(RepoMetaShard { meta_by_repo_id })
}

fn persist_repo_meta_snapshot(path: &Path, batch: &RepoMetaIngestBatch) -> Result<(), CoreError> {
    let mut meta_by_repo_id: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for entry in &batch.entries {
        let normalized_key = normalize_repo_meta_key(&entry.key).ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo meta ingest entry key must not be empty".to_string(),
            )
        })?;
        let _prior = meta_by_repo_id
            .entry(entry.source_repo_id.as_str().to_string())
            .or_default()
            .insert(normalized_key, entry.value.clone());
    }
    let payload = encode_repo_meta_snapshot(&RepoMetaShard { meta_by_repo_id })?;
    std::fs::write(path, payload).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: write repo meta snapshot {}: {err}",
            path.display()
        ))
    })?;
    Ok(())
}

fn load_repo_meta_snapshot(path: &Path) -> Result<Option<RepoMetaShard>, CoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let payload = std::fs::read(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: read repo meta snapshot {}: {err}",
            path.display()
        ))
    })?;
    decode_repo_meta_snapshot(payload.as_slice())
        .map(Some)
        .map_err(|err| match err {
            CoreError::InvalidContract(message) => CoreError::InvalidContract(format!(
                "lexical: repo meta decode {}: {message}",
                path.display()
            )),
            other @ (CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => other,
        })
}

fn encode_repo_topic_snapshot(shard: &RepoTopicShard) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, topics) in &shard.topics_by_repo_id {
        entries.push(CborValue::Map(vec![
            (
                CborValue::Text("source_repo_id".to_string()),
                CborValue::Text(source_repo_id.clone()),
            ),
            (
                CborValue::Text("topics".to_string()),
                CborValue::Array(
                    topics
                        .iter()
                        .cloned()
                        .map(CborValue::Text)
                        .collect::<Vec<_>>(),
                ),
            ),
        ]));
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: repo topic encode: {err}")))?;
    Ok(payload)
}

fn decode_repo_topic_snapshot(bytes: &[u8]) -> Result<RepoTopicShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: repo topic decode: {err}")))?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo topic decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo topic decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo topic decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo topic decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo topic decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut topics_by_repo_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: repo topic decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: repo topic decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut topics: Option<BTreeSet<String>> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: repo topic decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo topic decode: `source_repo_id` must be text".to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "topics" => {
                    let CborValue::Array(items) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo topic decode: `topics` must be an array".to_string(),
                        ));
                    };
                    let mut normalized: BTreeSet<String> = BTreeSet::new();
                    for item in items {
                        let CborValue::Text(text) = item else {
                            return Err(CoreError::InvalidContract(
                                "lexical: repo topic decode: topic must be text".to_string(),
                            ));
                        };
                        let topic = normalize_repo_topic_value(&text).ok_or_else(|| {
                            CoreError::InvalidContract(
                                "lexical: repo topic decode: topic must be non-empty".to_string(),
                            )
                        })?;
                        let _inserted = normalized.insert(topic);
                    }
                    topics = Some(normalized);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: repo topic decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo topic decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let topics = topics.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo topic decode: missing field `topics`".to_string(),
            )
        })?;
        let _prior = topics_by_repo_id.insert(source_repo_id, topics);
    }
    Ok(RepoTopicShard { topics_by_repo_id })
}

fn persist_repo_topic_snapshot(path: &Path, batch: &RepoTopicIngestBatch) -> Result<(), CoreError> {
    let mut topics_by_repo_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in &batch.entries {
        let topic = normalize_repo_topic_value(&entry.topic).ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo topic ingest entry topic must not be empty".to_string(),
            )
        })?;
        let _inserted = topics_by_repo_id
            .entry(entry.source_repo_id.as_str().to_string())
            .or_default()
            .insert(topic);
    }
    let payload = encode_repo_topic_snapshot(&RepoTopicShard { topics_by_repo_id })?;
    std::fs::write(path, payload).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: write repo topic snapshot {}: {err}",
            path.display()
        ))
    })?;
    Ok(())
}

fn load_repo_topic_snapshot(path: &Path) -> Result<Option<RepoTopicShard>, CoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let payload = std::fs::read(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: read repo topic snapshot {}: {err}",
            path.display()
        ))
    })?;
    decode_repo_topic_snapshot(payload.as_slice())
        .map(Some)
        .map_err(|err| match err {
            CoreError::InvalidContract(message) => CoreError::InvalidContract(format!(
                "lexical: repo topic decode {}: {message}",
                path.display()
            )),
            other @ (CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => other,
        })
}

/// Validate a producer-published repo description. Unlike topics, the
/// description is stored verbatim (case and internal whitespace preserved) so
/// regex matching at query time is faithful; only a non-empty constraint is
/// enforced. Returns the original string when it carries a non-whitespace
/// character, `None` otherwise.
fn validate_repo_description_value(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn encode_repo_description_snapshot(shard: &RepoDescriptionShard) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, description) in &shard.descriptions_by_repo_id {
        entries.push(CborValue::Map(vec![
            (
                CborValue::Text("source_repo_id".to_string()),
                CborValue::Text(source_repo_id.clone()),
            ),
            (
                CborValue::Text("description".to_string()),
                CborValue::Text(description.clone()),
            ),
        ]));
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo description encode: {err}"))
    })?;
    Ok(payload)
}

fn decode_repo_description_snapshot(bytes: &[u8]) -> Result<RepoDescriptionShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: repo description decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: repo description decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: repo description decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo description decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: repo description decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: repo description decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut descriptions_by_repo_id: BTreeMap<String, String> = BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: repo description decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: repo description decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut description: Option<String> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: repo description decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo description decode: `source_repo_id` must be text"
                                .to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "description" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo description decode: `description` must be text"
                                .to_string(),
                        ));
                    };
                    let value = validate_repo_description_value(&text).ok_or_else(|| {
                        CoreError::InvalidContract(
                            "lexical: repo description decode: description must be non-empty"
                                .to_string(),
                        )
                    })?;
                    description = Some(value);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: repo description decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo description decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let description = description.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo description decode: missing field `description`".to_string(),
            )
        })?;
        // A well-formed snapshot (written from a BTreeMap) has one entry per
        // repo; a duplicate means a corrupted or foreign file. Fail closed.
        if let Some(prior) = descriptions_by_repo_id.insert(source_repo_id.clone(), description) {
            return Err(CoreError::InvalidContract(format!(
                "lexical: repo description decode: duplicate entry for source_repo_id `{source_repo_id}` (prior `{prior}`)"
            )));
        }
    }
    Ok(RepoDescriptionShard {
        descriptions_by_repo_id,
    })
}

fn persist_repo_description_snapshot(
    path: &Path,
    batch: &RepoDescriptionIngestBatch,
) -> Result<(), CoreError> {
    let mut descriptions_by_repo_id: BTreeMap<String, String> = BTreeMap::new();
    for entry in &batch.entries {
        let description = validate_repo_description_value(&entry.description).ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: repo description ingest entry description must not be empty".to_string(),
            )
        })?;
        // The description is a single scalar per repo: a batch carrying two
        // entries for the same source repo is a malformed/conflicting authority
        // input. Fail closed rather than silently last-wins — never let a buggy
        // producer batch pick a description non-deterministically.
        if let Some(prior) = descriptions_by_repo_id
            .insert(entry.source_repo_id.as_str().to_string(), description)
        {
            return Err(CoreError::InvalidContract(format!(
                "lexical: repo description ingest carries conflicting entries for source_repo_id `{}` (prior `{prior}`); one description per repo per batch",
                entry.source_repo_id.as_str()
            )));
        }
    }
    let payload = encode_repo_description_snapshot(&RepoDescriptionShard {
        descriptions_by_repo_id,
    })?;
    std::fs::write(path, payload).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: write repo description snapshot {}: {err}",
            path.display()
        ))
    })?;
    Ok(())
}

fn load_repo_description_snapshot(path: &Path) -> Result<Option<RepoDescriptionShard>, CoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let payload = std::fs::read(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: read repo description snapshot {}: {err}",
            path.display()
        ))
    })?;
    decode_repo_description_snapshot(payload.as_slice())
        .map(Some)
        .map_err(|err| match err {
            CoreError::InvalidContract(message) => CoreError::InvalidContract(format!(
                "lexical: repo description decode {}: {message}",
                path.display()
            )),
            other @ (CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => other,
        })
}

fn encode_file_ownership_snapshot(shard: &FileOwnershipShard) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, by_path) in &shard.owners_by_repo_id {
        for (repo_relative_path, owners) in by_path {
            entries.push(CborValue::Map(vec![
                (
                    CborValue::Text("source_repo_id".to_string()),
                    CborValue::Text(source_repo_id.clone()),
                ),
                (
                    CborValue::Text("repo_relative_path".to_string()),
                    CborValue::Text(repo_relative_path.clone()),
                ),
                (
                    CborValue::Text("owners".to_string()),
                    CborValue::Array(
                        owners
                            .iter()
                            .cloned()
                            .map(CborValue::Text)
                            .collect::<Vec<_>>(),
                    ),
                ),
            ]));
        }
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: file ownership encode: {err}"))
    })?;
    Ok(payload)
}

fn decode_file_ownership_snapshot(bytes: &[u8]) -> Result<FileOwnershipShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: file ownership decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: file ownership decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: file ownership decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: file ownership decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: file ownership decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: file ownership decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut owners_by_repo_id: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> =
        BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: file ownership decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: file ownership decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut repo_relative_path: Option<String> = None;
        let mut owners: Option<BTreeSet<String>> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: file ownership decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file ownership decode: `source_repo_id` must be text"
                                .to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "repo_relative_path" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file ownership decode: `repo_relative_path` must be text"
                                .to_string(),
                        ));
                    };
                    repo_relative_path = Some(text);
                }
                "owners" => {
                    let CborValue::Array(items) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file ownership decode: `owners` must be an array".to_string(),
                        ));
                    };
                    let mut normalized: BTreeSet<String> = BTreeSet::new();
                    for item in items {
                        let CborValue::Text(text) = item else {
                            return Err(CoreError::InvalidContract(
                                "lexical: file ownership decode: owner must be text".to_string(),
                            ));
                        };
                        let owner = normalize_owner_identity(&text).ok_or_else(|| {
                            CoreError::InvalidContract(
                                "lexical: file ownership decode: owner must be non-empty"
                                    .to_string(),
                            )
                        })?;
                        let _inserted = normalized.insert(owner);
                    }
                    owners = Some(normalized);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: file ownership decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file ownership decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let repo_relative_path = repo_relative_path.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file ownership decode: missing field `repo_relative_path`".to_string(),
            )
        })?;
        let owners = owners.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file ownership decode: missing field `owners`".to_string(),
            )
        })?;
        let _prior = owners_by_repo_id
            .entry(source_repo_id)
            .or_default()
            .insert(repo_relative_path, owners);
    }
    Ok(FileOwnershipShard { owners_by_repo_id })
}

fn persist_file_ownership_snapshot(
    path: &Path,
    batch: &FileOwnershipIngestBatch,
) -> Result<(), CoreError> {
    let mut owners_by_repo_id: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> =
        BTreeMap::new();
    for entry in &batch.entries {
        let repo_relative_path = entry.repo_relative_path.as_str().to_string();
        let owners = entry
            .owners
            .iter()
            .map(|owner| {
                normalize_owner_identity(owner).ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: file ownership ingest owner must not be empty".to_string(),
                    )
                })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let _prior = owners_by_repo_id
            .entry(entry.source_repo_id.as_str().to_string())
            .or_default()
            .insert(repo_relative_path, owners);
    }
    let payload = encode_file_ownership_snapshot(&FileOwnershipShard { owners_by_repo_id })?;
    std::fs::write(path, payload).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: write file ownership snapshot {}: {err}",
            path.display()
        ))
    })?;
    Ok(())
}

fn load_file_ownership_snapshot(path: &Path) -> Result<Option<FileOwnershipShard>, CoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let payload = std::fs::read(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: read file ownership snapshot {}: {err}",
            path.display()
        ))
    })?;
    decode_file_ownership_snapshot(payload.as_slice())
        .map(Some)
        .map_err(|err| match err {
            CoreError::InvalidContract(message) => CoreError::InvalidContract(format!(
                "lexical: file ownership decode {}: {message}",
                path.display()
            )),
            other @ (CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => other,
        })
}

fn encode_file_contributor_snapshot(shard: &FileContributorShard) -> Result<Vec<u8>, CoreError> {
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    for (source_repo_id, by_path) in &shard.contributors_by_repo_id {
        for (repo_relative_path, contributors) in by_path {
            entries.push(CborValue::Map(vec![
                (
                    CborValue::Text("source_repo_id".to_string()),
                    CborValue::Text(source_repo_id.clone()),
                ),
                (
                    CborValue::Text("repo_relative_path".to_string()),
                    CborValue::Text(repo_relative_path.clone()),
                ),
                (
                    CborValue::Text("contributors".to_string()),
                    CborValue::Array(
                        contributors
                            .iter()
                            .cloned()
                            .map(CborValue::Text)
                            .collect::<Vec<_>>(),
                    ),
                ),
            ]));
        }
    }
    let wire = CborValue::Map(vec![(
        CborValue::Text("entries".to_string()),
        CborValue::Array(entries),
    )]);
    ciborium::into_writer(&wire, &mut payload).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: file contributor encode: {err}"))
    })?;
    Ok(payload)
}

fn decode_file_contributor_snapshot(bytes: &[u8]) -> Result<FileContributorShard, CoreError> {
    let wire = ciborium::from_reader::<CborValue, _>(bytes).map_err(|err| {
        CoreError::InvalidContract(format!("lexical: file contributor decode: {err}"))
    })?;
    let CborValue::Map(fields) = wire else {
        return Err(CoreError::InvalidContract(
            "lexical: file contributor decode: expected map".to_string(),
        ));
    };
    let mut entries: Option<Vec<CborValue>> = None;
    for (key, value) in fields {
        let CborValue::Text(field_name) = key else {
            return Err(CoreError::InvalidContract(
                "lexical: file contributor decode: field name must be text".to_string(),
            ));
        };
        match field_name.as_str() {
            "entries" => {
                if entries.is_some() {
                    return Err(CoreError::InvalidContract(
                        "lexical: file contributor decode: duplicate field `entries`".to_string(),
                    ));
                }
                let CborValue::Array(items) = value else {
                    return Err(CoreError::InvalidContract(
                        "lexical: file contributor decode: `entries` must be an array".to_string(),
                    ));
                };
                entries = Some(items);
            }
            other => {
                return Err(CoreError::InvalidContract(format!(
                    "lexical: file contributor decode: unknown field `{other}`"
                )));
            }
        }
    }
    let mut contributors_by_repo_id: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> =
        BTreeMap::new();
    for entry in entries.ok_or_else(|| {
        CoreError::InvalidContract(
            "lexical: file contributor decode: missing field `entries`".to_string(),
        )
    })? {
        let CborValue::Map(fields) = entry else {
            return Err(CoreError::InvalidContract(
                "lexical: file contributor decode: entry must be a map".to_string(),
            ));
        };
        let mut source_repo_id: Option<String> = None;
        let mut repo_relative_path: Option<String> = None;
        let mut contributors: Option<BTreeSet<String>> = None;
        for (field_key, field_value) in fields {
            let CborValue::Text(field_name) = field_key else {
                return Err(CoreError::InvalidContract(
                    "lexical: file contributor decode: entry field name must be text".to_string(),
                ));
            };
            match field_name.as_str() {
                "source_repo_id" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file contributor decode: `source_repo_id` must be text"
                                .to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "repo_relative_path" => {
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file contributor decode: `repo_relative_path` must be text"
                                .to_string(),
                        ));
                    };
                    repo_relative_path = Some(text);
                }
                "contributors" => {
                    let CborValue::Array(items) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: file contributor decode: `contributors` must be an array"
                                .to_string(),
                        ));
                    };
                    let mut normalized: BTreeSet<String> = BTreeSet::new();
                    for item in items {
                        let CborValue::Text(text) = item else {
                            return Err(CoreError::InvalidContract(
                                "lexical: file contributor decode: contributor must be text"
                                    .to_string(),
                            ));
                        };
                        let contributor =
                            normalize_contributor_identity(&text).ok_or_else(|| {
                                CoreError::InvalidContract(
                                "lexical: file contributor decode: contributor must be non-empty"
                                    .to_string(),
                            )
                            })?;
                        let _inserted = normalized.insert(contributor);
                    }
                    contributors = Some(normalized);
                }
                other => {
                    return Err(CoreError::InvalidContract(format!(
                        "lexical: file contributor decode: unknown entry field `{other}`"
                    )));
                }
            }
        }
        let source_repo_id = source_repo_id.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file contributor decode: missing field `source_repo_id`".to_string(),
            )
        })?;
        let repo_relative_path = repo_relative_path.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file contributor decode: missing field `repo_relative_path`".to_string(),
            )
        })?;
        let contributors = contributors.ok_or_else(|| {
            CoreError::InvalidContract(
                "lexical: file contributor decode: missing field `contributors`".to_string(),
            )
        })?;
        let _prior = contributors_by_repo_id
            .entry(source_repo_id)
            .or_default()
            .insert(repo_relative_path, contributors);
    }
    Ok(FileContributorShard {
        contributors_by_repo_id,
    })
}

fn persist_file_contributor_snapshot(
    path: &Path,
    batch: &FileContributorIngestBatch,
) -> Result<(), CoreError> {
    let mut contributors_by_repo_id: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> =
        BTreeMap::new();
    for entry in &batch.entries {
        let repo_relative_path = entry.repo_relative_path.as_str().to_string();
        let contributors = entry
            .contributors
            .iter()
            .map(|contributor| {
                normalize_contributor_identity(contributor).ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: file contributor ingest contributor must not be empty"
                            .to_string(),
                    )
                })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let _prior = contributors_by_repo_id
            .entry(entry.source_repo_id.as_str().to_string())
            .or_default()
            .insert(repo_relative_path, contributors);
    }
    let payload = encode_file_contributor_snapshot(&FileContributorShard {
        contributors_by_repo_id,
    })?;
    std::fs::write(path, payload).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: write file contributor snapshot {}: {err}",
            path.display()
        ))
    })?;
    Ok(())
}

fn load_file_contributor_snapshot(path: &Path) -> Result<Option<FileContributorShard>, CoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let payload = std::fs::read(path).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: read file contributor snapshot {}: {err}",
            path.display()
        ))
    })?;
    decode_file_contributor_snapshot(payload.as_slice())
        .map(Some)
        .map_err(|err| match err {
            CoreError::InvalidContract(message) => CoreError::InvalidContract(format!(
                "lexical: file contributor decode {}: {message}",
                path.display()
            )),
            other @ (CoreError::Typed { .. }
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => other,
        })
}

/// Build an `LqOptions` snapshot pinned to the standard pattern type.
///
/// Used by internal scope-discovery compiles (predicate path lowering) where
/// the caller's regex options should NOT bleed into the discovery query —
/// those are user-facing leaves planned separately.
fn standard_pattern_options() -> LqOptions {
    let mut opts = LqOptions::defaults();
    opts.pattern_type = LqPatternType::Standard;
    opts
}

fn normalize_language(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn normalize_repo_meta_key(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn normalize_repo_topic_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn normalize_owner_identity(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn normalize_contributor_identity(value: &str) -> Option<String> {
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

fn case_sensitive_text_options() -> TextOptions {
    TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(CASE_SENSITIVE_TOKENIZER_NAME)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    )
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
    doc.add_text(fields.repo_relative_path_query, repo_relative_path);
    doc.add_text(fields.repo_relative_path_case, repo_relative_path);
    if let Some(file_name) = file_name_for_path(repo_relative_path) {
        doc.add_text(fields.file_name, file_name);
    }
    if let Some(language) = language.and_then(normalize_language) {
        doc.add_text(fields.language, &language);
    }
}

fn add_snippet_field(fields: &SchemaFields, doc: &mut TantivyDocument, snippet: &str) {
    doc.add_text(fields.snippet, snippet);
}

fn add_content_fields(fields: &SchemaFields, doc: &mut TantivyDocument, indexed_text: &str) {
    doc.add_text(fields.chunk_text, indexed_text);
    doc.add_text(fields.chunk_text_case, indexed_text);
}

fn add_symbol_fields(fields: &SchemaFields, doc: &mut TantivyDocument, symbol: &SymbolRecord) {
    doc.add_text(fields.symbol_kind, symbol.symbol_kind.as_str());
    if let Some(symbol_kind_family) = symbol.symbol_kind_family {
        doc.add_text(fields.symbol_kind_family, symbol_kind_family.as_code_str());
    }
}

fn register_index_tokenizers(index: &Index) {
    index.tokenizers().register(
        CASE_SENSITIVE_TOKENIZER_NAME,
        TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(RemoveLongFilter::limit(40))
            .build(),
    );
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
    let index = Index::builder()
        .schema(fields.schema.clone())
        .open_or_create(directory)
        .map_err(|err| CoreError::Storage(format!("lexical: open generation index: {err}")))?;
    register_index_tokenizers(&index);
    Ok(index)
}

type TextAuthorityDocTableRow = (u64, String, String, String);

#[derive(Clone)]
struct TextAuthorityDoc {
    candidate_id: String,
    indexed_text: String,
    folded_indexed_text: String,
}

#[derive(Clone, Debug, Default)]
struct RepoCommitRecencyShard {
    latest_committer_time_ms_by_repo_id: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default)]
struct RepoMetaShard {
    meta_by_repo_id: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Default)]
struct RepoTopicShard {
    topics_by_repo_id: BTreeMap<String, BTreeSet<String>>,
}

/// Source-repo keyed repo-description authority. One verbatim description string
/// per `source_repo_id`, matched as a regex at `repo:has.description(<pattern>)`
/// query time. Distinct substrate from [`RepoTopicShard`] (topic set) and the
/// repo-meta key/value store so description support never silently piggybacks
/// on unrelated authority.
struct RepoDescriptionShard {
    descriptions_by_repo_id: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default)]
struct FileOwnershipShard {
    owners_by_repo_id: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

#[derive(Clone, Debug, Default)]
struct FileContributorShard {
    contributors_by_repo_id: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

struct TextAuthorityShard {
    docs_by_id: BTreeMap<u64, TextAuthorityDoc>,
    trigram: TrigramIndex,
    trigram_folded: TrigramIndex,
    positions: PositionsIndex,
    positions_folded: PositionsIndex,
}

impl TextAuthorityShard {
    fn doc(&self, doc_id: u64) -> Option<&TextAuthorityDoc> {
        self.docs_by_id.get(&doc_id)
    }
}

struct TextAuthorityResolver<'a> {
    shard: &'a TextAuthorityShard,
    folded: bool,
}

impl DocResolver for TextAuthorityResolver<'_> {
    fn resolve(&self, doc_id: TrigramDocId) -> Option<&[u8]> {
        let doc = self.shard.doc(doc_id.0)?;
        if self.folded {
            Some(doc.folded_indexed_text.as_bytes())
        } else {
            Some(doc.indexed_text.as_bytes())
        }
    }
}

fn text_authority_doc_table_path(path: &Path) -> PathBuf {
    path.join(TEXT_AUTHORITY_DOC_TABLE_FILE_NAME)
}

fn text_authority_trigram_path(path: &Path) -> PathBuf {
    path.join(TEXT_AUTHORITY_TRIGRAM_FILE_NAME)
}

fn text_authority_trigram_folded_path(path: &Path) -> PathBuf {
    path.join(TEXT_AUTHORITY_TRIGRAM_FOLDED_FILE_NAME)
}

fn text_authority_positions_path(path: &Path) -> PathBuf {
    path.join(TEXT_AUTHORITY_POSITIONS_FILE_NAME)
}

fn text_authority_positions_folded_path(path: &Path) -> PathBuf {
    path.join(TEXT_AUTHORITY_POSITIONS_FOLDED_FILE_NAME)
}

fn sidecar_generation_id(generation: ManifestGeneration) -> u64 {
    generation.get().max(1)
}

fn map_trigram_error(context: &str, err: &TrigramError) -> CoreError {
    match err.code {
        TrigramErrorCode::PlanLimitExceeded => CoreError::Typed {
            code: "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED".to_string(),
            message: format!("lexical: {context}: {err}"),
        },
        TrigramErrorCode::RegexPrefilterUnusable => CoreError::Typed {
            code: "LEX_TRIGRAM_PREFILTER_UNUSABLE".to_string(),
            message: format!("lexical: {context}: {err}"),
        },
        TrigramErrorCode::IndexDeserialize | TrigramErrorCode::IndexCorrupted => {
            CoreError::Storage(format!("lexical: {context}: {err}"))
        }
        TrigramErrorCode::InvalidGeneration => {
            CoreError::InvalidContract(format!("lexical: {context}: {err}"))
        }
    }
}

fn map_positions_error(context: &str, err: &PositionsError) -> CoreError {
    match err.code {
        PositionsErrorCode::PlanLimitExceeded => CoreError::Typed {
            code: "LEX_PHRASE_PLAN_LIMIT_EXCEEDED".to_string(),
            message: format!("lexical: {context}: {err}"),
        },
        PositionsErrorCode::StateGenerationRegression
        | PositionsErrorCode::NormalizerVersionMismatch
        | PositionsErrorCode::IndexDeserialize
        | PositionsErrorCode::IndexCorrupted => {
            CoreError::Storage(format!("lexical: {context}: {err}"))
        }
        PositionsErrorCode::InvalidTerm | PositionsErrorCode::WindowOutOfRange => {
            CoreError::InvalidContract(format!("lexical: {context}: {err}"))
        }
    }
}

fn collect_text_authority_docs(
    index: &Index,
    fields: &SchemaFields,
) -> Result<Vec<(String, String, String)>, CoreError> {
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|err| CoreError::Storage(format!("lexical: text authority reader: {err}")))?;
    reader.reload().map_err(|err| {
        CoreError::Storage(format!("lexical: text authority reader reload: {err}"))
    })?;
    let searcher = reader.searcher();
    let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: num_docs overflow while rebuilding text authority: {err}"
        ))
    })?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let hits = searcher
        .search(&AllQuery, &TopDocs::with_limit(limit))
        .map_err(|err| CoreError::Storage(format!("lexical: text authority scan: {err}")))?;
    let mut docs: Vec<(String, String, String)> = Vec::new();
    for (_score, doc_address) in hits {
        let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: fetch text authority doc {doc_address:?}: {err}"
            ))
        })?;
        if stored_text(&doc, fields.doc_kind).as_deref() != Some(TEXT_DOC_KIND) {
            continue;
        }
        let candidate_id = stored_text(&doc, fields.candidate_id).ok_or_else(|| {
            CoreError::Storage("lexical: text authority doc missing candidate_id field".to_string())
        })?;
        let indexed_text = stored_text(&doc, fields.chunk_text).ok_or_else(|| {
            CoreError::Storage("lexical: text authority doc missing chunk_text field".to_string())
        })?;
        let folded = indexed_text.to_ascii_lowercase();
        docs.push((candidate_id, indexed_text, folded));
    }
    docs.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(docs)
}

fn phrase_term_positions(text: &str, case_sensitive: bool) -> Vec<(String, Position)> {
    tokenize_phrase_terms(text, case_sensitive)
        .into_iter()
        .enumerate()
        .map(|(idx, term)| {
            (
                term,
                Position(u32::try_from(idx).map_or(u32::MAX, core::convert::identity)),
            )
        })
        .collect()
}

fn persist_text_authority_sidecars(
    path: &Path,
    fields: &SchemaFields,
    index: &Index,
    generation: ManifestGeneration,
) -> Result<(), CoreError> {
    let docs = collect_text_authority_docs(index, fields)?;
    let sidecar_generation = sidecar_generation_id(generation);
    let mut trigram = TrigramIndexBuilder::new(sidecar_generation)
        .map_err(|err| map_trigram_error("init trigram sidecar", &err))?;
    let mut trigram_folded = TrigramIndexBuilder::new(sidecar_generation)
        .map_err(|err| map_trigram_error("init folded trigram sidecar", &err))?;
    let mut positions = PositionsBuilder::new(sidecar_generation, POSITIONS_NORMALIZER_VERSION);
    let mut positions_folded =
        PositionsBuilder::new(sidecar_generation, POSITIONS_NORMALIZER_VERSION);
    let mut table: Vec<TextAuthorityDocTableRow> = Vec::with_capacity(docs.len());
    for (offset, (candidate_id, indexed_text, folded_indexed_text)) in docs.into_iter().enumerate()
    {
        let doc_id = u64::try_from(offset.saturating_add(1)).map_err(|err| {
            CoreError::InvalidContract(format!("lexical: text authority doc id overflow: {err}"))
        })?;
        trigram.add_doc(TrigramDocId(doc_id), indexed_text.as_bytes());
        trigram_folded.add_doc(TrigramDocId(doc_id), folded_indexed_text.as_bytes());
        let sensitive_pairs = phrase_term_positions(&indexed_text, true);
        positions
            .upsert_doc(
                PositionsDocId(doc_id),
                sensitive_pairs
                    .iter()
                    .map(|(term, pos)| (term.as_str(), *pos)),
            )
            .map_err(|err| map_positions_error("build positions sidecar", &err))?;
        let folded_pairs = phrase_term_positions(&indexed_text, false);
        positions_folded
            .upsert_doc(
                PositionsDocId(doc_id),
                folded_pairs.iter().map(|(term, pos)| (term.as_str(), *pos)),
            )
            .map_err(|err| map_positions_error("build folded positions sidecar", &err))?;
        table.push((doc_id, candidate_id, indexed_text, folded_indexed_text));
    }
    let trigram = trigram.finish();
    let trigram_folded = trigram_folded.finish();
    let positions = positions
        .finish()
        .map_err(|err| map_positions_error("finalize positions sidecar", &err))?;
    let positions_folded = positions_folded
        .finish()
        .map_err(|err| map_positions_error("finalize folded positions sidecar", &err))?;

    trigram
        .serialize_cbor(
            std::fs::File::create(text_authority_trigram_path(path)).map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: create trigram sidecar {}: {err}",
                    text_authority_trigram_path(path).display()
                ))
            })?,
        )
        .map_err(|err| map_trigram_error("write trigram sidecar", &err))?;
    trigram_folded
        .serialize_cbor(
            std::fs::File::create(text_authority_trigram_folded_path(path)).map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: create folded trigram sidecar {}: {err}",
                    text_authority_trigram_folded_path(path).display()
                ))
            })?,
        )
        .map_err(|err| map_trigram_error("write folded trigram sidecar", &err))?;
    positions
        .serialize_cbor(
            &mut std::fs::File::create(text_authority_positions_path(path)).map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: create positions sidecar {}: {err}",
                    text_authority_positions_path(path).display()
                ))
            })?,
        )
        .map_err(|err| map_positions_error("write positions sidecar", &err))?;
    positions_folded
        .serialize_cbor(
            &mut std::fs::File::create(text_authority_positions_folded_path(path)).map_err(
                |err| {
                    CoreError::Storage(format!(
                        "lexical: create folded positions sidecar {}: {err}",
                        text_authority_positions_folded_path(path).display()
                    ))
                },
            )?,
        )
        .map_err(|err| map_positions_error("write folded positions sidecar", &err))?;
    ciborium::into_writer(
        &table,
        std::fs::File::create(text_authority_doc_table_path(path)).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: create text authority doc table {}: {err}",
                text_authority_doc_table_path(path).display()
            ))
        })?,
    )
    .map_err(|err| {
        CoreError::Storage(format!(
            "lexical: write text authority doc table {}: {err}",
            text_authority_doc_table_path(path).display()
        ))
    })?;
    Ok(())
}

fn load_text_authority_sidecars(path: &Path) -> Result<Option<TextAuthorityShard>, CoreError> {
    let doc_table_path = text_authority_doc_table_path(path);
    let trigram_path = text_authority_trigram_path(path);
    let trigram_folded_path = text_authority_trigram_folded_path(path);
    let positions_path = text_authority_positions_path(path);
    let positions_folded_path = text_authority_positions_folded_path(path);
    let any_exists = [
        doc_table_path.as_path(),
        trigram_path.as_path(),
        trigram_folded_path.as_path(),
        positions_path.as_path(),
        positions_folded_path.as_path(),
    ]
    .into_iter()
    .any(Path::exists);
    if !any_exists {
        return Ok(None);
    }
    for required in [
        doc_table_path.as_path(),
        trigram_path.as_path(),
        trigram_folded_path.as_path(),
        positions_path.as_path(),
        positions_folded_path.as_path(),
    ] {
        if !required.exists() {
            return Err(CoreError::Storage(format!(
                "lexical: text authority sidecar incomplete under {} (missing {})",
                path.display(),
                required.display()
            )));
        }
    }
    let rows: Vec<TextAuthorityDocTableRow> =
        ciborium::from_reader(std::fs::File::open(&doc_table_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: open text authority doc table {}: {err}",
                doc_table_path.display()
            ))
        })?)
        .map_err(|err| {
            CoreError::Storage(format!(
                "lexical: decode text authority doc table {}: {err}",
                doc_table_path.display()
            ))
        })?;
    let mut docs_by_id: BTreeMap<u64, TextAuthorityDoc> = BTreeMap::new();
    for (doc_id, candidate_id, indexed_text, folded_indexed_text) in rows {
        let prior = docs_by_id.insert(
            doc_id,
            TextAuthorityDoc {
                candidate_id,
                indexed_text,
                folded_indexed_text,
            },
        );
        if prior.is_some() {
            return Err(CoreError::Storage(format!(
                "lexical: duplicate text authority doc id {doc_id} in {}",
                doc_table_path.display()
            )));
        }
    }
    let trigram =
        TrigramIndex::deserialize_cbor(std::fs::File::open(&trigram_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: open trigram sidecar {}: {err}",
                trigram_path.display()
            ))
        })?)
        .map_err(|err| map_trigram_error("load trigram sidecar", &err))?;
    let trigram_folded = TrigramIndex::deserialize_cbor(
        std::fs::File::open(&trigram_folded_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: open folded trigram sidecar {}: {err}",
                trigram_folded_path.display()
            ))
        })?,
    )
    .map_err(|err| map_trigram_error("load folded trigram sidecar", &err))?;
    let positions =
        PositionsIndex::deserialize_cbor(std::fs::File::open(&positions_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: open positions sidecar {}: {err}",
                positions_path.display()
            ))
        })?)
        .map_err(|err| map_positions_error("load positions sidecar", &err))?;
    let positions_folded = PositionsIndex::deserialize_cbor(
        std::fs::File::open(&positions_folded_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: open folded positions sidecar {}: {err}",
                positions_folded_path.display()
            ))
        })?,
    )
    .map_err(|err| map_positions_error("load folded positions sidecar", &err))?;
    Ok(Some(TextAuthorityShard {
        docs_by_id,
        trigram,
        trigram_folded,
        positions,
        positions_folded,
    }))
}

fn copy_generation_directory(src: &Path, dst: &Path) -> Result<(), CoreError> {
    if !src.exists() {
        return Err(CoreError::NotReady(format!(
            "lexical: base generation missing at {}",
            src.display()
        )));
    }
    std::fs::create_dir_all(dst).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: create cloned generation directory {}: {err}",
            dst.display()
        ))
    })?;
    for entry in std::fs::read_dir(src).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: list base generation directory {}: {err}",
            src.display()
        ))
    })? {
        let entry = entry.map_err(|err| {
            CoreError::Storage(format!(
                "lexical: read base generation entry {}: {err}",
                src.display()
            ))
        })?;
        let entry_path = entry.path();
        let target_path = dst.join(entry.file_name());
        let file_type = entry.file_type().map_err(|err| {
            CoreError::Storage(format!(
                "lexical: inspect base generation entry {}: {err}",
                entry_path.display()
            ))
        })?;
        if file_type.is_dir() {
            copy_generation_directory(&entry_path, &target_path)?;
            continue;
        }
        let _bytes_copied: u64 = std::fs::copy(&entry_path, &target_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: copy {} -> {}: {err}",
                entry_path.display(),
                target_path.display()
            ))
        })?;
    }
    Ok(())
}

/// Tantivy-backed lexical adapter.
pub struct LexicalAdapter {
    state_root: PathBuf,
    fields: SchemaFields,
    writers: Arc<Mutex<WriterCache>>,
    repo_metadata: Arc<Mutex<BTreeMap<GenKey, LexicalRepoMetadataPayload>>>,
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

    fn repo_commit_recency_path(&self, key: &GenKey) -> PathBuf {
        self.index_path(key).join(REPO_COMMIT_RECENCY_FILE_NAME)
    }

    fn repo_meta_path(&self, key: &GenKey) -> PathBuf {
        self.index_path(key).join(REPO_META_FILE_NAME)
    }

    fn repo_topic_path(&self, key: &GenKey) -> PathBuf {
        self.index_path(key).join(REPO_TOPIC_FILE_NAME)
    }

    fn repo_description_path(&self, key: &GenKey) -> PathBuf {
        self.index_path(key).join(REPO_DESCRIPTION_FILE_NAME)
    }

    fn file_ownership_path(&self, key: &GenKey) -> PathBuf {
        self.index_path(key).join(FILE_OWNERSHIP_FILE_NAME)
    }

    fn file_contributor_path(&self, key: &GenKey) -> PathBuf {
        self.index_path(key).join(FILE_CONTRIBUTOR_FILE_NAME)
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
    ) -> Result<Option<LexicalRepoMetadataPayload>, CoreError> {
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

    fn prepare_generation_for_ops(
        &self,
        key: &GenKey,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        let target_path = self.index_path(key);
        if target_path.exists() {
            return Ok(());
        }
        for op in ops {
            let base_generation = match op {
                LexicalChannelOp::ReplaceLexicalScope(payload) => {
                    let (_mode, base_generation, _scope) =
                        decode_replace_scope_payload(&payload.payload)?;
                    base_generation
                }
                LexicalChannelOp::TombstoneLexicalScope(payload) => {
                    let (_mode, base_generation, _scope) =
                        decode_tombstone_scope_payload(&payload.payload)?;
                    base_generation
                }
                LexicalChannelOp::FullBundle(_)
                | LexicalChannelOp::UpsertChunk(_)
                | LexicalChannelOp::DeleteChunk(_)
                | LexicalChannelOp::UpsertSymbol(_)
                | LexicalChannelOp::DeleteSymbol(_)
                | LexicalChannelOp::Seal(_)
                | LexicalChannelOp::UpsertCommit(_)
                | LexicalChannelOp::UpsertRef(_)
                | LexicalChannelOp::UpsertTag(_)
                | LexicalChannelOp::DeleteRef(_)
                | LexicalChannelOp::DeleteTag(_)
                | LexicalChannelOp::UpsertDirty(_)
                | LexicalChannelOp::EvictDirty(_)
                | LexicalChannelOp::UpsertParseTree(_)
                | LexicalChannelOp::DeleteParseTree(_)
                | LexicalChannelOp::ReplaceStructuralScope(_)
                | LexicalChannelOp::TombstoneStructuralScope(_)
                | LexicalChannelOp::UpsertDiffHunk(_) => continue,
            };
            if let Some(base_generation) = base_generation {
                let base_key = GenKey {
                    repo_id: key.repo_id.clone(),
                    revision_id: key.revision_id.clone(),
                    generation: base_generation,
                };
                copy_generation_directory(&self.index_path(&base_key), &target_path)?;
            }
            break;
        }
        Ok(())
    }

    fn delete_scope_docs(&self, writer: &IndexWriter, repo_relative_path: &RepoRelativePath) {
        let term =
            Term::from_field_text(self.fields.repo_relative_path, repo_relative_path.as_str());
        let _opstamp = writer.delete_term(term);
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
                doc.add_text(
                    self.fields.repo_id,
                    chunk.searchable_repo_id(&key.repo_id).as_str(),
                );
                doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                doc.add_text(self.fields.doc_kind, TEXT_DOC_KIND);
                add_metadata_fields(
                    &self.fields,
                    &mut doc,
                    chunk.repo_relative_path.as_str(),
                    Some(chunk.language.as_str()),
                );
                doc.add_u64(self.fields.start_line, u64::from(chunk.start_line));
                doc.add_u64(self.fields.end_line, u64::from(chunk.end_line));
                add_snippet_field(&self.fields, &mut doc, chunk.derived_snippet());
                add_content_fields(&self.fields, &mut doc, chunk.text.as_ref());
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
                    symbol.repo_relative_path.as_str(),
                    Some(symbol.language.as_str()),
                );
                doc.add_u64(
                    self.fields.start_line,
                    u64::from(symbol.definition_span.line_start),
                );
                doc.add_u64(
                    self.fields.end_line,
                    u64::from(symbol.definition_span.line_end),
                );
                let snippet = match symbol.container_qualified_name.as_deref() {
                    Some(container) if !container.is_empty() => {
                        format!("{} {}", symbol.local_name.as_ref(), container)
                    }
                    _ => symbol.local_name.as_ref().to_string(),
                };
                add_snippet_field(&self.fields, &mut doc, &snippet);
                add_content_fields(&self.fields, &mut doc, &snippet);
                add_symbol_fields(&self.fields, &mut doc, &symbol);
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
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_replace_scope_payload(&payload.payload)?;
                self.delete_scope_docs(writer, &scope.scope.repo_relative_path);
                for chunk in &scope.chunks {
                    let mut doc = TantivyDocument::new();
                    doc.add_text(self.fields.candidate_id, chunk.chunk_id.as_str());
                    doc.add_text(
                        self.fields.repo_id,
                        chunk.searchable_repo_id(&key.repo_id).as_str(),
                    );
                    doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                    doc.add_text(self.fields.doc_kind, TEXT_DOC_KIND);
                    add_metadata_fields(
                        &self.fields,
                        &mut doc,
                        chunk.repo_relative_path.as_str(),
                        Some(chunk.language.as_str()),
                    );
                    doc.add_u64(self.fields.start_line, u64::from(chunk.start_line));
                    doc.add_u64(self.fields.end_line, u64::from(chunk.end_line));
                    add_snippet_field(&self.fields, &mut doc, chunk.derived_snippet());
                    add_content_fields(&self.fields, &mut doc, chunk.text.as_ref());
                    let _opstamp = writer.add_document(doc).map_err(|err| {
                        CoreError::Storage(format!("lexical: add_document: {err}"))
                    })?;
                }
                for symbol in &scope.symbols {
                    let mut doc = TantivyDocument::new();
                    doc.add_text(self.fields.candidate_id, symbol.symbol_id.as_str());
                    doc.add_text(self.fields.repo_id, key.repo_id.as_str());
                    doc.add_text(self.fields.revision_id, key.revision_id.as_str());
                    doc.add_text(self.fields.doc_kind, SYMBOL_DOC_KIND);
                    add_metadata_fields(
                        &self.fields,
                        &mut doc,
                        symbol.repo_relative_path.as_str(),
                        Some(symbol.language.as_str()),
                    );
                    doc.add_u64(
                        self.fields.start_line,
                        u64::from(symbol.definition_span.line_start),
                    );
                    doc.add_u64(
                        self.fields.end_line,
                        u64::from(symbol.definition_span.line_end),
                    );
                    let snippet = match symbol.container_qualified_name.as_deref() {
                        Some(container) if !container.is_empty() => {
                            format!("{} {}", symbol.local_name.as_ref(), container)
                        }
                        _ => symbol.local_name.as_ref().to_string(),
                    };
                    add_snippet_field(&self.fields, &mut doc, &snippet);
                    add_content_fields(&self.fields, &mut doc, &snippet);
                    add_symbol_fields(&self.fields, &mut doc, symbol);
                    let _opstamp = writer.add_document(doc).map_err(|err| {
                        CoreError::Storage(format!("lexical: add_document: {err}"))
                    })?;
                }
                Ok(true)
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_tombstone_scope_payload(&payload.payload)?;
                self.delete_scope_docs(writer, &scope.scope.repo_relative_path);
                Ok(true)
            }
            // FullBundle/Seal carry no document-level effect (dispatcher's
            // ledger update observes Seal).
            LexicalChannelOp::FullBundle(bundle) => {
                let metadata = decode_repo_metadata_payload(&bundle.payload)?;
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
            | LexicalChannelOp::ReplaceStructuralScope(_)
            | LexicalChannelOp::TombstoneStructuralScope(_)
            | LexicalChannelOp::UpsertDiffHunk(_) => Ok(false),
        }
    }
}

fn op_touches_text_authority(op: &LexicalChannelOp) -> bool {
    matches!(
        op,
        LexicalChannelOp::UpsertChunk(_)
            | LexicalChannelOp::DeleteChunk(_)
            | LexicalChannelOp::ReplaceLexicalScope(_)
            | LexicalChannelOp::TombstoneLexicalScope(_)
    )
}

fn legacy_ops_for_batch(
    batch: &LexicalIngestBatch,
    include_seal: bool,
) -> Result<Vec<LexicalChannelOp>, CoreError> {
    let op_capacity = batch
        .bundle_payload
        .as_ref()
        .map_or(0usize, |_payload| 1usize)
        .saturating_add(
            batch
                .replace_scopes
                .len()
                .saturating_add(batch.tombstone_scopes.len())
                .saturating_add(usize::from(include_seal)),
        );
    let mut ops = Vec::with_capacity(op_capacity);
    if let Some(payload) = batch.bundle_payload.as_ref() {
        ops.push(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            payload: payload.clone(),
        }));
    }
    for scope in &batch.replace_scopes {
        ops.push(LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            payload: encode_cbor(
                &(batch.mode, batch.base_generation, scope.clone()),
                "replace lexical scope payload",
            )?,
        }));
    }
    for scope in &batch.tombstone_scopes {
        ops.push(LexicalChannelOp::TombstoneLexicalScope(
            TombstoneLexicalScope {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: encode_cbor(
                    &(batch.mode, batch.base_generation, scope.clone()),
                    "tombstone lexical scope payload",
                )?,
            },
        ));
    }
    if include_seal {
        ops.push(LexicalChannelOp::Seal(LexicalSeal {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        }));
    }
    Ok(ops)
}

impl LexicalBatchBuildPort for LexicalAdapter {
    fn build_batch(&self, batch: &LexicalIngestBatch) -> Result<(), CoreError> {
        let ops = legacy_ops_for_batch(batch, batch.seal)?;
        self.build(&batch.repo_id, &batch.revision_id, batch.generation, &ops)
    }
}

impl RepoCommitRecencyIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &RepoCommitRecencyIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let index_path = self.index_path(&key);
        std::fs::create_dir_all(&index_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: create repo commit recency dir {}: {err}",
                index_path.display()
            ))
        })?;
        persist_repo_commit_recency_snapshot(self.repo_commit_recency_path(&key).as_path(), batch)?;
        let mut receipt = quanta_index_contract::BatchPublishReceipt::empty_for(
            batch.generation,
            batch.batch_digest.clone(),
        );
        for _entry in &batch.entries {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }
}

impl RepoMetaIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &RepoMetaIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let index_path = self.index_path(&key);
        std::fs::create_dir_all(&index_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: create repo meta dir {}: {err}",
                index_path.display()
            ))
        })?;
        persist_repo_meta_snapshot(self.repo_meta_path(&key).as_path(), batch)?;
        let mut receipt = quanta_index_contract::BatchPublishReceipt::empty_for(
            batch.generation,
            batch.batch_digest.clone(),
        );
        for _entry in &batch.entries {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }
}

impl RepoTopicIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &RepoTopicIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let index_path = self.index_path(&key);
        std::fs::create_dir_all(&index_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: create repo topic dir {}: {err}",
                index_path.display()
            ))
        })?;
        persist_repo_topic_snapshot(self.repo_topic_path(&key).as_path(), batch)?;
        let mut receipt = quanta_index_contract::BatchPublishReceipt::empty_for(
            batch.generation,
            batch.batch_digest.clone(),
        );
        for _entry in &batch.entries {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }
}

impl RepoDescriptionIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &RepoDescriptionIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let index_path = self.index_path(&key);
        std::fs::create_dir_all(&index_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: create repo description dir {}: {err}",
                index_path.display()
            ))
        })?;
        persist_repo_description_snapshot(self.repo_description_path(&key).as_path(), batch)?;
        let mut receipt = quanta_index_contract::BatchPublishReceipt::empty_for(
            batch.generation,
            batch.batch_digest.clone(),
        );
        for _entry in &batch.entries {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }
}

impl FileOwnershipIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &FileOwnershipIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let index_path = self.index_path(&key);
        std::fs::create_dir_all(&index_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: create file ownership dir {}: {err}",
                index_path.display()
            ))
        })?;
        persist_file_ownership_snapshot(self.file_ownership_path(&key).as_path(), batch)?;
        let mut receipt = quanta_index_contract::BatchPublishReceipt::empty_for(
            batch.generation,
            batch.batch_digest.clone(),
        );
        for _entry in &batch.entries {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }
}

impl FileContributorIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &FileContributorIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let index_path = self.index_path(&key);
        std::fs::create_dir_all(&index_path).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: create file contributor dir {}: {err}",
                index_path.display()
            ))
        })?;
        persist_file_contributor_snapshot(self.file_contributor_path(&key).as_path(), batch)?;
        let mut receipt = quanta_index_contract::BatchPublishReceipt::empty_for(
            batch.generation,
            batch.batch_digest.clone(),
        );
        for _entry in &batch.entries {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
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
        self.prepare_generation_for_ops(&key, ops)?;
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
        let mut needs_text_authority_rebuild = false;
        for op in ops {
            if self.apply_op(&guarded.writer, key, op)? {
                needs_commit = true;
                if op_touches_text_authority(op) {
                    needs_text_authority_rebuild = true;
                }
            }
        }
        if needs_commit {
            let _opstamp = guarded
                .writer
                .commit()
                .map_err(|err| CoreError::Storage(format!("lexical: commit: {err}")))?;
            if needs_text_authority_rebuild {
                persist_text_authority_sidecars(
                    self.index_path(key).as_path(),
                    &self.fields,
                    &guarded.index,
                    key.generation,
                )?;
            }
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
        let text_authority = load_text_authority_sidecars(&path)?;
        let repo_commit_recency =
            load_repo_commit_recency_snapshot(&self.repo_commit_recency_path(&key))?;
        let repo_meta = load_repo_meta_snapshot(&self.repo_meta_path(&key))?;
        let repo_topic = load_repo_topic_snapshot(&self.repo_topic_path(&key))?;
        let repo_description = load_repo_description_snapshot(&self.repo_description_path(&key))?;
        let file_ownership = load_file_ownership_snapshot(&self.file_ownership_path(&key))?;
        let file_contributor = load_file_contributor_snapshot(&self.file_contributor_path(&key))?;
        Ok(Box::new(TantivySearcher {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
            fields: self.fields.clone(),
            index,
            reader,
            repo_metadata,
            regex_policy: self.regex_policy,
            text_authority,
            repo_commit_recency,
            repo_meta,
            repo_topic,
            repo_description,
            file_ownership,
            file_contributor,
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
    repo_metadata: Option<LexicalRepoMetadataPayload>,
    /// Deployment-scoped regex policy threaded from the adapter at open time.
    /// Read at the regex-leaf compile site rather than fabricated there, so
    /// the dialect/literal/trigram-cap knobs are a single source of truth.
    regex_policy: RegexPolicy,
    text_authority: Option<TextAuthorityShard>,
    repo_commit_recency: Option<RepoCommitRecencyShard>,
    repo_meta: Option<RepoMetaShard>,
    repo_topic: Option<RepoTopicShard>,
    repo_description: Option<RepoDescriptionShard>,
    file_ownership: Option<FileOwnershipShard>,
    file_contributor: Option<FileContributorShard>,
}

struct PreparedPredicatePlan {
    expr: LqExpr,
    allowed_paths: Option<BTreeSet<String>>,
    allowed_repo_ids: Option<BTreeSet<String>>,
    allowed_candidate_ids: Option<BTreeSet<String>>,
    force_empty: bool,
}

struct PreparedExecutableQuery {
    query: LqQuery,
    predicate_plan: PreparedPredicatePlan,
    doc_kind: QueryDocKind,
}

/// A repo-existence gate to intersect into `allowed_repo_ids`.
///
/// `repo.has.file` gates by an indexed `path:`/`name:`/`lang:` matcher;
/// `repo.has.content` gates by an indexed content query. Both narrow the
/// eligible repo surface, so `prepare_predicate_plan` treats them uniformly.
enum RepoScopeConstraint {
    File(RepoFileConstraint),
    Content(LqLeaf),
    CommitAfter(String),
    Meta(RepoMetaArg),
    Topic(RepoTopicArg),
    Description(RepoDescriptionArg),
}

/// Lower a validated `repo.has.content` scalar into a content leaf, applying the
/// same `/regex/`-delimiter stripping the `ContentLeaf` family uses so a regex
/// content gate stays a regex.
fn content_leaf_from_scalar(arg: &ContentScalarArg) -> LqLeaf {
    match arg {
        ContentScalarArg::Keyword(value) => {
            if let Some(regex) = strip_regex_delimiters(value) {
                return LqLeaf::Regex(regex.to_string());
            }
            LqLeaf::Keyword(value.clone())
        }
        ContentScalarArg::Phrase(value) => LqLeaf::Phrase(value.clone()),
        ContentScalarArg::RawString(value) => {
            if let Some(regex) = strip_regex_delimiters(value) {
                return LqLeaf::Regex(regex.to_string());
            }
            LqLeaf::RawString(value.clone())
        }
        ContentScalarArg::Number(value) => LqLeaf::Keyword(value.to_string()),
    }
}

fn symbol_name_predicate_leaf(args: &[LqPredicateArg]) -> Result<LqLeaf, CoreError> {
    match args {
        [LqPredicateArg::Keyword(value)] => Ok(LqLeaf::Keyword(value.clone())),
        [LqPredicateArg::Phrase(value)] => Ok(LqLeaf::Phrase(value.clone())),
        [LqPredicateArg::RawString(value)] => Ok(LqLeaf::RawString(value.clone())),
        [LqPredicateArg::Number(_) | LqPredicateArg::Filter { .. }] | [] | [_, _, ..] => {
            Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `symbol.has.name` only supports exactly one keyword/phrase/raw-string argument (owner: {PREDICATE_OWNER})"
            )))
        }
    }
}

fn rewrite_symbol_name_predicate_query(query: &LqQuery) -> Result<Option<LqQuery>, CoreError> {
    let LqExpr::Leaf(LqLeaf::Predicate { name, args }) = &query.expr else {
        return Ok(None);
    };
    if name != "symbol.has.name" {
        return Ok(None);
    }

    let needle = symbol_name_predicate_leaf(args)?;
    let mut rewritten = query.clone();
    rewritten.expr = LqExpr::Leaf(needle);
    if !rewritten.filters.iter().any(|filter| {
        matches!(
            filter,
            LqFilter::Type {
                kind: LqType::Symbol
            } | LqFilter::Select {
                dim: LqSelect::Symbol
            }
        )
    }) {
        rewritten.filters.push(LqFilter::Type {
            kind: LqType::Symbol,
        });
    }
    Ok(Some(rewritten))
}

impl TantivySearcher {
    fn is_case_sensitive(options: &LqOptions) -> bool {
        matches!(options.case, Some(quanta_index_contract::LqCase::Sensitive))
    }

    fn full_recall_limit(&self, requested: usize, limit: usize) -> usize {
        usize::try_from(self.reader.searcher().num_docs()).map_or_else(
            |_| requested.max(limit),
            |docs| requested.max(limit).max(docs),
        )
    }

    fn effective_limit(&self, query: &LqQuery, requested: usize) -> usize {
        match query.options.count {
            Some(quanta_index_contract::LqCountBound::Bounded(bound)) => {
                usize::try_from(bound).map_or(requested, |bound| requested.min(bound))
            }
            Some(quanta_index_contract::LqCountBound::All) => {
                self.full_recall_limit(requested, requested)
            }
            None => requested,
        }
    }

    fn collect_limit(&self, query: &LqQuery, requested: usize, limit: usize) -> usize {
        let full_recall_limit = self.full_recall_limit(requested, limit);
        if Self::projects_repo_surface(query)
            || Self::projects_path_surface(query)
            || Self::selects_file_projection(query)
            || matches!(
                query.options.count,
                Some(quanta_index_contract::LqCountBound::Bounded(_))
            )
        {
            return full_recall_limit;
        }
        if limit >= full_recall_limit {
            full_recall_limit
        } else {
            limit.saturating_add(1).min(full_recall_limit)
        }
    }

    fn boundary_tie_detected(hits: &[(f32, DocAddress)], limit: usize) -> bool {
        if limit == 0 || hits.len() <= limit {
            return false;
        }
        let Some(boundary_index) = limit.checked_sub(1) else {
            return false;
        };
        let Some(boundary_score) = hits.get(boundary_index).map(|hit| hit.0) else {
            return false;
        };
        let Some(overflow_score) = hits.get(limit).map(|hit| hit.0) else {
            return false;
        };
        boundary_score.total_cmp(&overflow_score).is_eq()
    }

    fn candidate_precedes(candidate: &LexicalCandidate, current: &LexicalCandidate) -> bool {
        candidate.score.total_cmp(&current.score).is_gt()
            || (candidate.score.total_cmp(&current.score).is_eq()
                && (
                    candidate.repo_relative_path.as_str(),
                    candidate.start_line,
                    candidate.end_line,
                    candidate.candidate_id.as_str(),
                ) < (
                    current.repo_relative_path.as_str(),
                    current.start_line,
                    current.end_line,
                    current.candidate_id.as_str(),
                ))
    }

    fn symbol_candidate_precedes(candidate: &SymbolCandidate, current: &SymbolCandidate) -> bool {
        candidate.score.total_cmp(&current.score).is_gt()
            || (candidate.score.total_cmp(&current.score).is_eq()
                && (
                    candidate.repo_relative_path.as_str(),
                    candidate.start_line,
                    candidate.end_line,
                    candidate.candidate_id.as_str(),
                ) < (
                    current.repo_relative_path.as_str(),
                    current.start_line,
                    current.end_line,
                    current.candidate_id.as_str(),
                ))
    }

    fn stabilize_and_cap_hits(
        &self,
        query: &LqQuery,
        mut hits: Vec<LexicalCandidate>,
        limit: usize,
    ) -> Vec<LexicalCandidate> {
        if matches!(
            query.options.count,
            Some(quanta_index_contract::LqCountBound::Bounded(_))
        ) {
            hits.sort_by(|left, right| {
                if Self::candidate_precedes(left, right) {
                    std::cmp::Ordering::Less
                } else if Self::candidate_precedes(right, left) {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            });
        }
        if hits.len() > limit {
            hits.truncate(limit);
        }
        hits
    }

    fn stabilize_and_cap_symbol_hits(
        mut hits: Vec<SymbolCandidate>,
        limit: usize,
    ) -> Vec<SymbolCandidate> {
        hits.sort_by(|left, right| {
            if Self::symbol_candidate_precedes(left, right) {
                std::cmp::Ordering::Less
            } else if Self::symbol_candidate_precedes(right, left) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        if hits.len() > limit {
            hits.truncate(limit);
        }
        hits
    }

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

    fn query_parser(&self, options: &LqOptions, include_path_terms: bool) -> QueryParser {
        let mut fields: Vec<Field> = Vec::with_capacity(if include_path_terms { 2 } else { 1 });
        if Self::is_case_sensitive(options) {
            fields.push(self.fields.chunk_text_case);
            if include_path_terms {
                fields.push(self.fields.repo_relative_path_case);
            }
        } else {
            fields.push(self.fields.chunk_text);
            if include_path_terms {
                fields.push(self.fields.repo_relative_path_query);
            }
        }
        QueryParser::for_index(&self.index, fields)
    }

    fn enables_path_term_surface(expr: &LqExpr, options: &LqOptions) -> bool {
        options.pattern_type != LqPatternType::Regexp
            && matches!(expr, LqExpr::Leaf(LqLeaf::Keyword(_)))
    }

    fn repo_id_restriction_query(&self, repo_ids: &BTreeSet<String>) -> Box<dyn Query> {
        if repo_ids.len() == 1
            && let Some(repo_id) = repo_ids.iter().next()
        {
            return self.exact_text_query(self.fields.repo_id, repo_id);
        }
        Box::new(BooleanQuery::new(
            repo_ids
                .iter()
                .map(|repo_id| {
                    (
                        Occur::Should,
                        self.exact_text_query(self.fields.repo_id, repo_id),
                    )
                })
                .collect(),
        ))
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

    fn candidate_restriction_query(&self, candidate_ids: &BTreeSet<String>) -> Box<dyn Query> {
        if candidate_ids.len() == 1
            && let Some(candidate_id) = candidate_ids.iter().next()
        {
            return self.exact_text_query(self.fields.candidate_id, candidate_id);
        }
        Box::new(BooleanQuery::new(
            candidate_ids
                .iter()
                .map(|candidate_id| {
                    (
                        Occur::Should,
                        self.exact_text_query(self.fields.candidate_id, candidate_id),
                    )
                })
                .collect(),
        ))
    }

    fn match_none_query(&self) -> Box<dyn Query> {
        Box::new(BooleanQuery::new(vec![
            (Occur::Must, Box::new(AllQuery)),
            (Occur::MustNot, Box::new(AllQuery)),
        ]))
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
        // Predicate scope collection (`file.contains` / `file.has.content`)
        // intentionally compiles with `LqPatternType::Standard` regardless of
        // the caller's pattern type: it is a path-discovery prelude, not a
        // user-facing leaf evaluation. The full caller options carry through
        // to the user-facing executor pass downstream.
        let scope_options = standard_pattern_options();
        let compiled = self.with_doc_kind(
            self.compile_leaf(leaf, &scope_options, false)?,
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

    fn text_authority(&self, feature: &str) -> Result<&TextAuthorityShard, CoreError> {
        self.text_authority.as_ref().ok_or_else(|| CoreError::Typed {
            code: format!("{feature}_INDEX_MISSING"),
            message: format!(
                "lexical: {feature} execution requires a materialized text authority sidecar for this generation"
            ),
        })
    }

    fn repo_commit_recency_authority(&self) -> Result<&RepoCommitRecencyShard, CoreError> {
        self.repo_commit_recency.as_ref().ok_or_else(|| CoreError::Typed {
            code: "HISTORY_REPO_COMMIT_RECENCY_UNAVAILABLE".to_string(),
            message: "lexical: repo.has.commit.after execution requires materialized source-repo commit recency authority for this generation".to_string(),
        })
    }

    fn repo_meta_authority(&self) -> Result<&RepoMetaShard, CoreError> {
        self.repo_meta.as_ref().ok_or_else(|| CoreError::Typed {
            code: "REPO_META_UNAVAILABLE".to_string(),
            message: "lexical: repo.has.meta execution requires materialized source-repo repo metadata authority for this generation".to_string(),
        })
    }

    fn repo_topic_authority(&self) -> Result<&RepoTopicShard, CoreError> {
        self.repo_topic.as_ref().ok_or_else(|| CoreError::Typed {
            code: "REPO_TOPIC_UNAVAILABLE".to_string(),
            message: "lexical: repo.has.topic execution requires materialized source-repo repo topic authority for this generation".to_string(),
        })
    }

    fn repo_description_authority(&self) -> Result<&RepoDescriptionShard, CoreError> {
        self.repo_description.as_ref().ok_or_else(|| CoreError::Typed {
            code: "REPO_DESCRIPTION_UNAVAILABLE".to_string(),
            message: "lexical: repo.has.description execution requires materialized source-repo repo description authority for this generation".to_string(),
        })
    }

    fn file_ownership_authority(&self) -> Result<&FileOwnershipShard, CoreError> {
        self.file_ownership.as_ref().ok_or_else(|| CoreError::Typed {
            code: "FILE_OWNERSHIP_UNAVAILABLE".to_string(),
            message: "lexical: file.has.owner execution requires materialized source-repo file ownership authority for this generation".to_string(),
        })
    }

    fn file_contributor_authority(&self) -> Result<&FileContributorShard, CoreError> {
        self.file_contributor.as_ref().ok_or_else(|| CoreError::Typed {
            code: "FILE_CONTRIBUTOR_UNAVAILABLE".to_string(),
            message: "lexical: file.has.contributor execution requires materialized source-repo file contributor authority for this generation".to_string(),
        })
    }

    fn collect_matching_candidate_ids_for_raw_substring(
        &self,
        needle: &str,
        options: &LqOptions,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.text_authority("LEX_RAW_SUBSTRING")?;
        let folded = !Self::is_case_sensitive(options);
        let trigram_index = if folded {
            &authority.trigram_folded
        } else {
            &authority.trigram
        };
        let resolver = TextAuthorityResolver {
            shard: authority,
            folded,
        };
        let query_bytes = if folded {
            needle.to_ascii_lowercase().into_bytes()
        } else {
            needle.as_bytes().to_vec()
        };
        let verified_doc_ids = query_raw_substring(trigram_index, &query_bytes, &resolver)
            .map_err(|err| match err.code {
                TrigramErrorCode::RegexPrefilterUnusable => CoreError::Typed {
                    code: "LEX_RAW_SUBSTRING_TRIGRAM_INDEX_MISSING".to_string(),
                    message: format!("lexical: raw substring requires verify-only fallback: {err}"),
                },
                TrigramErrorCode::PlanLimitExceeded
                | TrigramErrorCode::InvalidGeneration
                | TrigramErrorCode::IndexDeserialize
                | TrigramErrorCode::IndexCorrupted => {
                    map_trigram_error("raw substring prefilter", &err)
                }
            })?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for doc_id in verified_doc_ids {
            let Some(doc) = authority.doc(doc_id.0) else {
                return Err(CoreError::Storage(format!(
                    "lexical: raw substring resolver missing doc {}",
                    doc_id.0
                )));
            };
            let _inserted: bool = out.insert(doc.candidate_id.clone());
        }
        Ok(out)
    }

    fn regex_source_for_options(source: &str, options: &LqOptions) -> String {
        if Self::is_case_sensitive(options) {
            return source.to_string();
        }
        format!("(?i){source}")
    }

    fn regex_timeout_budget_ms(options: &LqOptions) -> Option<u64> {
        options.timeout_ms
    }

    fn collect_matching_candidate_ids_for_regex(
        &self,
        source: &str,
        options: &LqOptions,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.text_authority("LEX_REGEX_TRIGRAM")?;
        let normalized_source = Self::regex_source_for_options(source, options);
        let plan = crate::regex::plan_regex(&normalized_source, options, &self.regex_policy)
            .map_err(map_regex_plan_error)?;
        let executor = RegexExecutor::compile(&normalized_source).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: regex plan/verify mismatch for {source:?}: {err}"
            ))
        })?;
        let folded = !Self::is_case_sensitive(options);
        let trigram_index = if folded {
            &authority.trigram_folded
        } else {
            &authority.trigram
        };
        let required_literals = if folded {
            plan.required_literals()
                .iter()
                .map(|literal| literal.to_ascii_lowercase())
                .collect::<Vec<_>>()
        } else {
            plan.required_literals().to_vec()
        };
        let prefiltered_doc_ids = match regex_prefilter(trigram_index, &required_literals) {
            Ok(doc_ids) => doc_ids,
            Err(err) if err.code == TrigramErrorCode::RegexPrefilterUnusable => authority
                .docs_by_id
                .keys()
                .copied()
                .map(TrigramDocId)
                .collect(),
            Err(err) => return Err(map_trigram_error("regex prefilter", &err)),
        };
        let resolver = TextAuthorityResolver {
            shard: authority,
            folded: false,
        };
        let budget_ms = Self::regex_timeout_budget_ms(options).unwrap_or(0);
        if options.timeout_ms == Some(0) && !prefiltered_doc_ids.is_empty() {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::QueryTimeout.as_code_str().to_string(),
                message:
                    "lexical: regex verify timed out before candidate verification began (budget 0ms)"
                        .to_string(),
            });
        }
        let verified_doc_ids = executor
            .execute_with_budget(&prefiltered_doc_ids, &resolver, budget_ms)
            .map_err(|err| match err.code {
                quanta_index_lq_regex::RegexErrorCode::QueryTimeout => CoreError::Typed {
                    code: LexicalErrorCode::QueryTimeout.as_code_str().to_string(),
                    message: format!("lexical: regex verify timed out: {err}"),
                },
                quanta_index_lq_regex::RegexErrorCode::ParseFail
                | quanta_index_lq_regex::RegexErrorCode::ForbiddenSyntax
                | quanta_index_lq_regex::RegexErrorCode::PlanLimitExceeded
                | quanta_index_lq_regex::RegexErrorCode::RegexPrefilterUnusable
                | quanta_index_lq_regex::RegexErrorCode::ExecutionInternal => CoreError::Typed {
                    code: format!("LEX_REGEX_{}", err.code.as_code_str()),
                    message: format!("lexical: regex verify failed: {err}"),
                },
            })?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for doc_id in verified_doc_ids {
            let Some(doc) = authority.doc(doc_id.0) else {
                return Err(CoreError::Storage(format!(
                    "lexical: regex resolver missing doc {}",
                    doc_id.0
                )));
            };
            let _inserted: bool = out.insert(doc.candidate_id.clone());
        }
        Ok(out)
    }

    fn collect_matching_candidate_ids_for_phrase(
        &self,
        text: &str,
        options: &LqOptions,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.text_authority("LEX_PHRASE_POSITIONS")?;
        let plan = plan_phrase(
            text,
            options,
            &PhrasePolicy::defaults(),
            PhraseField::Content,
        )
        .map_err(|err| CoreError::InvalidContract(format!("lexical: phrase plan: {err}")))?;
        let positions_index = if plan.case_sensitive {
            &authority.positions
        } else {
            &authority.positions_folded
        };
        let terms = plan.tokens.iter().map(String::as_str).collect::<Vec<_>>();
        let matches = query_phrase(positions_index, &terms)
            .map_err(|err| map_positions_error("phrase query", &err))?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for phrase_match in matches.matches {
            let Some(doc) = authority.doc(phrase_match.doc_id.0) else {
                return Err(CoreError::Storage(format!(
                    "lexical: phrase resolver missing doc {}",
                    phrase_match.doc_id.0
                )));
            };
            let _inserted: bool = out.insert(doc.candidate_id.clone());
        }
        Ok(out)
    }

    fn repo_has_file_path_query(
        &self,
        constraint: &RepoFileConstraint,
        options: &LqOptions,
    ) -> Result<Box<dyn Query>, CoreError> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> =
            Vec::with_capacity(constraint.matchers.len());
        for matcher in &constraint.matchers {
            let query = match matcher {
                RepoFileMatcher::Path(pattern) => {
                    self.regex_text_query(self.fields.repo_relative_path, pattern)?
                }
                RepoFileMatcher::Name(pattern) => {
                    self.regex_text_query(self.fields.file_name, pattern)?
                }
                RepoFileMatcher::Content(value) => {
                    // SGX-01: AND the content clause onto the SAME per-document
                    // BooleanQuery as the path/name/lang clauses, so a repo
                    // matches only when ONE file satisfies path AND content.
                    // Lower through content_leaf_from_scalar so `/regex/` content
                    // stays a regex; compile_leaf builds the chunk_text query.
                    let leaf = content_leaf_from_scalar(&ContentScalarArg::Keyword(value.clone()));
                    self.compile_leaf(&leaf, options, false)?
                }
                RepoFileMatcher::Language(value) => {
                    // normalize_language case-folds the value for the exact
                    // language-field match. The registry already rejects
                    // empty/whitespace lang values, so the `None` branch is
                    // unreachable on every path that builds a constraint through
                    // parse_repo_file_matchers; it is kept as a fail-closed guard
                    // (not a panic) against a future caller constructing
                    // RepoFileMatcher::Language directly.
                    let Some(normalized) = normalize_language(value) else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `repo.has.file` lang: argument cannot be empty (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    self.exact_text_query(self.fields.language, &normalized)
                }
            };
            clauses.push((Occur::Must, query));
        }
        Ok(self.with_doc_kind(Box::new(BooleanQuery::new(clauses)), TEXT_DOC_KIND))
    }

    fn collect_repo_ids_for_repo_has_file(
        &self,
        constraint: &RepoFileConstraint,
        options: &LqOptions,
    ) -> Result<BTreeSet<String>, CoreError> {
        let compiled = self.repo_has_file_path_query(constraint, options)?;
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting repo.has.file scope: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(BTreeSet::new());
        }
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(limit))
            .map_err(|err| {
                CoreError::Storage(format!("lexical: repo.has.file repo scope search: {err}"))
            })?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (_, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if let Some(repo_id) = stored_text(&doc, self.fields.repo_id) {
                let _inserted: bool = out.insert(repo_id);
            }
        }
        Ok(out)
    }

    fn canonicalize_predicate_call(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<Option<(String, Vec<LqPredicateArg>)>, CoreError> {
        canonicalize_predicate_call(name, args)
            .map(|call| call.map(|canonical| (canonical.name.to_string(), canonical.args)))
            .map_err(|_err| {
                unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` does not admit this alias argument shape (owner: {PREDICATE_OWNER})"
                ))
            })
    }

    fn content_predicate_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<ContentPredicateConstraint, CoreError> {
        parse_content_predicate_constraint(args).map_err(|err| match err {
            ContentPredicateArgError::MissingContentScalar => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` requires exactly one content scalar argument (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::MultipleContentScalars => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one content scalar argument (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::UnsupportedFilter => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one content scalar plus optional file:/path:/lang: scope filters (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::DuplicatePathScope => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one file:/path: scope filter (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::DuplicateLanguage => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one lang: scope filter (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::EmptyLanguageValue => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` lang: argument cannot be empty (owner: {PREDICATE_OWNER})"
            )),
        })
    }

    /// Validate `repo.has.content(...)` and lower it to its content leaf.
    fn repo_content_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<LqLeaf, CoreError> {
        let arg = parse_content_scalar_arg(args).map_err(|err| match err {
            ContentScalarArgError::WrongArity => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` requires exactly one content scalar argument (owner: {PREDICATE_OWNER})"
            )),
            ContentScalarArgError::UnsupportedArg => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports a keyword/phrase/raw-string/number content argument (owner: {PREDICATE_OWNER})"
            )),
        })?;
        Ok(content_leaf_from_scalar(&arg))
    }

    fn repo_commit_after_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<String, CoreError> {
        parse_timeref_scalar_arg(args)
            .map(|arg| arg.value)
            .map_err(|err| match err {
                TimerefScalarArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one timeref scalar argument (owner: {PREDICATE_OWNER})"
                )),
                TimerefScalarArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports one keyword/phrase/raw-string/number timeref argument (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    /// Compile a `repo.has.content` leaf into its repo-scope content query.
    ///
    /// Unlike `file.contains` path-discovery (which uses `standard_pattern_options`
    /// as a prelude), this collection IS the user-visible evaluation, so it
    /// preserves the caller's `case` / `patterntype` options.
    fn repo_has_content_query(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
    ) -> Result<Box<dyn Query>, CoreError> {
        Ok(self.with_doc_kind(self.compile_leaf(leaf, options, false)?, TEXT_DOC_KIND))
    }

    fn collect_repo_ids_for_repo_has_content(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
    ) -> Result<BTreeSet<String>, CoreError> {
        let compiled = self.repo_has_content_query(leaf, options)?;
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting repo.has.content scope: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(BTreeSet::new());
        }
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(limit))
            .map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: repo.has.content repo scope search: {err}"
                ))
            })?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (_, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if let Some(repo_id) = stored_text(&doc, self.fields.repo_id) {
                let _inserted: bool = out.insert(repo_id);
            }
        }
        Ok(out)
    }

    fn collect_repo_ids_for_repo_has_commit_after(
        &self,
        timeref: &str,
    ) -> Result<BTreeSet<String>, CoreError> {
        let Some(boundary_ms) = parse_search_timeref_ms(timeref) else {
            return Err(CoreError::Typed {
                code: "HISTORY_INVALID_TIMEREF".to_string(),
                message: format!(
                    "history: timeref `{timeref}` is not a valid RFC3339 timestamp, date-only value, duration, or supported human timeref"
                ),
            });
        };
        let authority = self.repo_commit_recency_authority()?;
        Ok(authority
            .latest_committer_time_ms_by_repo_id
            .iter()
            .filter(|&(_, latest_committer_time_ms)| *latest_committer_time_ms > boundary_ms)
            .map(|(repo_id, _)| repo_id.clone())
            .collect())
    }

    fn repo_meta_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<RepoMetaArg, CoreError> {
        parse_repo_meta_arg(args)
            .and_then(|arg| {
                let Some(key) = normalize_repo_meta_key(&arg.key) else {
                    return Err(RepoMetaArgError::EmptyKey);
                };
                Ok(RepoMetaArg {
                    key,
                    value: arg.value,
                })
            })
            .map_err(|err| match err {
                RepoMetaArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one metadata argument (owner: {PREDICATE_OWNER})"
                )),
                RepoMetaArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports `key:value`, bare `key`, or `tag:` existence shapes (owner: {PREDICATE_OWNER})"
                )),
                RepoMetaArgError::EmptyKey => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires a non-empty metadata key (owner: {PREDICATE_OWNER})"
                )),
                RepoMetaArgError::RegexUnsupported => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` does not support `/.../ ` regex key/value; the metadata authority is exact-string only (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    fn collect_repo_ids_for_repo_has_meta(
        &self,
        arg: &RepoMetaArg,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.repo_meta_authority()?;
        Ok(authority
            .meta_by_repo_id
            .iter()
            .filter(|(_, by_key)| {
                // SGX-03: `Some(v)` is exact key:value equality; `None` is genuine
                // key EXISTENCE (key present with any value). NOTE: this must NOT
                // be folded to `get(key) == value.as_ref()` — for `None` that
                // would test key ABSENCE, the exact inverse of existence.
                arg.value.as_ref().map_or_else(
                    || by_key.contains_key(&arg.key),
                    |value| by_key.get(&arg.key) == Some(value),
                )
            })
            .map(|(repo_id, _)| repo_id.clone())
            .collect())
    }

    fn repo_topic_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<RepoTopicArg, CoreError> {
        parse_repo_topic_arg(args)
            .and_then(|arg| {
                let Some(topic) = normalize_repo_topic_value(&arg.topic) else {
                    return Err(RepoTopicArgError::EmptyTopic);
                };
                Ok(RepoTopicArg { topic })
            })
            .map_err(|err| match err {
                RepoTopicArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one topic scalar argument (owner: {PREDICATE_OWNER})"
                )),
                RepoTopicArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports one keyword/phrase/raw-string topic argument (owner: {PREDICATE_OWNER})"
                )),
                RepoTopicArgError::EmptyTopic => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` topic cannot be empty (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    fn collect_repo_ids_for_repo_has_topic(
        &self,
        arg: &RepoTopicArg,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.repo_topic_authority()?;
        Ok(authority
            .topics_by_repo_id
            .iter()
            .filter(|(_, topics)| topics.contains(&arg.topic))
            .map(|(repo_id, _)| repo_id.clone())
            .collect())
    }

    fn repo_description_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<RepoDescriptionArg, CoreError> {
        parse_repo_description_arg(args).map_err(|err| match err {
            RepoDescriptionArgError::WrongArity => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` requires exactly one description pattern scalar argument (owner: {PREDICATE_OWNER})"
            )),
            RepoDescriptionArgError::UnsupportedArg => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one keyword/phrase/raw-string description pattern argument (owner: {PREDICATE_OWNER})"
            )),
            RepoDescriptionArgError::EmptyPattern => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` description pattern cannot be empty (owner: {PREDICATE_OWNER})"
            )),
        })
    }

    fn collect_repo_ids_for_repo_has_description(
        &self,
        arg: &RepoDescriptionArg,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.repo_description_authority()?;
        // SG `repo:has.description(<pattern>)` matches the repo description as a
        // regex. Compile the producer-published pattern once and verify it
        // against each repo's verbatim description. A malformed pattern is a
        // typed query error, not a silent empty result.
        //
        // We compile directly via `RegexExecutor::compile` (which applies the
        // upstream fixed NFA-state ceiling) rather than `crate::regex::plan_regex`:
        // `plan_regex` exists to drive the trigram pre-filter over the indexed
        // content corpus (`require_literal`, candidate caps), none of which apply
        // when we verify a handful of in-memory description strings. The RE2
        // engine is linear-time with no backtracking, so the bounded compile is
        // the only cost and the fixed ceiling is sufficient here.
        let executor = RegexExecutor::compile(&arg.pattern).map_err(|err| CoreError::Typed {
            code: format!("LEX_REGEX_{}", err.code.as_code_str()),
            message: format!(
                "lexical: repo.has.description pattern {:?} failed to compile: {err}",
                arg.pattern
            ),
        })?;
        Ok(authority
            .descriptions_by_repo_id
            .iter()
            .filter(|(_, description)| executor.verify(description.as_bytes()))
            .map(|(repo_id, _)| repo_id.clone())
            .collect())
    }

    fn file_owner_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<FileOwnerArg, CoreError> {
        parse_file_owner_arg(args)
            .and_then(|arg| match arg.owner {
                Some(owner) => {
                    let Some(owner) = normalize_owner_identity(&owner) else {
                        return Err(FileOwnerArgError::EmptyOwner);
                    };
                    Ok(FileOwnerArg { owner: Some(owner) })
                }
                None => Ok(arg),
            })
            .map_err(|err| match err {
                FileOwnerArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` supports zero args (`any owner`) or exactly one owner identity argument (owner: {PREDICATE_OWNER})"
                )),
                FileOwnerArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports zero args or one keyword/phrase/raw-string owner identity (owner: {PREDICATE_OWNER})"
                )),
                FileOwnerArgError::EmptyOwner => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` owner identity cannot be empty (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    fn collect_candidate_ids_for_file_has_owner(
        &self,
        arg: &FileOwnerArg,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.file_ownership_authority()?;
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting file ownership candidates: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(BTreeSet::new());
        }
        let hits = searcher
            .search(&AllQuery, &TopDocs::with_limit(limit))
            .map_err(|err| {
                CoreError::Storage(format!("lexical: file ownership authority scan: {err}"))
            })?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (_score, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if stored_text(&doc, self.fields.doc_kind).as_deref() != Some(TEXT_DOC_KIND) {
                continue;
            }
            let Some(source_repo_id) = stored_text(&doc, self.fields.repo_id) else {
                continue;
            };
            let Some(repo_relative_path) = stored_text(&doc, self.fields.repo_relative_path) else {
                continue;
            };
            let Some(candidate_id) = stored_text(&doc, self.fields.candidate_id) else {
                continue;
            };
            let Some(owners) = authority
                .owners_by_repo_id
                .get(&source_repo_id)
                .and_then(|by_path| by_path.get(&repo_relative_path))
            else {
                continue;
            };
            let matches = arg
                .owner
                .as_ref()
                .map_or(!owners.is_empty(), |owner| owners.contains(owner));
            if matches {
                let _inserted = out.insert(candidate_id);
            }
        }
        Ok(out)
    }

    fn file_contributor_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<FileContributorArg, CoreError> {
        parse_file_contributor_arg(args)
            .and_then(|arg| {
                let Some(contributor) = normalize_contributor_identity(&arg.contributor) else {
                    return Err(FileContributorArgError::EmptyContributor);
                };
                Ok(FileContributorArg { contributor })
            })
            .map_err(|err| match err {
                FileContributorArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one contributor identity argument (owner: {PREDICATE_OWNER})"
                )),
                FileContributorArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports one keyword/phrase/raw-string contributor identity (owner: {PREDICATE_OWNER})"
                )),
                FileContributorArgError::EmptyContributor => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` contributor identity cannot be empty (owner: {PREDICATE_OWNER})"
                )),
                FileContributorArgError::RegexUnsupported => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` does not support `/.../ ` regex contributor patterns; the contributor authority is exact-string only (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    fn collect_candidate_ids_for_file_has_contributor(
        &self,
        arg: &FileContributorArg,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.file_contributor_authority()?;
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting file contributor candidates: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(BTreeSet::new());
        }
        let hits = searcher
            .search(&AllQuery, &TopDocs::with_limit(limit))
            .map_err(|err| {
                CoreError::Storage(format!("lexical: file contributor authority scan: {err}"))
            })?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (_score, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if stored_text(&doc, self.fields.doc_kind).as_deref() != Some(TEXT_DOC_KIND) {
                continue;
            }
            let Some(source_repo_id) = stored_text(&doc, self.fields.repo_id) else {
                continue;
            };
            let Some(repo_relative_path) = stored_text(&doc, self.fields.repo_relative_path) else {
                continue;
            };
            let Some(candidate_id) = stored_text(&doc, self.fields.candidate_id) else {
                continue;
            };
            let Some(contributors) = authority
                .contributors_by_repo_id
                .get(&source_repo_id)
                .and_then(|by_path| by_path.get(&repo_relative_path))
            else {
                continue;
            };
            if contributors.contains(&arg.contributor) {
                let _inserted = out.insert(candidate_id);
            }
        }
        Ok(out)
    }

    fn predicate_content_leaf_from_constraint(
        &self,
        constraint: &ContentPredicateConstraint,
    ) -> LqLeaf {
        content_leaf_from_scalar(&constraint.content)
    }

    fn collect_matching_paths_for_content_scope(
        &self,
        constraint: &ContentPredicateConstraint,
    ) -> Result<Option<BTreeSet<String>>, CoreError> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if let Some(ContentPathScope { pattern, scope }) = constraint.path_scope.as_ref() {
            clauses.push((Occur::Must, self.compile_file_filter(pattern, *scope)?));
        }
        if let Some(language) = constraint.language.as_ref() {
            let Some(normalized) = normalize_language(language) else {
                return Err(CoreError::InvalidContract(
                    "lexical: scoped content predicate escaped with an empty lang value"
                        .to_string(),
                ));
            };
            clauses.push((
                Occur::Must,
                self.exact_text_query(self.fields.language, &normalized),
            ));
        }
        if clauses.is_empty() {
            return Ok(None);
        }
        let compiled = self.with_doc_kind(Box::new(BooleanQuery::new(clauses)), TEXT_DOC_KIND);
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting scoped content paths: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(Some(BTreeSet::new()));
        }
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(limit))
            .map_err(|err| {
                CoreError::Storage(format!("lexical: scoped content path search: {err}"))
            })?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (_, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if let Some(path) = stored_text(&doc, self.fields.repo_relative_path) {
                let _inserted: bool = out.insert(path);
            }
        }
        Ok(Some(out))
    }

    fn allowed_paths_for_content_predicate(
        &self,
        constraint: &ContentPredicateConstraint,
    ) -> Result<BTreeSet<String>, CoreError> {
        let content_leaf = self.predicate_content_leaf_from_constraint(constraint);
        let content_paths = self.collect_matching_paths_for_leaf(&content_leaf)?;
        let Some(scope_paths) = self.collect_matching_paths_for_content_scope(constraint)? else {
            return Ok(content_paths);
        };
        Ok(content_paths.intersection(&scope_paths).cloned().collect())
    }

    fn repo_has_file_constraint(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<RepoFileConstraint, CoreError> {
        parse_repo_file_matchers(args).map_err(|err| match err {
            RepoFileArgError::UnsupportedArg => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one scalar path argument or path:/name:/lang:/content: filter arguments (owner: {PREDICATE_OWNER})"
            )),
            RepoFileArgError::NoMatcher => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` requires either one scalar path argument or at least one path:/name:/lang:/content: argument (owner: {PREDICATE_OWNER})"
            )),
            RepoFileArgError::EmptyLanguageValue => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` lang: argument cannot be empty (owner: {PREDICATE_OWNER})"
            )),
            RepoFileArgError::EmptyContentValue => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` content: argument cannot be empty (owner: {PREDICATE_OWNER})"
            )),
        })
    }

    fn lower_predicate_for_boolean_scope(
        &self,
        name: &str,
        args: &[LqPredicateArg],
    ) -> Result<LqExpr, CoreError> {
        let Some((canonical_name, canonical_args)) =
            self.canonicalize_predicate_call(name, args)?
        else {
            return Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            )));
        };
        match kind_of(&canonical_name) {
            Some(PredicateKind::RepoFileGate) => {
                let _constraint =
                    self.repo_has_file_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoContentGate) => {
                let _leaf = self.repo_content_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoCommitRecencyGate) => {
                let _timeref =
                    self.repo_commit_after_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoMetaGate) => {
                let _arg = self.repo_meta_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoTopicGate) => {
                let _arg = self.repo_topic_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::RepoDescriptionGate) => {
                let _arg = self.repo_description_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::FileOwnerGate) => {
                let _arg = self.file_owner_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::FileContributorGate) => {
                let _arg = self.file_contributor_constraint(&canonical_name, &canonical_args)?;
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            Some(PredicateKind::ContentLeaf) => {
                let constraint =
                    self.content_predicate_constraint(&canonical_name, &canonical_args)?;
                if constraint.has_scopes() {
                    return Err(CoreError::Typed {
                        code: PREDICATE_SCOPED_BOOLEAN_UNSUPPORTED_CODE.to_string(),
                        message: format!(
                            "lexical: predicate leaf `{canonical_name}` with file:/path:/lang: scope is not executable inside OR/NOT boolean scope (owner: {PREDICATE_OWNER})"
                        ),
                    });
                }
                Ok(LqExpr::Leaf(
                    self.predicate_content_leaf_from_constraint(&constraint),
                ))
            }
            None => Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{canonical_name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            ))),
        }
    }

    fn lower_predicates_for_boolean_scope(&self, expr: &LqExpr) -> Result<LqExpr, CoreError> {
        match expr {
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
                self.lower_predicate_for_boolean_scope(name, args)
            }
            LqExpr::Empty | LqExpr::Leaf(_) => Ok(expr.clone()),
            LqExpr::All(parts) => {
                let mut out: Vec<LqExpr> = Vec::with_capacity(parts.len());
                for part in parts {
                    out.push(self.lower_predicates_for_boolean_scope(part)?);
                }
                Ok(collapse_exprs(out, true))
            }
            LqExpr::Any(parts) => {
                let mut out: Vec<LqExpr> = Vec::with_capacity(parts.len());
                for part in parts {
                    out.push(self.lower_predicates_for_boolean_scope(part)?);
                }
                Ok(collapse_exprs(out, false))
            }
            LqExpr::Not(inner) => Ok(LqExpr::Not(Box::new(
                self.lower_predicates_for_boolean_scope(inner)?,
            ))),
        }
    }

    fn extract_predicate_plan(
        &self,
        expr: &LqExpr,
    ) -> Result<
        (
            LqExpr,
            Vec<RepoScopeConstraint>,
            Vec<ContentPredicateConstraint>,
        ),
        CoreError,
    > {
        match expr {
            LqExpr::Empty => Ok((LqExpr::Empty, Vec::new(), Vec::new())),
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
                let Some((canonical_name, canonical_args)) =
                    self.canonicalize_predicate_call(name, args)?
                else {
                    return Err(unimplemented_predicate(format!(
                        "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                    )));
                };
                match kind_of(&canonical_name) {
                    Some(PredicateKind::RepoFileGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::File(self.repo_has_file_constraint(
                            &canonical_name,
                            &canonical_args,
                        )?)],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoContentGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::Content(
                            self.repo_content_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoCommitRecencyGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::CommitAfter(
                            self.repo_commit_after_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoMetaGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::Meta(
                            self.repo_meta_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoTopicGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::Topic(
                            self.repo_topic_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::RepoDescriptionGate) => Ok((
                        LqExpr::Empty,
                        vec![RepoScopeConstraint::Description(
                            self.repo_description_constraint(&canonical_name, &canonical_args)?,
                        )],
                        Vec::new(),
                    )),
                    Some(PredicateKind::FileOwnerGate) => {
                        let _arg = self.file_owner_constraint(&canonical_name, &canonical_args)?;
                        Ok((
                            LqExpr::Leaf(LqLeaf::Predicate {
                                name: canonical_name,
                                args: canonical_args,
                            }),
                            Vec::new(),
                            Vec::new(),
                        ))
                    }
                    Some(PredicateKind::FileContributorGate) => {
                        let _arg =
                            self.file_contributor_constraint(&canonical_name, &canonical_args)?;
                        Ok((
                            LqExpr::Leaf(LqLeaf::Predicate {
                                name: canonical_name,
                                args: canonical_args,
                            }),
                            Vec::new(),
                            Vec::new(),
                        ))
                    }
                    Some(PredicateKind::ContentLeaf) => Ok((
                        LqExpr::Empty,
                        Vec::new(),
                        vec![self.content_predicate_constraint(&canonical_name, &canonical_args)?],
                    )),
                    None => Err(unimplemented_predicate(format!(
                        "lexical: predicate leaf `{canonical_name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                    ))),
                }
            }
            LqExpr::Leaf(_) => Ok((expr.clone(), Vec::new(), Vec::new())),
            LqExpr::All(parts) => {
                let mut exprs: Vec<LqExpr> = Vec::new();
                let mut repo_predicates: Vec<RepoScopeConstraint> = Vec::new();
                let mut file_predicates: Vec<ContentPredicateConstraint> = Vec::new();
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
            LqExpr::Any(_) | LqExpr::Not(_) => Ok((
                self.lower_predicates_for_boolean_scope(expr)?,
                Vec::new(),
                Vec::new(),
            )),
        }
    }

    fn prepare_predicate_plan(&self, query: &LqQuery) -> Result<PreparedPredicatePlan, CoreError> {
        if let LqExpr::Leaf(LqLeaf::Predicate { name, args }) = &query.expr {
            let Some((canonical_name, canonical_args)) =
                self.canonicalize_predicate_call(name, args)?
            else {
                return Err(unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                )));
            };
            if matches!(kind_of(&canonical_name), Some(PredicateKind::ContentLeaf)) {
                let constraint =
                    self.content_predicate_constraint(&canonical_name, &canonical_args)?;
                let allowed_paths = self.collect_matching_paths_for_content_scope(&constraint)?;
                if allowed_paths.as_ref().is_some_and(BTreeSet::is_empty) {
                    return Ok(PreparedPredicatePlan {
                        expr: LqExpr::Empty,
                        allowed_paths: None,
                        allowed_repo_ids: None,
                        allowed_candidate_ids: None,
                        force_empty: true,
                    });
                }
                let lowered = self.predicate_content_leaf_from_constraint(&constraint);
                return Ok(PreparedPredicatePlan {
                    expr: LqExpr::Leaf(lowered),
                    allowed_paths,
                    allowed_repo_ids: None,
                    allowed_candidate_ids: None,
                    force_empty: false,
                });
            }
        }
        let (expr, repo_constraints, file_predicates) = self.extract_predicate_plan(&query.expr)?;
        let mut allowed_repo_ids: Option<BTreeSet<String>> = None;
        for constraint in &repo_constraints {
            let repo_ids = match constraint {
                RepoScopeConstraint::File(file) => {
                    self.collect_repo_ids_for_repo_has_file(file, &query.options)?
                }
                RepoScopeConstraint::Content(leaf) => {
                    self.collect_repo_ids_for_repo_has_content(leaf, &query.options)?
                }
                RepoScopeConstraint::CommitAfter(timeref) => {
                    self.collect_repo_ids_for_repo_has_commit_after(timeref)?
                }
                RepoScopeConstraint::Meta(arg) => self.collect_repo_ids_for_repo_has_meta(arg)?,
                RepoScopeConstraint::Topic(arg) => self.collect_repo_ids_for_repo_has_topic(arg)?,
                RepoScopeConstraint::Description(arg) => {
                    self.collect_repo_ids_for_repo_has_description(arg)?
                }
            };
            if repo_ids.is_empty() {
                return Ok(PreparedPredicatePlan {
                    expr: LqExpr::Empty,
                    allowed_paths: None,
                    allowed_repo_ids: None,
                    allowed_candidate_ids: None,
                    force_empty: true,
                });
            }
            allowed_repo_ids = Some(match allowed_repo_ids.take() {
                Some(existing) => existing.intersection(&repo_ids).cloned().collect(),
                None => repo_ids,
            });
        }
        if allowed_repo_ids.as_ref().is_some_and(BTreeSet::is_empty) {
            return Ok(PreparedPredicatePlan {
                expr: LqExpr::Empty,
                allowed_paths: None,
                allowed_repo_ids: None,
                allowed_candidate_ids: None,
                force_empty: true,
            });
        }
        let mut allowed_paths: Option<BTreeSet<String>> = None;
        for predicate in &file_predicates {
            let paths = self.allowed_paths_for_content_predicate(predicate)?;
            if paths.is_empty() {
                return Ok(PreparedPredicatePlan {
                    expr: LqExpr::Empty,
                    allowed_paths: None,
                    allowed_repo_ids: None,
                    allowed_candidate_ids: None,
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
                allowed_repo_ids: None,
                allowed_candidate_ids: None,
                force_empty: true,
            });
        }
        Ok(PreparedPredicatePlan {
            expr,
            allowed_paths,
            allowed_repo_ids,
            allowed_candidate_ids: None,
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
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. }
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
            LqType::File | LqType::Path | LqType::Repo => Ok(QueryDocKind::Text),
            LqType::Symbol => Ok(QueryDocKind::Symbol),
            // The planner pre-flight surfaces this as typed
            // `HISTORY_PRODUCER_UNAVAILABLE` before `search()` reaches the
            // doc-kind routing path. The defensive arm here preserves the
            // same typed code for callers that bypass the planner (today
            // there are none on the live rail).
            LqType::Commit | LqType::Diff => Err(CoreError::Typed {
                code: crate::filters::codes::HISTORY_PRODUCER_UNAVAILABLE.to_string(),
                message: format!(
                    "lexical: type filter `{}` targets a surface with no producer on the lexical rail",
                    kind.as_str()
                ),
            }),
        }
    }

    fn doc_kind_for_select(dim: LqSelect) -> QueryDocKind {
        match dim {
            // `select:repo` collapses text hits to one representative row per
            // repo. The current lexical rail opens exactly one repo/revision
            // generation at a time, so execution still runs against text docs
            // and the projection collapse happens after recall.
            LqSelect::File
            | LqSelect::FileOwners
            | LqSelect::Path
            | LqSelect::Content
            | LqSelect::ContentMatch
            | LqSelect::Repo => QueryDocKind::Text,
            LqSelect::Symbol => QueryDocKind::Symbol,
        }
    }

    fn projects_repo_surface(query: &LqQuery) -> bool {
        query.filters.iter().any(|filter| {
            matches!(
                filter,
                LqFilter::Select {
                    dim: LqSelect::Repo
                } | LqFilter::Type { kind: LqType::Repo }
            )
        })
    }

    fn projects_path_surface(query: &LqQuery) -> bool {
        query.filters.iter().any(|filter| {
            matches!(
                filter,
                LqFilter::Select {
                    dim: LqSelect::Path
                } | LqFilter::Type { kind: LqType::Path }
            )
        })
    }

    fn selects_file_projection(query: &LqQuery) -> bool {
        query.filters.iter().any(|filter| {
            matches!(
                filter,
                LqFilter::Select {
                    dim: LqSelect::File
                }
            )
        })
    }

    fn selects_file_owner_projection(query: &LqQuery) -> bool {
        query.filters.iter().any(|filter| {
            matches!(
                filter,
                LqFilter::Select {
                    dim: LqSelect::FileOwners
                }
            )
        })
    }

    fn collapse_repo_projection(
        query: &LqQuery,
        hits: Vec<LexicalCandidate>,
    ) -> Vec<LexicalCandidate> {
        if !Self::projects_repo_surface(query) {
            return hits;
        }
        let mut representatives: BTreeMap<RepoId, LexicalCandidate> = BTreeMap::new();
        for hit in hits {
            match representatives.entry(hit.repo_id.clone()) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    let _inserted: &mut LexicalCandidate = slot.insert(hit);
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    if Self::repo_projection_candidate_precedes(&hit, slot.get()) {
                        let _replaced: LexicalCandidate = slot.insert(hit);
                    }
                }
            }
        }
        representatives.into_values().collect()
    }

    fn repo_projection_candidate_precedes(
        candidate: &LexicalCandidate,
        current: &LexicalCandidate,
    ) -> bool {
        Self::candidate_precedes(candidate, current)
    }

    fn collapse_path_projection(
        query: &LqQuery,
        hits: Vec<LexicalCandidate>,
    ) -> Vec<LexicalCandidate> {
        if !Self::projects_path_surface(query) {
            return hits;
        }
        let mut representatives: BTreeMap<String, LexicalCandidate> = BTreeMap::new();
        for hit in hits {
            let key = hit.repo_relative_path.as_str().to_string();
            match representatives.entry(key) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    let _inserted: &mut LexicalCandidate = slot.insert(hit);
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    if Self::candidate_precedes(&hit, slot.get()) {
                        let _replaced: LexicalCandidate = slot.insert(hit);
                    }
                }
            }
        }
        let mut collapsed: Vec<LexicalCandidate> = representatives.into_values().collect();
        collapsed.sort_by(|left, right| {
            if Self::candidate_precedes(left, right) {
                std::cmp::Ordering::Less
            } else if Self::candidate_precedes(right, left) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        collapsed
    }

    fn collapse_file_projection(
        query: &LqQuery,
        hits: Vec<LexicalCandidate>,
    ) -> Vec<LexicalCandidate> {
        if !Self::selects_file_projection(query) {
            return hits;
        }
        let mut representatives: BTreeMap<String, LexicalCandidate> = BTreeMap::new();
        for hit in hits {
            let key = hit.repo_relative_path.as_str().to_string();
            match representatives.entry(key) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    let _inserted: &mut LexicalCandidate = slot.insert(hit);
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    if Self::candidate_precedes(&hit, slot.get()) {
                        let _replaced: LexicalCandidate = slot.insert(hit);
                    }
                }
            }
        }
        let mut collapsed: Vec<LexicalCandidate> = representatives.into_values().collect();
        collapsed.sort_by(|left, right| {
            if Self::candidate_precedes(left, right) {
                std::cmp::Ordering::Less
            } else if Self::candidate_precedes(right, left) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        collapsed
    }

    fn collapse_select_projection(
        query: &LqQuery,
        hits: Vec<LexicalCandidate>,
    ) -> Vec<LexicalCandidate> {
        let path_collapsed = Self::collapse_path_projection(query, hits);
        let file_collapsed = if Self::selects_file_owner_projection(query) {
            Self::collapse_file_projection(
                &LqQuery {
                    filters: query
                        .filters
                        .iter()
                        .cloned()
                        .map(|filter| {
                            if matches!(
                                &filter,
                                LqFilter::Select {
                                    dim: LqSelect::FileOwners
                                }
                            ) {
                                LqFilter::Select {
                                    dim: LqSelect::File,
                                }
                            } else {
                                filter
                            }
                        })
                        .collect(),
                    ..query.clone()
                },
                path_collapsed,
            )
        } else {
            Self::collapse_file_projection(query, path_collapsed)
        };
        Self::collapse_repo_projection(query, file_collapsed)
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
                    let next = Self::doc_kind_for_select(*dim);
                    doc_kind = Self::merge_doc_kind(doc_kind, next, "select")?;
                }
                LqFilter::Rev { spec } => {
                    // Planner pre-flight surfaces this as
                    // `LEX_FILTER_REV_UNAVAILABLE` before reaching here on
                    // the live `search` path; this arm preserves the same
                    // typed code as a defense-in-depth for any future caller
                    // that bypasses the planner.
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::REV_UNAVAILABLE.to_string(),
                        message: if is_rev_at_time_spec(spec) {
                            "lexical: rev:at.time(...) requires revision-selection and pin rebinding before lexical execution".to_string()
                        } else {
                            "lexical: rev filter requires history producer".to_string()
                        },
                    });
                }
                LqFilter::Author { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::AUTHOR_UNAVAILABLE.to_string(),
                        message:
                            "lexical: author filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Committer { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::COMMITTER_UNAVAILABLE.to_string(),
                        message:
                            "lexical: committer filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Message { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::MESSAGE_UNAVAILABLE.to_string(),
                        message:
                            "lexical: message filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Dirty { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::DIRTY_UNAVAILABLE.to_string(),
                        message:
                            "lexical: dirty filter is not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Changed { .. }
                | LqFilter::Stale { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::RUNTIME_CATALOG_UNAVAILABLE.to_string(),
                        message:
                            "lexical: runtime catalog filters are not executable on the current adapter set"
                                .to_string(),
                    });
                }
                LqFilter::Before { .. }
                | LqFilter::After { .. }
                | LqFilter::Since { .. }
                | LqFilter::Until { .. }
                | LqFilter::DiffAdded { .. }
                | LqFilter::DiffRemoved { .. }
                | LqFilter::DiffTouched { .. } => {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::HISTORY_PRODUCER_UNAVAILABLE.to_string(),
                        message: "lexical: history date/diff filters require history producer"
                            .to_string(),
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
        options: &LqOptions,
        include_path_terms: bool,
    ) -> Result<Box<dyn Query>, CoreError> {
        match expr {
            LqExpr::Empty => Ok(Box::new(AllQuery)),
            LqExpr::Leaf(leaf) => self.compile_leaf(leaf, options, include_path_terms),
            LqExpr::All(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((Occur::Must, self.compile_expr(part, options, false)?));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Any(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((Occur::Should, self.compile_expr(part, options, false)?));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Not(inner) => {
                let inner_q = self.compile_expr(inner, options, false)?;
                let clauses: Vec<(Occur, Box<dyn Query>)> =
                    vec![(Occur::Must, Box::new(AllQuery)), (Occur::MustNot, inner_q)];
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
        }
    }

    /// Compile a content-side regex leaf via the LXE-04 planner pipeline.
    ///
    /// All regex-shaped content leaves route here — both the explicit
    /// `LqLeaf::Regex(_)` AST shape AND `LqLeaf::Keyword`/`LqLeaf::RawString`
    /// leaves carrying `LqOptions::pattern_type = LqPatternType::Regexp`.
    /// Routing both shapes through one body keeps the dialect filter,
    /// trigram-missing threshold, and typed `LEX_REGEX_*` codes identical
    /// across surface kinds — no second path bypasses the planner.
    ///
    /// Pipeline:
    /// 1. plan the regex with `crate::regex::plan_regex` using the caller's
    ///    real [`LqOptions`] and the adapter-injected
    ///    [`RegexPolicy`]. Typed regex failures (lookbehind, possessive,
    ///    pattern budget) surface as `CoreError::Typed { code: "LEX_REGEX_*",
    ///    .. }` with a stable code per dialect-rejection kind.
    /// 2. prefilter the materialized trigram sidecar through
    ///    `regex_prefilter`, falling back to whole-corpus exact verify only
    ///    when the regex exposes no mandatory literals.
    /// 3. exact-verify every prefiltered authority doc via
    ///    `quanta-index-lq-regex::RegexExecutor`, then lower the verified
    ///    ids to a candidate restriction query over the active generation.
    ///
    /// Vendor tokens (`RegexExecutor`, Tantivy scan) are kept inside this
    /// method; callers see only typed `CoreError`s and `Box<dyn Query>`.
    fn compile_regex_content_leaf(
        &self,
        source: &str,
        options: &LqOptions,
    ) -> Result<Box<dyn Query>, CoreError> {
        let candidate_ids = self.collect_matching_candidate_ids_for_regex(source, options)?;
        if candidate_ids.is_empty() {
            return Ok(self.match_none_query());
        }
        Ok(self.candidate_restriction_query(&candidate_ids))
    }

    fn compile_leaf(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
        include_path_terms: bool,
    ) -> Result<Box<dyn Query>, CoreError> {
        let parser = self.query_parser(options, include_path_terms);
        let query_text = match leaf {
            LqLeaf::Keyword(text) | LqLeaf::RawString(text) => {
                // Both AST shapes route through the planner-gated regex
                // pipeline when the caller's options pin
                // `LqPatternType::Regexp`; bypassing this would skip the
                // dialect filter, the trigram-missing threshold, and the
                // typed `LEX_REGEX_*` error codes.
                if options.pattern_type == LqPatternType::Regexp {
                    return self.compile_regex_content_leaf(text, options);
                }
                if matches!(leaf, LqLeaf::RawString(_)) {
                    let candidate_ids =
                        self.collect_matching_candidate_ids_for_raw_substring(text, options)?;
                    if candidate_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.candidate_restriction_query(&candidate_ids));
                }
                text.clone()
            }
            LqLeaf::Phrase(text) => {
                let candidate_ids =
                    self.collect_matching_candidate_ids_for_phrase(text, options)?;
                if candidate_ids.is_empty() {
                    return Ok(self.match_none_query());
                }
                return Ok(self.candidate_restriction_query(&candidate_ids));
            }
            LqLeaf::Regex(text) => {
                return self.compile_regex_content_leaf(text, options);
            }
            LqLeaf::StructuralBlock(_) => {
                return Err(CoreError::Typed {
                    code: "STR_PRODUCER_PARSE_TREE_UNAVAILABLE".to_string(),
                    message:
                        "lexical: structural leaf cannot compile without producer parse-tree ops"
                            .to_string(),
                });
            }
            LqLeaf::Predicate { name, args } => match kind_of(name) {
                Some(PredicateKind::RepoFileGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let constraint =
                        self.repo_has_file_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_file(&constraint, options)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.repo_id_restriction_query(&repo_ids));
                }
                Some(PredicateKind::RepoContentGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let leaf = self.repo_content_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_content(&leaf, options)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.repo_id_restriction_query(&repo_ids));
                }
                Some(PredicateKind::RepoCommitRecencyGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let timeref =
                        self.repo_commit_after_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_commit_after(&timeref)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.repo_id_restriction_query(&repo_ids));
                }
                Some(PredicateKind::RepoMetaGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.repo_meta_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_meta(&arg)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.repo_id_restriction_query(&repo_ids));
                }
                Some(PredicateKind::FileOwnerGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.file_owner_constraint(&canonical_name, &canonical_args)?;
                    let candidate_ids = self.collect_candidate_ids_for_file_has_owner(&arg)?;
                    if candidate_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.candidate_restriction_query(&candidate_ids));
                }
                Some(PredicateKind::FileContributorGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.file_contributor_constraint(&canonical_name, &canonical_args)?;
                    let candidate_ids =
                        self.collect_candidate_ids_for_file_has_contributor(&arg)?;
                    if candidate_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.candidate_restriction_query(&candidate_ids));
                }
                Some(PredicateKind::RepoTopicGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.repo_topic_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_topic(&arg)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.repo_id_restriction_query(&repo_ids));
                }
                Some(PredicateKind::RepoDescriptionGate) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let arg = self.repo_description_constraint(&canonical_name, &canonical_args)?;
                    let repo_ids = self.collect_repo_ids_for_repo_has_description(&arg)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.repo_id_restriction_query(&repo_ids));
                }
                Some(PredicateKind::ContentLeaf) => {
                    let Some((canonical_name, canonical_args)) =
                        self.canonicalize_predicate_call(name, args)?
                    else {
                        return Err(unimplemented_predicate(format!(
                            "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                        )));
                    };
                    let constraint =
                        self.content_predicate_constraint(&canonical_name, &canonical_args)?;
                    if constraint.has_scopes() {
                        return Err(CoreError::InvalidContract(format!(
                            "lexical: scoped content predicate `{canonical_name}` must be prepared before compile_leaf"
                        )));
                    }
                    let lowered = self.predicate_content_leaf_from_constraint(&constraint);
                    return self.compile_leaf(&lowered, options, false);
                }
                None => {
                    return Err(unimplemented_predicate(format!(
                        "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                    )));
                }
            },
        };
        parser
            .parse_query(&query_text)
            .map_err(|err| CoreError::InvalidContract(format!("lexical: parse: {err}")))
    }

    fn compile_filter(
        &self,
        filter: &LqFilter,
        options: &LqOptions,
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
            LqFilter::Content { leaf } => Ok(Some(self.compile_leaf(leaf, options, false)?)),
            LqFilter::Lang { id } => {
                let Some(language) = normalize_language(id.as_str()) else {
                    return Err(CoreError::InvalidContract(
                        "lexical: lang filter value cannot be empty".to_string(),
                    ));
                };
                Ok(Some(self.exact_text_query(self.fields.language, &language)))
            }
            LqFilter::Rev { spec } => Err(CoreError::Typed {
                code: crate::filters::codes::REV_UNAVAILABLE.to_string(),
                message: if is_rev_at_time_spec(spec) {
                    "lexical: rev:at.time(...) requires revision-selection and pin rebinding before lexical execution".to_string()
                } else {
                    "lexical: rev filter requires history producer".to_string()
                },
            }),
            LqFilter::Author { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::AUTHOR_UNAVAILABLE.to_string(),
                message: "lexical: author filter is not executable on the current adapter set"
                    .to_string(),
            }),
            LqFilter::Committer { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::COMMITTER_UNAVAILABLE.to_string(),
                message: "lexical: committer filter is not executable on the current adapter set"
                    .to_string(),
            }),
            LqFilter::Message { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::MESSAGE_UNAVAILABLE.to_string(),
                message: "lexical: message filter is not executable on the current adapter set"
                    .to_string(),
            }),
            LqFilter::Dirty { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::DIRTY_UNAVAILABLE.to_string(),
                message: "lexical: dirty filter is not executable on the current adapter set"
                    .to_string(),
            }),
            LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::RUNTIME_CATALOG_UNAVAILABLE.to_string(),
                message:
                    "lexical: runtime catalog filters are not executable on the current adapter set"
                        .to_string(),
            }),
            LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. } => Err(CoreError::Typed {
                code: crate::filters::codes::HISTORY_PRODUCER_UNAVAILABLE.to_string(),
                message: "lexical: history date/diff filters require history producer".to_string(),
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

    fn compile_query_from_prepared(
        &self,
        query: &LqQuery,
        prepared: &PreparedPredicatePlan,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        if prepared.force_empty {
            return Ok(None);
        }
        let include_path_terms = Self::enables_path_term_surface(&prepared.expr, &query.options);
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if !matches!(prepared.expr, LqExpr::Empty) {
            clauses.push((
                Occur::Must,
                self.compile_expr(&prepared.expr, &query.options, include_path_terms)?,
            ));
        }
        for filter in &query.filters {
            if let Some(compiled_filter) = self.compile_filter(filter, &query.options)? {
                clauses.push((Occur::Must, compiled_filter));
            }
        }
        if let Some(paths) = prepared.allowed_paths.as_ref() {
            clauses.push((Occur::Must, self.path_restriction_query(paths)));
        }
        if let Some(repo_ids) = prepared.allowed_repo_ids.as_ref() {
            clauses.push((Occur::Must, self.repo_id_restriction_query(repo_ids)));
        }
        if let Some(candidate_ids) = prepared.allowed_candidate_ids.as_ref() {
            clauses.push((Occur::Must, self.candidate_restriction_query(candidate_ids)));
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

    fn prepare_executable_query(
        &self,
        query: &LqQuery,
        default_doc_kind: QueryDocKind,
    ) -> Result<Option<PreparedExecutableQuery>, CoreError> {
        let (prepared_query, doc_kind) =
            self.prepare_query_for_doc_kind(query, default_doc_kind)?;
        let predicate_plan = self.prepare_predicate_plan(&prepared_query)?;
        if predicate_plan.force_empty {
            return Ok(None);
        }
        Ok(Some(PreparedExecutableQuery {
            query: prepared_query,
            predicate_plan,
            doc_kind,
        }))
    }

    fn document_to_candidate(
        &self,
        doc: &TantivyDocument,
        score: f32,
    ) -> Result<LexicalCandidate, CoreError> {
        let candidate_id = stored_text(doc, self.fields.candidate_id).ok_or_else(|| {
            CoreError::Storage("lexical: stored doc missing candidate_id field".to_string())
        })?;
        let snippet = stored_text(doc, self.fields.snippet)
            .or_else(|| stored_text(doc, self.fields.chunk_text))
            .unwrap_or_default();
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

    fn document_to_symbol_candidate(
        &self,
        doc: &TantivyDocument,
        score: f32,
    ) -> Result<SymbolCandidate, CoreError> {
        let candidate = self.document_to_candidate(doc, score)?;
        let symbol_kind = stored_text(doc, self.fields.symbol_kind)
            .ok_or_else(|| {
                CoreError::Storage(
                    "lexical: stored symbol doc missing symbol_kind field".to_string(),
                )
            })
            .and_then(|raw| {
                SymbolKindCode::new(raw).map_err(|err| {
                    CoreError::Storage(format!("lexical: invalid stored symbol_kind: {err}"))
                })
            })?;
        let symbol_kind_family = match stored_text(doc, self.fields.symbol_kind_family) {
            Some(raw) => Some(
                SymbolKindFamily::from_code_str(raw.as_str()).ok_or_else(|| {
                    CoreError::Storage(format!(
                        "lexical: invalid stored symbol_kind_family `{raw}`"
                    ))
                })?,
            ),
            None => None,
        };
        Ok(SymbolCandidate {
            candidate_id: candidate.candidate_id,
            repo_id: candidate.repo_id,
            revision_id: candidate.revision_id,
            manifest_generation: candidate.manifest_generation,
            repo_relative_path: candidate.repo_relative_path,
            start_line: candidate.start_line,
            end_line: candidate.end_line,
            score,
            snippet: candidate.snippet,
            symbol_kind,
            symbol_kind_family,
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
/// repo metadata from the bundle payload, because the live
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
/// to typed failures (e.g. `count:0` → `LEX_FILTER_INVALID_COUNT`,
/// unsupported NOT/OR/filter combos → `LEX_PLANNER_UNSUPPORTED_*`).
///
/// Ordering rule for `typed_unavailable`: the planner records typed-
/// unavailable filters in the order they appeared in the input
/// `LqQuery::filters`. The executor surfaces the **first** such entry that
/// is not suppressed by adapter-side producer state — this gives
/// operator-facing diagnostics a deterministic single cause rather than a
/// multi-line laundry list.
///
/// Planner [`Unimplemented`](crate::planner::LexicalPlannerError::Unimplemented)
/// shapes surface as `CoreError::NotImplemented` carrying the owning
/// follow-up ticket. The planner is now the single authority for these IR
/// shapes — there is no silent delegation to a legacy executor.
fn planner_preflight(query: &LqQuery, has_repo_metadata: bool) -> Result<(), CoreError> {
    let plan = match crate::planner::LexicalPlanner::plan(query) {
        Ok(plan) => plan,
        Err(err) => return Err(map_planner_error(&err)),
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
/// Every planner error variant surfaces as a typed `CoreError` so the search
/// path never silently runs an unplanned query and never delegates to a
/// legacy executor.
///
/// * Filter-plan failures (`count:0`, conflicting surfaces, unsupported
///   filter combos) lower to `LEX_FILTER_*` typed codes.
/// * Unsupported IR-shape arms (`UnsupportedNotScope`, `UnsupportedOrScope`,
///   `UnsupportedFilterCombo`) lower to `LEX_PLANNER_UNSUPPORTED_*` typed
///   codes — these are stable wire codes for IR shapes the planner has
///   chosen not to lower.
/// * `Unimplemented { owner_ticket, .. }` surfaces as
///   `CoreError::NotImplemented` carrying the owning ticket id, so callers
///   can attribute the gap to a concrete follow-up.
/// * Leaf-planner failures (`RegexPlan`, `TrigramPlan`, `PhrasePlan`,
///   `SymbolPlan`) lower to `InvalidContract` because their dedicated
///   typed-error mappers (`map_regex_plan_error`, etc.) own the leaf-side
///   surfacing on the live execution path; the planner pre-flight should
///   never reach those arms in practice.
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
        LexicalPlannerError::UnsupportedNotScope => CoreError::Typed {
            code: "LEX_PLANNER_UNSUPPORTED_NOT_SCOPE".to_string(),
            message: "lexical: planner does not yet lower the NOT scope shape".to_string(),
        },
        LexicalPlannerError::UnsupportedOrScope => CoreError::Typed {
            code: "LEX_PLANNER_UNSUPPORTED_OR_SCOPE".to_string(),
            message: "lexical: planner does not yet lower the OR scope shape".to_string(),
        },
        LexicalPlannerError::UnsupportedFilterCombo => CoreError::Typed {
            code: "LEX_PLANNER_UNSUPPORTED_FILTER_COMBO".to_string(),
            message: "lexical: planner does not yet lower this filter combination".to_string(),
        },
        LexicalPlannerError::Unimplemented { node, owner_ticket } => CoreError::NotImplemented(
            format!("lex planner: IR node '{node}' is unimplemented (owner: {owner_ticket})"),
        ),
        // Leaf-planner errors reaching this site would mean the pre-flight
        // disagreed with the live executor's leaf-side mapping. They are
        // never produced on the current pipeline (leaves compile during
        // executor lowering, not during pre-flight planning); the arm is
        // kept for completeness and lowers to `InvalidContract` so the
        // mismatch is visible rather than swallowed.
        LexicalPlannerError::RegexPlan(_)
        | LexicalPlannerError::TrigramPlan(_)
        | LexicalPlannerError::PhrasePlan(_)
        | LexicalPlannerError::SymbolPlan(_) => {
            CoreError::InvalidContract(format!("lexical: planner: {err}"))
        }
    }
}

impl LexicalSearcher for TantivySearcher {
    fn search(&self, query: &LqQuery, top_k: u32) -> Result<Vec<LexicalCandidate>, CoreError> {
        // Validation ordering: `LexicalPolicy::validate_query` runs FIRST so
        // core-policy contract violations (empty-query rejection, structural
        // fail-closed, top-level rev/type:commit gating) surface their own
        // typed errors before the lexical planner's pre-flight runs. The
        // planner is then the single authority for typed-unavailable
        // surfacing on filters/IR shapes that pass the core policy —
        // overlapping responsibility is gone.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query(&effective_query)?;
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Text)?
        else {
            return Ok(Vec::new());
        };
        let mut planner_query = prepared_query.query.clone();
        planner_query.expr = prepared_query.predicate_plan.expr.clone();
        planner_preflight(&planner_query, self.repo_metadata.is_some())?;
        if !self.repo_filters_allow(&effective_query)? {
            return Ok(Vec::new());
        }
        let Some(base) = self
            .compile_query_from_prepared(&prepared_query.query, &prepared_query.predicate_plan)?
        else {
            return Ok(Vec::new());
        };
        let compiled = self.with_doc_kind(base, prepared_query.doc_kind.as_str());
        let requested = usize::try_from(top_k)
            .map_err(|err| CoreError::InvalidContract(format!("lexical: top_k: {err}")))?;
        let limit = self.effective_limit(&effective_query, requested);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let collect_limit = self.collect_limit(&effective_query, requested, limit);
        let searcher = self.reader.searcher();
        let mut hits = searcher
            .search(&*compiled, &TopDocs::with_limit(collect_limit))
            .map_err(|err| CoreError::Storage(format!("lexical: search: {err}")))?;
        let full_recall_limit = self.full_recall_limit(requested, limit);
        if collect_limit < full_recall_limit && Self::boundary_tie_detected(&hits, limit) {
            hits = searcher
                .search(&*compiled, &TopDocs::with_limit(full_recall_limit))
                .map_err(|err| CoreError::Storage(format!("lexical: search full recall: {err}")))?;
        }
        let mut out: Vec<LexicalCandidate> = Vec::with_capacity(hits.len());
        for (score, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_candidate(&doc, score)?);
        }
        let projected = Self::collapse_select_projection(&effective_query, out);
        Ok(self.stabilize_and_cap_hits(&effective_query, projected, limit))
    }

    fn project_file_owners(
        &self,
        candidates: &[LexicalCandidate],
    ) -> Result<Vec<FileOwnerProjectionRow>, CoreError> {
        let authority = self.file_ownership_authority()?;
        let searcher = self.reader.searcher();
        let mut rows: Vec<FileOwnerProjectionRow> = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let source_repo_hit = searcher
                .search(
                    &TermQuery::new(
                        Term::from_field_text(
                            self.fields.candidate_id,
                            candidate.candidate_id.as_str(),
                        ),
                        IndexRecordOption::Basic,
                    ),
                    &TopDocs::with_limit(1),
                )
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "lexical: file owner projection candidate lookup `{}`: {err}",
                        candidate.candidate_id
                    ))
                })?
                .into_iter()
                .next();
            // A storage error fetching the matched doc propagates (fail-closed);
            // a missing repo_id field falls back to the candidate's own repo_id,
            // which is the authoritative value the candidate already carries.
            let source_repo_id = match source_repo_hit {
                Some((_score, doc_address)) => {
                    let doc = searcher
                        .doc::<TantivyDocument>(doc_address)
                        .map_err(|err| {
                            CoreError::Storage(format!(
                                "lexical: file owner projection doc fetch `{}`: {err}",
                                candidate.candidate_id
                            ))
                        })?;
                    stored_text(&doc, self.fields.repo_id)
                        .unwrap_or_else(|| candidate.repo_id.as_str().to_string())
                }
                None => candidate.repo_id.as_str().to_string(),
            };
            let owners = authority
                .owners_by_repo_id
                .get(&source_repo_id)
                .and_then(|by_path| by_path.get(candidate.repo_relative_path.as_str()))
                .map(|set| set.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            rows.push(FileOwnerProjectionRow {
                candidate_id: candidate.candidate_id.clone(),
                repo_id: RepoId::new(source_repo_id),
                revision_id: candidate.revision_id.clone(),
                manifest_generation: candidate.manifest_generation,
                repo_relative_path: candidate.repo_relative_path.clone(),
                owners,
            });
        }
        Ok(rows)
    }

    fn search_symbols(
        &self,
        query: &LqQuery,
        top_k: u32,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query(&effective_query)?;
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Symbol)?
        else {
            return Ok(Vec::new());
        };
        let mut planner_query = prepared_query.query.clone();
        planner_query.expr = prepared_query.predicate_plan.expr.clone();
        planner_preflight(&planner_query, self.repo_metadata.is_some())?;
        if !self.repo_filters_allow(&effective_query)? {
            return Ok(Vec::new());
        }
        let Some(base) = self
            .compile_query_from_prepared(&prepared_query.query, &prepared_query.predicate_plan)?
        else {
            return Ok(Vec::new());
        };
        let compiled = self.with_doc_kind(base, prepared_query.doc_kind.as_str());
        let requested = usize::try_from(top_k)
            .map_err(|err| CoreError::InvalidContract(format!("lexical: top_k: {err}")))?;
        let limit = self.effective_limit(&effective_query, requested);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let collect_limit = self.collect_limit(&effective_query, requested, limit);
        let searcher = self.reader.searcher();
        let mut hits = searcher
            .search(&*compiled, &TopDocs::with_limit(collect_limit))
            .map_err(|err| CoreError::Storage(format!("lexical: symbol search: {err}")))?;
        let full_recall_limit = self.full_recall_limit(requested, limit);
        if collect_limit < full_recall_limit && Self::boundary_tie_detected(&hits, limit) {
            hits = searcher
                .search(&*compiled, &TopDocs::with_limit(full_recall_limit))
                .map_err(|err| {
                    CoreError::Storage(format!("lexical: symbol search full recall: {err}"))
                })?;
        }
        let mut out: Vec<SymbolCandidate> = Vec::with_capacity(hits.len());
        for (score, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_symbol_candidate(&doc, score)?);
        }
        Ok(Self::stabilize_and_cap_symbol_hits(out, limit))
    }

    fn search_all(&self, query: &LqQuery) -> Result<Vec<LexicalCandidate>, CoreError> {
        LexicalPolicy::validate_query(query)?;
        let Some(prepared_query) = self.prepare_executable_query(query, QueryDocKind::Text)? else {
            return Ok(Vec::new());
        };
        let mut planner_query = prepared_query.query.clone();
        planner_query.expr = prepared_query.predicate_plan.expr.clone();
        planner_preflight(&planner_query, self.repo_metadata.is_some())?;
        if !self.repo_filters_allow(query)? {
            return Ok(Vec::new());
        }
        let Some(base) = self
            .compile_query_from_prepared(&prepared_query.query, &prepared_query.predicate_plan)?
        else {
            return Ok(Vec::new());
        };
        let compiled = self.with_doc_kind(base, prepared_query.doc_kind.as_str());
        let searcher = self.reader.searcher();
        let requested = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while materializing scope: {err}"
            ))
        })?;
        let limit = self.effective_limit(query, requested);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let collect_limit = self.collect_limit(query, requested, limit);
        let hits = searcher
            .search(&*compiled, &TopDocs::with_limit(collect_limit))
            .map_err(|err| CoreError::Storage(format!("lexical: search_all: {err}")))?;
        let mut out: Vec<LexicalCandidate> = Vec::with_capacity(hits.len());
        for (score, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_candidate(&doc, score)?);
        }
        Ok(Self::collapse_repo_projection(
            query,
            self.stabilize_and_cap_hits(query, out, limit),
        ))
    }
}
