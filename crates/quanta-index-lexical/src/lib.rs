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
//!   `{state_root}/generation-v1-{sha256(repo, revision)}/g{generation}/`
//!
//! Beside the Tantivy index, a generation carries its text authority under
//! `text-authority/`: a bounded manifest plus immutable shard files, each
//! shard a fixed doc-id range's slice of the doc table, the byte-trigram
//! postings and the token positions (see the crate-private
//! `text_authority` module). Every text document stores its text-authority
//! doc id in the index, so a delta names exactly the documents it retires
//! and rewrites only the shards it touches; every other shard is the base
//! generation's file, hard-linked.
//!
//! Beside the index and the text authority, a generation carries the
//! repo-metadata overlays (the crate-private `sealed_generation::overlay`
//! families): whole-snapshot files the aux ingest routes publish before the
//! seal. The seal writes a manifest that names every file a query opens —
//! the Tantivy commit and the segment files it references, the
//! text-authority tree, the overlays — with length and digest, and nothing
//! may land in a sealed generation afterwards: an index-mutating op and an
//! overlay publish alike are refused typed. The activation validator and
//! the query open walk that manifest through one function, so activation
//! can only admit a generation a query can open (QI-BB-030).
//!
//! The adapter caches one `IndexWriter` per generation to amortize the
//! per-commit cost across many ops, and one `IndexReader` per opened
//! generation.
//!
//! Text semantics — NFC, Unicode case folding, token boundaries, and the
//! split between token surfaces (keyword, phrase) and byte surfaces (raw
//! string, regex) — are defined once in the crate-private `normalize`
//! module and consumed by the index analyzer, every text-authority shard,
//! and the `index:no` scan alike. A sealed generation records the normalizer
//! version and the text-authority format it was built under and is refused
//! typed when either differs from the running one.

#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "tantivy 0.22 pulls multiple transitive versions (rustix, linux-raw-sys, windows-sys) we cannot collapse; scoped allowance in deny.toml [bans] skip-tree."
)]

mod analyzer;
mod budgeted_search;
mod dense_admission;
pub mod filters;
pub mod history_text_index;
pub mod phrase;
pub mod plan;
pub mod planner;
mod predicate_registry;
pub mod regex;
mod sealed_generation;
pub mod symbol;
mod text_authority;
pub mod trigram_plan;

/// The one text normalization contract (QI-BB-011), shared with the query DSL
/// and the search plane; every text surface of this crate lowers through it.
pub(crate) use quanta_index_lq_text_normalizer as normalize;
pub use sealed_generation::LexicalSealCommitmentStats;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ciborium::Value as CborValue;
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{
    LexicalErrorCode, SymbolKindCode, SymbolKindFamily, SymbolRecord,
};
use quanta_index_contract::{
    BatchIngestMode, CandidatePresenceV1, ChunkRecord, ClearLexicalSurface,
    FileContributorIdentityEntry, FileContributorIngestBatch, FileOwnerProjectionRow,
    FileOwnershipIngestBatch, GenerationSnapshot, HighlightSpan, LexicalCandidate,
    LexicalFullBundle, LexicalSeal, LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions,
    LqPatternType, LqPredicateArg, LqQuery, LqSelect, LqType, LqVisibility, LqYesNoOnly,
    ManifestGeneration, QueryConstraintSetV1, ReplaceLexicalScope, RepoCommitRecencyIngestBatch,
    RepoDescriptionIngestBatch, RepoId, RepoMetaIngestBatch, RepoRelativePath,
    RepoTopicIngestBatch, RevisionId, SearchCorpusIngestBatch, SearchPlaneTrackKind,
    SearchScopeSurface, SymbolCandidate, TombstoneLexicalScope,
};
use quanta_index_core::domains::generation::{
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, IncompleteGenerationDiscardOutcomeV1,
    IncompleteGenerationDiscardPort, InventoriedSealedGenerationV1, QuarantinedGenerationV1,
    SealedGenerationBytesV1, SealedGenerationInventoryV1, SealedGenerationReclaimOutcomeV1,
    SealedGenerationReclaimPort, unique_inode_tree_bytes,
};
use quanta_index_core::{
    CoreError, FileContributorIngestPort, FileOwnershipIngestPort, GenerationIdentityValidatePort,
    IntegrityScrubBudgetV1, IntegrityScrubCandidateV1, IntegrityScrubCursorV1, IntegrityScrubPort,
    IntegrityScrubReportV1, LEXICAL_WRITER_HEAP_BYTES_MIN, LexicalArtifactIdentityV1,
    LexicalCandidateExplanationV1, LexicalExecutionBudgetV1, LexicalIndexBuildPort,
    LexicalIndexOpenPort, LexicalPredicateV1, LexicalScoreEngineV1, LexicalScoreTraceV1,
    LexicalSearchPageV1, LexicalSearcher, LexicalWriterCacheStats, LexicalWriterPolicy,
    MetricPointV1, MetricSourcePort, QUARANTINE_TARGET_NOT_QUARANTINED_CODE,
    QuarantineDiscardOutcomeV1, QuarantinedGenerationDiscardPort, RegexMatchCachePolicy,
    RegexMatchCacheStats, RepoCommitRecencyIngestPort, RepoDescriptionIngestPort,
    RepoMetaIngestPort, RepoMetadataAuthoritiesV1, RepoMetadataAuthorityV1, RepoTopicIngestPort,
    RequestBudgetV1, SealedGenerationScanPort, SearchCorpusBatchBuildPort,
    TextAuthorityUpdateStats, TextNormalizerVersionV1, TrackDiskUsagePort,
    UnboundedWriterAdmission, WriterAdmissionPort, WriterIdleSweepPort, count_from_usize,
    domains::lexical::LexicalPolicy,
    timeref::{is_rev_at_time_spec, parse_search_timeref_ms},
};

const LEXICAL_SEALED_IDENTITY_FILE_NAME: &str = "search-corpus-generation-identity.cbor";
/// Records which base generation a delta generation actually carried forward.
///
/// Directory existence is not proof of carry-forward: the per-generation
/// authority sidecars each create the generation directory as a side effect,
/// so a sidecar published before the lexical delta would otherwise make the
/// base clone look already done. This marker is the authority instead.
const LEXICAL_DELTA_BASE_FILE_NAME: &str = "search-corpus-delta-base.cbor";
/// Tantivy writes this on every commit; its presence proves index content exists.
const TANTIVY_INDEX_META_FILE_NAME: &str = "meta.json";
/// Tantivy's managed-file list, rewritten whenever the generation's file set changes.
const TANTIVY_MANAGED_FILE_NAME: &str = ".managed.json";
/// Prefix of Tantivy's lock files, which belong to one live writer only.
const TANTIVY_LOCK_FILE_PREFIX: &str = ".tantivy";
use quanta_index_lq_positions::{PositionsError, PositionsErrorCode, query_phrase};
use quanta_index_lq_regex::RegexExecutor;
use quanta_index_lq_trigram::{
    DocId as TrigramDocId, TrigramError, TrigramErrorCode, query_raw_substring,
    regex_prefilter_any_of,
};

use crate::analyzer::{NormalizingTokenizer, tokenizer_name};
use crate::budgeted_search::{BudgetProbe, budgeted_search};
use crate::normalize::{CaseMode, TEXT_NORMALIZER_VERSION, TextNormalizerVersion, TextQueryError};
use crate::phrase::{PhraseField, PhrasePlannerError, PhrasePolicy, plan_phrase};
use crate::predicate_registry::{
    ContentPathScope, ContentPredicateArgError, ContentPredicateConstraint, ContentScalarArg,
    ContentScalarArgError, ContributorPattern, FileContributorArg, FileContributorArgError,
    FileOwnerArg, FileOwnerArgError, MetaPattern, PREDICATE_OWNER, PredicateKind,
    RepoDescriptionArg, RepoDescriptionArgError, RepoFileArgError, RepoFileConstraint,
    RepoFileMatcher, RepoMetaArg, RepoMetaArgError, RepoTopicArg, RepoTopicArgError,
    TimerefScalarArgError, canonicalize_predicate_call, kind_of,
    parse_content_predicate_constraint, parse_content_scalar_arg, parse_file_contributor_arg,
    parse_file_owner_arg, parse_repo_description_arg, parse_repo_file_matchers,
    parse_repo_meta_arg, parse_repo_topic_arg, parse_timeref_scalar_arg, unimplemented_predicate,
};
use crate::regex::RegexPolicy;
use crate::sealed_generation::{
    DiscardingVisitor, LEXICAL_QUARANTINE_RECEIPT_FILE_NAME, LEXICAL_SCRUB_RECEIPT_FILE_NAME,
    LEXICAL_SEALED_MANIFEST_FILE_NAME, OverlayFamily, SealedGenerationVisitor,
    last_completed_scrub, persist_overlay, quarantined_by_scrub, remove_overlay, scrub_step,
    seal_generation, walk_sealed_generation,
};
use crate::text_authority::{
    AddedTextDoc, ShardBody, ShardedTextAuthority, TEXT_AUTHORITY_DIR_NAME, TextAuthorityManifest,
    TextAuthorityWriteReceipt, shard_index_of,
};
use tantivy::DocSet as _;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{
    AllQuery, BooleanQuery, EnableScoring, Occur, PhraseQuery, Query, RegexQuery, TermQuery,
};
use tantivy::schema::{
    Field, IndexRecordOption, OwnedValue, STORED, STRING, Schema, TantivyDocument,
    TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::TextAnalyzer;
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Term};

const TEXT_DOC_KIND: &str = "text";
const SYMBOL_DOC_KIND: &str = "symbol";
/// Typed refusal for a sealed generation whose bytes are not what its
/// manifest committed to: a file missing, truncated, rewritten, stale, or
/// present although the seal never listed it.
const GENERATION_SIDECAR_CORRUPT_CODE: &str = "GENERATION_SIDECAR_CORRUPT";
/// Typed refusal for anything that would change a sealed generation.
const GENERATION_IMMUTABLE_CODE: &str = "GENERATION_IMMUTABLE";
/// Marker inside the name of a durable write's temporary file.
const DURABLE_WRITE_TEMPORARY_MARKER: &str = ".tmp-";

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
    /// The text-authority doc id of a text document (absent on symbols):
    /// the shard-addressing key the sidecar and the index share, assigned
    /// once when the document is written and never reused within the
    /// generation's chain.
    text_authority_doc_id: Field,
}

impl SchemaFields {
    fn build() -> Self {
        let mut builder = Schema::builder();
        let candidate_id = builder.add_text_field("candidate_id", STRING | STORED);
        let repo_id = builder.add_text_field("repo_id", STRING | STORED);
        let revision_id = builder.add_text_field("revision_id", STRING | STORED);
        let doc_kind = builder.add_text_field("doc_kind", STRING | STORED);
        let repo_relative_path = builder.add_text_field("repo_relative_path", STRING | STORED);
        let repo_relative_path_query = builder.add_text_field(
            "repo_relative_path_query",
            tokenized_text_options(CaseMode::Folded),
        );
        let repo_relative_path_case = builder.add_text_field(
            "repo_relative_path_case",
            tokenized_text_options(CaseMode::Sensitive),
        );
        let file_name = builder.add_text_field("file_name", STRING);
        let language = builder.add_text_field("language", STRING);
        let start_line = builder.add_u64_field("start_line", STORED);
        let end_line = builder.add_u64_field("end_line", STORED);
        let snippet = builder.add_text_field("snippet", STORED);
        let chunk_text = builder.add_text_field(
            "chunk_text",
            tokenized_text_options(CaseMode::Folded) | STORED,
        );
        let chunk_text_case = builder.add_text_field(
            "chunk_text_case",
            tokenized_text_options(CaseMode::Sensitive),
        );
        let symbol_kind = builder.add_text_field("symbol_kind", STRING | STORED);
        let symbol_kind_family = builder.add_text_field("symbol_kind_family", STRING | STORED);
        let text_authority_doc_id = builder.add_u64_field("text_authority_doc_id", STORED);
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
            text_authority_doc_id,
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

/// One cached writer and when it was last touched.
struct CachedWriter {
    handle: Arc<Mutex<GenerationWriter>>,
    last_used: Instant,
}

/// Why the cache committed and released a writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriterRelease {
    /// Room was needed for another writer.
    Lru,
    /// Nothing touched it for the policy's idle interval.
    Idle,
    /// Its generation sealed; no build will touch it again.
    Seal,
}

/// The largest number of indexing threads one writer runs.
const WRITER_THREADS_MAX: usize = 8;

/// Bounded cache of per-generation Tantivy writers under one heap envelope
/// (QI-BB-016).
///
/// # Capacity
///
/// At most [`LexicalWriterPolicy::max_writers`] entries are retained — the
/// envelope divided by one writer's heap — so the heap every open writer may
/// use together never exceeds the envelope. Inserting past the cap releases
/// the least-recently-used entry first.
///
/// # Idle release
///
/// A writer nothing has touched for the policy's idle interval is committed
/// and released the next time the cache is entered, and whenever the adapter
/// sweeps after a batch; a producer that stops mid-generation does not pin
/// its heap forever.
///
/// # Commit-on-release guarantee
///
/// Releasing an entry calls `IndexWriter::commit` on the writer it owns. The
/// adapter's invariant is that any uncommitted ops belong to an in-flight
/// `build` invocation that holds the entry's `Arc<Mutex<GenerationWriter>>`;
/// the cache only drops its own `Arc`, so an in-flight build is unaffected.
/// A failing commit is surfaced as a typed storage error and aborts the
/// operation that triggered the release.
struct WriterCache {
    entries: BTreeMap<GenKey, CachedWriter>,
    order: VecDeque<GenKey>,
    policy: LexicalWriterPolicy,
    /// Asked before a writer the cache does not hold is opened (QI-BB-016):
    /// the composition root's resident-memory gate, or the unbounded one.
    admission: Arc<dyn WriterAdmissionPort>,
    lru_releases: u64,
    idle_releases: u64,
    seal_releases: u64,
}

impl WriterCache {
    fn new(policy: LexicalWriterPolicy) -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            policy,
            admission: Arc::new(UnboundedWriterAdmission),
            lru_releases: 0,
            idle_releases: 0,
            seal_releases: 0,
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

    fn remove(&mut self, key: &GenKey) -> Option<Arc<Mutex<GenerationWriter>>> {
        self.order.retain(|candidate| candidate != key);
        self.entries.remove(key).map(|cached| cached.handle)
    }

    /// Commit and drop one writer, counting why.
    ///
    /// A seal release also waits for the writer's background merges: a
    /// merge that finished after the seal measured the directory would
    /// rewrite `meta.json` behind the manifest, so the seal's commit is
    /// only final once no merge is in flight. That needs the writer by
    /// value, which is only possible when no in-flight build still holds
    /// it — for a seal, a structural guarantee the batch order gives.
    fn release(&mut self, key: &GenKey, why: WriterRelease) -> Result<(), CoreError> {
        let Some(victim) = self.remove(key) else {
            return Err(CoreError::Storage(
                "lexical writer cache: order references missing entry".to_string(),
            ));
        };
        match why {
            WriterRelease::Seal => {
                let Ok(owned) = Arc::try_unwrap(victim) else {
                    return Err(CoreError::Storage(format!(
                        "lexical: seal of generation {} while a build still holds its writer",
                        key.generation.get()
                    )));
                };
                let GenerationWriter { index, mut writer } = owned.into_inner().map_err(|err| {
                    CoreError::Storage(format!("lexical: release lock poisoned: {err}"))
                })?;
                let _opstamp = writer
                    .commit()
                    .map_err(|err| CoreError::Storage(format!("lexical: seal commit: {err}")))?;
                writer.wait_merging_threads().map_err(|err| {
                    CoreError::Storage(format!("lexical: seal wait for merges: {err}"))
                })?;
                drop(index);
            }
            WriterRelease::Lru | WriterRelease::Idle => {
                // If a concurrent build still holds the Arc this lock contends;
                // that is acceptable because the cap is small and contention
                // only happens on release, not on the hot path.
                let mut guarded = victim.lock().map_err(|err| {
                    CoreError::Storage(format!("lexical: release lock poisoned: {err}"))
                })?;
                let _opstamp = guarded
                    .writer
                    .commit()
                    .map_err(|err| CoreError::Storage(format!("lexical: release commit: {err}")))?;
                drop(guarded);
                drop(victim);
            }
        }
        match why {
            WriterRelease::Lru => self.lru_releases = self.lru_releases.saturating_add(1),
            WriterRelease::Idle => self.idle_releases = self.idle_releases.saturating_add(1),
            WriterRelease::Seal => self.seal_releases = self.seal_releases.saturating_add(1),
        }
        Ok(())
    }

    /// Release the least-recently-used writers until there is room for one
    /// more under the envelope.
    fn release_until_room(&mut self) -> Result<(), CoreError> {
        let max_writers = self.policy.max_writers();
        while self.entries.len() >= max_writers {
            let Some(victim_key) = self.order.front().cloned() else {
                // entries and order are kept in lock-step; an empty order with
                // non-empty entries would be a structural bug.
                return Err(CoreError::Storage(
                    "lexical writer cache: order/entries desync during release".to_string(),
                ));
            };
            self.release(&victim_key, WriterRelease::Lru)?;
        }
        Ok(())
    }

    /// Release every writer nothing has touched for the idle interval;
    /// returns how many were released.
    fn release_idle(&mut self, now: Instant) -> Result<u64, CoreError> {
        let idle_after = self.policy.idle_after();
        let idle: Vec<GenKey> = self
            .entries
            .iter()
            .filter(|(_, cached)| now.saturating_duration_since(cached.last_used) >= idle_after)
            .map(|(key, _)| key.clone())
            .collect();
        let released = count_from_usize(idle.len());
        for key in idle {
            self.release(&key, WriterRelease::Idle)?;
        }
        Ok(released)
    }

    /// Returns the cached handle for `key`, opening and inserting a new one if
    /// absent. The returned handle is the entry's `Arc<Mutex<_>>`; the cache
    /// retains its own clone so subsequent calls hit the same writer.
    fn get_or_open(
        &mut self,
        key: &GenKey,
        fields: &SchemaFields,
        path: &Path,
        now: Instant,
    ) -> Result<Arc<Mutex<GenerationWriter>>, CoreError> {
        if let Some(existing) = self.entries.get_mut(key) {
            existing.last_used = now;
            let cloned = Arc::clone(&existing.handle);
            self.touch(key);
            return Ok(cloned);
        }
        let _released = self.release_idle(now)?;
        self.release_until_room()?;
        // The process-level gate (QI-BB-016): a writer is never opened while
        // the process is above its resident-memory ceiling.
        self.admission.admit_writer_open()?;
        let index = open_or_create_index(fields, path)?;
        let heap_bytes = usize::try_from(self.policy.writer_heap_bytes()).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: writer heap does not fit this platform: {err}"
            ))
        })?;
        let writer: IndexWriter = index
            .writer_with_num_threads(writer_threads_for_heap(heap_bytes), heap_bytes)
            .map_err(|err| CoreError::Storage(format!("lexical: writer: {err}")))?;
        let handle = Arc::new(Mutex::new(GenerationWriter { index, writer }));
        let _prior = self.entries.insert(
            key.clone(),
            CachedWriter {
                handle: Arc::clone(&handle),
                last_used: now,
            },
        );
        self.order.push_back(key.clone());
        Ok(handle)
    }

    fn stats(&self) -> LexicalWriterCacheStats {
        let open_writers = self.entries.len();
        LexicalWriterCacheStats {
            open_writers,
            max_writers: self.policy.max_writers(),
            allocated_heap_bytes: u64::try_from(open_writers).map_or(u64::MAX, |writers| {
                writers.saturating_mul(self.policy.writer_heap_bytes())
            }),
            lru_releases: self.lru_releases,
            idle_releases: self.idle_releases,
            seal_releases: self.seal_releases,
        }
    }
}

/// Indexing threads for one writer of `heap_bytes`.
///
/// As many as the heap gives the minimum arena to, capped by the machine
/// and by the writer's own ceiling — the derivation the library applies,
/// made explicit so the envelope's per-writer term is the whole story.
fn writer_threads_for_heap(heap_bytes: usize) -> usize {
    let by_heap = usize::try_from(LEXICAL_WRITER_HEAP_BYTES_MIN)
        .map_or(1, |minimum| {
            heap_bytes.checked_div(minimum).map_or(1, |threads| threads)
        })
        .max(1);
    let by_machine = std::thread::available_parallelism().map_or(1, usize::from);
    by_heap.min(by_machine).min(WRITER_THREADS_MAX)
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RegexMatchCacheKey {
    generation: GenKey,
    normalized_source: String,
}

/// Why a computed match set was not cached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegexMatchCacheRefusal {
    Cardinality { matches: usize },
    Bytes { bytes: u64 },
}

/// Per-entry accounting overhead beyond the candidate id bytes: the
/// `String` header and the tree node, rounded generously so the byte bound
/// errs on the side of counting more.
const REGEX_MATCH_CACHE_ENTRY_OVERHEAD: u64 = 64;

/// Bytes one match set holds resident: every candidate id's bytes plus the
/// per-id overhead.
fn regex_match_set_bytes(matches: &BTreeSet<String>) -> u64 {
    matches.iter().fold(0_u64, |total, id| {
        total
            .saturating_add(u64::try_from(id.len()).map_or(u64::MAX, |len| len))
            .saturating_add(REGEX_MATCH_CACHE_ENTRY_OVERHEAD)
    })
}

/// Byte-weighted LRU of regex match sets, shared by `Arc` (QI-BB-024).
///
/// A hit hands out the shared set — never a deep clone of every candidate
/// id — and an insert evicts least-recently-used entries until both the
/// entry and the byte bound hold. A result wider than one entry may be is
/// refused rather than cached, so a run of broad regexes cannot turn the
/// cache into copies of the corpus.
struct RegexMatchCache {
    entries: BTreeMap<RegexMatchCacheKey, Arc<BTreeSet<String>>>,
    order: VecDeque<RegexMatchCacheKey>,
    policy: RegexMatchCachePolicy,
    resident_bytes: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
    refused_cardinality: u64,
    refused_bytes: u64,
}

impl RegexMatchCache {
    fn new(policy: RegexMatchCachePolicy) -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            policy,
            resident_bytes: 0,
            hits: 0,
            misses: 0,
            evictions: 0,
            refused_cardinality: 0,
            refused_bytes: 0,
        }
    }

    fn touch(&mut self, key: &RegexMatchCacheKey) {
        if let Some(position) = self.order.iter().position(|candidate| candidate == key) {
            let removed = self.order.remove(position);
            if let Some(key) = removed {
                self.order.push_back(key);
            }
        }
    }

    fn get(&mut self, key: &RegexMatchCacheKey) -> Option<Arc<BTreeSet<String>>> {
        let Some(cached) = self.entries.get(key).map(Arc::clone) else {
            self.misses = self.misses.saturating_add(1);
            return None;
        };
        self.hits = self.hits.saturating_add(1);
        self.touch(key);
        Some(cached)
    }

    fn remove_entry(&mut self, key: &RegexMatchCacheKey) {
        if let Some(removed) = self.entries.remove(key) {
            self.resident_bytes = self
                .resident_bytes
                .saturating_sub(regex_match_set_bytes(&removed));
        }
    }

    fn insert(
        &mut self,
        key: RegexMatchCacheKey,
        matches: Arc<BTreeSet<String>>,
    ) -> Result<(), RegexMatchCacheRefusal> {
        if matches.len() > self.policy.max_matches_per_entry() {
            self.refused_cardinality = self.refused_cardinality.saturating_add(1);
            return Err(RegexMatchCacheRefusal::Cardinality {
                matches: matches.len(),
            });
        }
        let bytes = regex_match_set_bytes(&matches);
        if bytes > self.policy.max_resident_bytes() {
            self.refused_bytes = self.refused_bytes.saturating_add(1);
            return Err(RegexMatchCacheRefusal::Bytes { bytes });
        }
        if self.entries.contains_key(&key) {
            self.remove_entry(&key);
            self.order.retain(|candidate| candidate != &key);
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= self.policy.max_entries()
                || self.resident_bytes.saturating_add(bytes) > self.policy.max_resident_bytes())
        {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            self.remove_entry(&evicted);
            self.evictions = self.evictions.saturating_add(1);
        }
        self.order.push_back(key.clone());
        self.resident_bytes = self.resident_bytes.saturating_add(bytes);
        let _inserted = self.entries.insert(key, matches);
        Ok(())
    }

    fn invalidate_generation(&mut self, generation: &GenKey) {
        let stale: Vec<RegexMatchCacheKey> = self
            .entries
            .keys()
            .filter(|key| &key.generation == generation)
            .cloned()
            .collect();
        for key in &stale {
            self.remove_entry(key);
        }
        self.order.retain(|key| &key.generation != generation);
    }

    fn stats(&self) -> RegexMatchCacheStats {
        RegexMatchCacheStats {
            hits: self.hits,
            misses: self.misses,
            entries: self.entries.len(),
            resident_bytes: self.resident_bytes,
            evictions: self.evictions,
            refused_cardinality: self.refused_cardinality,
            refused_bytes: self.refused_bytes,
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
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

/// A length or a count as `u64`; a platform where `usize` exceeds `u64`
/// is refused rather than truncated.
fn count_from_len(value: usize) -> Result<u64, CoreError> {
    u64::try_from(value)
        .map_err(|err| CoreError::InvalidContract(format!("lexical: count overflow: {err}")))
}

/// Open an existing generation's index strictly, never creating or
/// repairing one, with the tokenizers registered.
///
/// A cached writer handle could survive deletion or corruption of the
/// backing files, so every door bypasses that cache and opens the durable
/// directory.
fn open_sealed_index(generation_dir: &Path) -> Result<Index, CoreError> {
    let index = Index::open_in_dir(generation_dir).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: strict open existing generation {}: {error}",
            generation_dir.display()
        ))
    })?;
    register_index_tokenizers(&index);
    Ok(index)
}

/// Whether `name` is a durable write's temporary file
/// (`.<file>.tmp-<pid>-<n>`), which only a crash leaves behind.
fn is_durable_write_temporary(name: &str) -> bool {
    name.starts_with('.') && name.contains(DURABLE_WRITE_TEMPORARY_MARKER)
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
        quanta_index_contract::SearchCorpusReplaceScope,
    ),
    CoreError,
> {
    ciborium::from_reader::<
        (
            BatchIngestMode,
            Option<ManifestGeneration>,
            quanta_index_contract::SearchCorpusReplaceScope,
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
        quanta_index_contract::SearchCorpusTombstoneScope,
    ),
    CoreError,
> {
    ciborium::from_reader::<
        (
            BatchIngestMode,
            Option<ManifestGeneration>,
            quanta_index_contract::SearchCorpusTombstoneScope,
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

/// The commit-recency snapshot one batch publishes, encoded.
fn encode_repo_commit_recency_batch(
    batch: &RepoCommitRecencyIngestBatch,
) -> Result<Vec<u8>, CoreError> {
    let mut latest_committer_time_ms_by_repo_id = BTreeMap::new();
    for entry in &batch.entries {
        let _prior = latest_committer_time_ms_by_repo_id.insert(
            entry.source_repo_id.as_str().to_string(),
            entry.latest_committer_time_ms,
        );
    }
    encode_repo_commit_recency_snapshot(&RepoCommitRecencyShard {
        latest_committer_time_ms_by_repo_id,
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

/// The repo-meta snapshot one batch publishes, encoded.
fn encode_repo_meta_batch(batch: &RepoMetaIngestBatch) -> Result<Vec<u8>, CoreError> {
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
    encode_repo_meta_snapshot(&RepoMetaShard { meta_by_repo_id })
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

/// The repo-topic snapshot one batch publishes, encoded.
fn encode_repo_topic_batch(batch: &RepoTopicIngestBatch) -> Result<Vec<u8>, CoreError> {
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
    encode_repo_topic_snapshot(&RepoTopicShard { topics_by_repo_id })
}

/// Validate a producer-published repo description.
///
/// Unlike topics, the description is stored verbatim (case and internal
/// whitespace preserved) so regex matching at query time is faithful; only a
/// non-empty constraint is enforced. Returns the original string when it carries
/// a non-whitespace character, `None` otherwise.
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
                    if source_repo_id.is_some() {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo description decode: duplicate entry field `source_repo_id`"
                                .to_string(),
                        ));
                    }
                    let CborValue::Text(text) = field_value else {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo description decode: `source_repo_id` must be text"
                                .to_string(),
                        ));
                    };
                    source_repo_id = Some(text);
                }
                "description" => {
                    if description.is_some() {
                        return Err(CoreError::InvalidContract(
                            "lexical: repo description decode: duplicate entry field `description`"
                                .to_string(),
                        ));
                    }
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

/// The repo-description snapshot one batch publishes, encoded.
fn encode_repo_description_batch(batch: &RepoDescriptionIngestBatch) -> Result<Vec<u8>, CoreError> {
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
        if let Some(prior) =
            descriptions_by_repo_id.insert(entry.source_repo_id.as_str().to_string(), description)
        {
            return Err(CoreError::InvalidContract(format!(
                "lexical: repo description ingest carries conflicting entries for source_repo_id `{}` (prior `{prior}`); one description per repo per batch",
                entry.source_repo_id.as_str()
            )));
        }
    }
    encode_repo_description_snapshot(&RepoDescriptionShard {
        descriptions_by_repo_id,
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

/// The file-ownership snapshot one batch publishes, encoded.
fn encode_file_ownership_batch(batch: &FileOwnershipIngestBatch) -> Result<Vec<u8>, CoreError> {
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
    encode_file_ownership_snapshot(&FileOwnershipShard { owners_by_repo_id })
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
                            .map(|contributor| {
                                let mut fields = vec![(
                                    CborValue::Text("canonical".to_string()),
                                    CborValue::Text(contributor.canonical),
                                )];
                                if let Some(name) = contributor.name {
                                    fields.push((
                                        CborValue::Text("name".to_string()),
                                        CborValue::Text(name),
                                    ));
                                }
                                if let Some(email) = contributor.email {
                                    fields.push((
                                        CborValue::Text("email".to_string()),
                                        CborValue::Text(email),
                                    ));
                                }
                                CborValue::Map(fields)
                            })
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
    let mut contributors_by_repo_id: BTreeMap<
        String,
        BTreeMap<String, BTreeSet<FileContributorIdentityEntry>>,
    > = BTreeMap::new();
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
        let mut contributors: Option<BTreeSet<FileContributorIdentityEntry>> = None;
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
                    let mut normalized: BTreeSet<FileContributorIdentityEntry> = BTreeSet::new();
                    for item in items {
                        let contributor = decode_file_contributor_identity_entry(&item)?;
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

/// The file-contributor snapshot one batch publishes, encoded.
fn encode_file_contributor_batch(batch: &FileContributorIngestBatch) -> Result<Vec<u8>, CoreError> {
    let mut contributors_by_repo_id: BTreeMap<
        String,
        BTreeMap<String, BTreeSet<FileContributorIdentityEntry>>,
    > = BTreeMap::new();
    for entry in &batch.entries {
        let repo_relative_path = entry.repo_relative_path.as_str().to_string();
        let contributors = entry
            .contributors
            .iter()
            .map(|contributor| {
                normalize_contributor_identity_entry(contributor).ok_or_else(|| {
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
    encode_file_contributor_snapshot(&FileContributorShard {
        contributors_by_repo_id,
    })
}

/// One overlay family's snapshot as a query holds it, decoded from the
/// bytes the sealed-generation walk proved.
enum OverlaySnapshot {
    RepoMetadata(LexicalRepoMetadataPayload),
    CommitRecency(RepoCommitRecencyShard),
    Meta(RepoMetaShard),
    Topic(RepoTopicShard),
    Description(RepoDescriptionShard),
    FileOwnership(FileOwnershipShard),
    Contributor(FileContributorShard),
}

/// Decode one overlay family's proved bytes; a decode failure names the
/// file and is a corrupt sealed generation, never a partial authority.
fn decode_overlay(
    family: OverlayFamily,
    bytes: &[u8],
    generation_dir: &Path,
) -> Result<OverlaySnapshot, CoreError> {
    let decoded = match family {
        OverlayFamily::RepoMetadata => decode_repo_metadata_payload(bytes).and_then(|payload| {
            payload.map(OverlaySnapshot::RepoMetadata).ok_or_else(|| {
                CoreError::InvalidContract("repo metadata snapshot carries no payload".to_string())
            })
        }),
        OverlayFamily::CommitRecency => {
            decode_repo_commit_recency_snapshot(bytes).map(OverlaySnapshot::CommitRecency)
        }
        OverlayFamily::Meta => decode_repo_meta_snapshot(bytes).map(OverlaySnapshot::Meta),
        OverlayFamily::Topic => decode_repo_topic_snapshot(bytes).map(OverlaySnapshot::Topic),
        OverlayFamily::Description => {
            decode_repo_description_snapshot(bytes).map(OverlaySnapshot::Description)
        }
        OverlayFamily::FileOwnership => {
            decode_file_ownership_snapshot(bytes).map(OverlaySnapshot::FileOwnership)
        }
        OverlayFamily::Contributor => {
            decode_file_contributor_snapshot(bytes).map(OverlaySnapshot::Contributor)
        }
    };
    decoded.map_err(|err| match err {
        CoreError::InvalidContract(message) => {
            sidecar_corrupt(generation_dir, family.file_name(), &message)
        }
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

fn normalize_repo_meta_pattern(pattern: MetaPattern) -> Result<MetaPattern, RepoMetaArgError> {
    match pattern {
        MetaPattern::Exact(value) => normalize_repo_meta_key(&value)
            .map(MetaPattern::Exact)
            .ok_or(RepoMetaArgError::EmptyKey),
        MetaPattern::Regex(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Err(RepoMetaArgError::EmptyKey)
            } else {
                Ok(MetaPattern::Regex(trimmed.to_string()))
            }
        }
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

fn normalize_contributor_identity_entry(
    entry: &FileContributorIdentityEntry,
) -> Option<FileContributorIdentityEntry> {
    let canonical = normalize_contributor_identity(&entry.canonical)?;
    let name = entry
        .name
        .as_deref()
        .and_then(normalize_contributor_identity);
    let email = entry
        .email
        .as_deref()
        .and_then(normalize_contributor_identity);
    Some(FileContributorIdentityEntry {
        canonical,
        name,
        email,
    })
}

// Fail-closed CBOR decode: every wildcard arm below rejects any value that is
// not the explicitly admitted shape (Text/Null per field, Text/Map per entry).
// `wildcard_enum_match_arm` is expected at the function level because the lint is
// emitted against the arm *pattern*, which an arm-local attribute does not cover.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "fail-closed decode: any CBOR variant outside the admitted shapes is a contract violation, caught by the wildcard arms"
)]
fn decode_file_contributor_identity_entry(
    value: &CborValue,
) -> Result<FileContributorIdentityEntry, CoreError> {
    match value {
        CborValue::Text(text) => {
            normalize_contributor_identity_entry(&FileContributorIdentityEntry {
                canonical: text.clone(),
                name: None,
                email: None,
            })
            .ok_or_else(|| {
                CoreError::InvalidContract(
                    "lexical: file contributor decode: contributor must be non-empty".to_string(),
                )
            })
        }
        CborValue::Map(fields) => {
            let mut canonical: Option<String> = None;
            let mut name: Option<Option<String>> = None;
            let mut email: Option<Option<String>> = None;
            for (field_key, field_value) in fields {
                let CborValue::Text(field_name) = field_key else {
                    return Err(CoreError::InvalidContract(
                        "lexical: file contributor decode: contributor field name must be text"
                            .to_string(),
                    ));
                };
                match field_name.as_str() {
                    "canonical" => {
                        let CborValue::Text(text) = field_value else {
                            return Err(CoreError::InvalidContract(
                                "lexical: file contributor decode: contributor `canonical` must be text"
                                    .to_string(),
                            ));
                        };
                        canonical = Some(text.clone());
                    }
                    "name" => match field_value {
                        CborValue::Text(text) => name = Some(Some(text.clone())),
                        CborValue::Null => name = Some(None),
                        _ => {
                            return Err(CoreError::InvalidContract(
                                "lexical: file contributor decode: contributor `name` must be text or null"
                                    .to_string(),
                            ));
                        }
                    },
                    "email" => match field_value {
                        CborValue::Text(text) => email = Some(Some(text.clone())),
                        CborValue::Null => email = Some(None),
                        _ => {
                            return Err(CoreError::InvalidContract(
                                "lexical: file contributor decode: contributor `email` must be text or null"
                                    .to_string(),
                            ));
                        }
                    },
                    other => {
                        return Err(CoreError::InvalidContract(format!(
                            "lexical: file contributor decode: unknown contributor field `{other}`"
                        )));
                    }
                }
            }
            normalize_contributor_identity_entry(&FileContributorIdentityEntry {
                canonical: canonical.ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical: file contributor decode: contributor missing field `canonical`"
                            .to_string(),
                    )
                })?,
                name: name.unwrap_or(None),
                email: email.unwrap_or(None),
            })
            .ok_or_else(|| {
                CoreError::InvalidContract(
                    "lexical: file contributor decode: contributor must be non-empty".to_string(),
                )
            })
        }
        _ => Err(CoreError::InvalidContract(
            "lexical: file contributor decode: contributor must be text or map".to_string(),
        )),
    }
}

fn file_name_for_path(path: &str) -> Option<&str> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
}

fn language_from_path_hint(path: &str) -> Option<&'static str> {
    let ext = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "md" => Some("markdown"),
        "java" => Some("java"),
        "js" => Some("javascript"),
        "ts" => Some("typescript"),
        "jsx" => Some("javascriptreact"),
        "tsx" => Some("typescriptreact"),
        "rb" => Some("ruby"),
        "go" => Some("go"),
        "c" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Some("cpp"),
        "cs" => Some("csharp"),
        "kt" | "kts" => Some("kotlin"),
        "swift" => Some("swift"),
        "scala" => Some("scala"),
        "php" => Some("php"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "sh" | "bash" => Some("shell"),
        "txt" => Some("text"),
        _ => None,
    }
}

/// Indexing options for a text field analyzed by the shared normalizer
/// under `case`, with positions so keyword sequences can be phrase-matched.
fn tokenized_text_options(case: CaseMode) -> TextOptions {
    TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(tokenizer_name(case))
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

/// Store and index a chunk's text in its NFC form.
///
/// The stored copy is what the text-authority sidecars and the `index:no`
/// scan read back, so normalizing here (and idempotently again inside the
/// analyzer) keeps every surface over the same bytes.
fn add_content_fields(fields: &SchemaFields, doc: &mut TantivyDocument, indexed_text: &str) {
    let indexed_text = normalize::nfc(indexed_text);
    doc.add_text(fields.chunk_text, indexed_text.as_ref());
    doc.add_text(fields.chunk_text_case, indexed_text.as_ref());
}

fn add_symbol_fields(fields: &SchemaFields, doc: &mut TantivyDocument, symbol: &SymbolRecord) {
    doc.add_text(fields.symbol_kind, symbol.symbol_kind.as_str());
    if let Some(symbol_kind_family) = symbol.symbol_kind_family {
        doc.add_text(fields.symbol_kind_family, symbol_kind_family.as_code_str());
    }
}

/// Register the two analyzers every text field names.
///
/// Both are the shared normalizer; they differ only in case mode. No
/// filter is chained after it: the normalizer already owns boundaries,
/// folding, and the term-length cap.
fn register_index_tokenizers(index: &Index) {
    for case in [CaseMode::Folded, CaseMode::Sensitive] {
        index.tokenizers().register(
            tokenizer_name(case),
            TextAnalyzer::from(NormalizingTokenizer::new(case)),
        );
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
    let index = Index::builder()
        .schema(fields.schema.clone())
        .open_or_create(directory)
        .map_err(|err| CoreError::Storage(format!("lexical: open generation index: {err}")))?;
    register_index_tokenizers(&index);
    Ok(index)
}

fn lexical_sealed_identity_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_SEALED_IDENTITY_FILE_NAME)
}

/// Write `bytes` to `path` durably.
///
/// A uniquely named temporary beside it, fsync, rename over `path`, fsync
/// the parent. A crash leaves either the old file or the new one, never a
/// torn one, plus at most a temporary the seal removes
/// ([`is_durable_write_temporary`]).
fn write_atomic_durable(path: &Path, bytes: &[u8], label: &str) -> Result<(), CoreError> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "lexical: {label} path has no parent: {}",
            path.display()
        ))
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "lexical: {label} has no UTF-8 file name: {}",
                path.display()
            ))
        })?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}{DURABLE_WRITE_TEMPORARY_MARKER}{}-{sequence}",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: create {label} temporary {}: {error}",
                temporary.display()
            ))
        })?;
    file.write_all(bytes).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: write {label} temporary {}: {error}",
            temporary.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        CoreError::Storage(format!(
            "lexical: fsync {label} temporary {}: {error}",
            temporary.display()
        ))
    })?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: rename {label} temporary {} to {}: {error}",
            temporary.display(),
            path.display()
        ))
    })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: fsync {label} parent {}: {error}",
                parent.display()
            ))
        })
}

fn persist_lexical_sealed_identity(
    generation_dir: &Path,
    identity: &GenerationSnapshot,
) -> Result<(), CoreError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(identity, &mut bytes).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: encode sealed generation identity for repo={} revision={} generation={}: {error}",
            identity.repo_id.as_str(),
            identity.revision_id.as_str(),
            identity.manifest_generation.get(),
        ))
    })?;
    write_atomic_durable(
        &lexical_sealed_identity_path(generation_dir),
        &bytes,
        "sealed generation identity",
    )
}

fn normalizer_unsupported(path: &Path, built_with: TextNormalizerVersion) -> CoreError {
    CoreError::Typed {
        code: "GENERATION_NORMALIZER_UNSUPPORTED".to_string(),
        message: format!(
            "lexical: sealed generation {} was built under text normalizer {built_with} (this build runs {TEXT_NORMALIZER_VERSION}); it must be rebuilt, never served with mismatched text semantics",
            path.display()
        ),
    }
}

/// The typed refusal for a sealed generation that is not what its
/// manifest committed to.
fn sidecar_corrupt(generation_dir: &Path, name: &str, reason: &str) -> CoreError {
    CoreError::Typed {
        code: GENERATION_SIDECAR_CORRUPT_CODE.to_string(),
        message: format!(
            "lexical: generation {} does not match its manifest: {name}: {reason}",
            generation_dir.display()
        ),
    }
}

fn read_lexical_sealed_identity(generation_dir: &Path) -> Result<GenerationSnapshot, CoreError> {
    let path = lexical_sealed_identity_path(generation_dir);
    let bytes = std::fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CoreError::Typed {
                code: "GENERATION_IDENTITY_INCOMPLETE".to_string(),
                message: format!(
                    "lexical: incomplete generation has no sealed identity at {}",
                    path.display()
                ),
            }
        } else {
            CoreError::Storage(format!(
                "lexical: read sealed generation identity {}: {error}",
                path.display()
            ))
        }
    })?;
    ciborium::from_reader(bytes.as_slice()).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: decode sealed generation identity {}: {error}",
            path.display()
        ))
    })
}

fn validate_lexical_sealed_identity(
    observed: &GenerationSnapshot,
    candidate: &GenerationSnapshot,
) -> Result<(), CoreError> {
    if observed != candidate {
        return Err(CoreError::Typed {
            code: "GENERATION_IDENTITY_DIGEST_MISMATCH".to_string(),
            message: format!(
                "lexical: durable generation identity mismatch for repo={} revision={} generation={}",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation.get(),
            ),
        });
    }
    Ok(())
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

/// Source-repo keyed repo-description authority.
///
/// One verbatim description string per `source_repo_id`, matched as a regex at
/// `repo:has.description(<pattern>)` query time. Distinct substrate from
/// [`RepoTopicShard`] (topic set) and the repo-meta key/value store so
/// description support never silently piggybacks on unrelated authority.
struct RepoDescriptionShard {
    descriptions_by_repo_id: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default)]
struct FileOwnershipShard {
    owners_by_repo_id: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

#[derive(Clone, Debug, Default)]
struct FileContributorShard {
    contributors_by_repo_id:
        BTreeMap<String, BTreeMap<String, BTreeSet<FileContributorIdentityEntry>>>,
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

/// The typed refusal for a keyword or phrase literal the token surfaces
/// cannot express; the code is the normalizer's, shared by every route.
fn map_text_query_error(err: &TextQueryError) -> CoreError {
    CoreError::Typed {
        code: err.code().to_string(),
        message: format!("lexical: {err}"),
    }
}

/// Tokenize a keyword or phrase literal for lowering, refusing typed when
/// it has no token or a run past the term cap.
fn text_query_tokens(text: &str, case: CaseMode) -> Result<Vec<normalize::Token>, CoreError> {
    normalize::query_tokens(text, case).map_err(|err| map_text_query_error(&err))
}

/// Lower a phrase-planner error to the typed keyword codes or a contract fault.
///
/// The planner's literal refusals share the keyword codes, since both leaves
/// lower through the same tokenizer; its other errors are contract faults of
/// the plan itself.
fn map_phrase_plan_error(err: PhrasePlannerError) -> CoreError {
    match err {
        PhrasePlannerError::EmptyPhrase => map_text_query_error(&TextQueryError::NoTokens),
        PhrasePlannerError::TokenTooLong { bytes, max } => {
            map_text_query_error(&TextQueryError::TokenTooLong { bytes, max })
        }
        other @ (PhrasePlannerError::TooFewTokens { .. }
        | PhrasePlannerError::UnsupportedSlop { .. }
        | PhrasePlannerError::UnsupportedField { .. }) => {
            CoreError::InvalidContract(format!("lexical: phrase plan: {other}"))
        }
    }
}

/// Fold one regex literal alternative for the `case:no` trigram prefilter.
///
/// An alternative is a byte prefix of some match. The extractor may have
/// cut it inside a multi-byte character, so only the longest well-formed
/// UTF-8 prefix is kept — a shorter prefix of a match is still a prefix —
/// and it is folded with the same per-char fold that built the folded copy.
/// An alternative with no well-formed prefix folds to nothing, which the
/// prefilter refuses as unusable rather than filtering anything away.
fn fold_literal_prefix(literal: &[u8]) -> Vec<u8> {
    let complete = literal
        .utf8_chunks()
        .next()
        .map_or("", |chunk| chunk.valid());
    normalize::fold(complete).into_bytes()
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

/// One text document as the index stores it: candidate id and the
/// text-authority doc id the sidecar shares with it.
struct IndexedTextDoc {
    candidate_id: String,
    doc_id: u64,
}

/// The text-authority doc id stored on a text document.
///
/// Every text document this adapter writes carries one; a text document
/// without it belongs to a generation built before doc ids were stored,
/// which the sealed-manifest format refuses. It is an error here too, so
/// an unsealed such generation cannot be continued into an authority
/// that cannot name what it retires.
fn stored_text_authority_doc_id(
    doc: &TantivyDocument,
    fields: &SchemaFields,
    candidate_id: &str,
) -> Result<u64, CoreError> {
    doc.get_first(fields.text_authority_doc_id)
        .and_then(|value| Value::as_u64(&value))
        .ok_or_else(|| CoreError::Typed {
            code: text_authority::TEXT_AUTHORITY_FORMAT_UNSUPPORTED_CODE.to_string(),
            message: format!(
                "lexical: text document {candidate_id} stores no text-authority doc id; the generation predates the sharded text authority and must be rebuilt"
            ),
        })
}

/// The text documents currently indexed under one path, with their doc ids.
///
/// Read from the committed index before a scope mutation is applied, so an
/// incremental text-authority update knows exactly which documents — and
/// therefore which shards — the mutation retires.
fn text_candidates_at_path(
    index: &Index,
    fields: &SchemaFields,
    repo_relative_path: &str,
) -> Result<Vec<IndexedTextDoc>, CoreError> {
    let reader: IndexReader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .map_err(|err| CoreError::Storage(format!("lexical: scope candidate reader: {err}")))?;
    reader.reload().map_err(|err| {
        CoreError::Storage(format!("lexical: scope candidate reader reload: {err}"))
    })?;
    let searcher = reader.searcher();
    let query = TermQuery::new(
        Term::from_field_text(fields.repo_relative_path, repo_relative_path),
        IndexRecordOption::Basic,
    );
    let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "lexical: num_docs overflow while collecting scope candidates: {err}"
        ))
    })?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let hits = searcher
        .search(&query, &TopDocs::with_limit(limit))
        .map_err(|err| CoreError::Storage(format!("lexical: scope candidate scan: {err}")))?;
    let mut candidates = Vec::with_capacity(hits.len());
    for (_score, doc_address) in hits {
        let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: fetch scope candidate doc {doc_address:?}: {err}"
            ))
        })?;
        if stored_text(&doc, fields.doc_kind).as_deref() != Some(TEXT_DOC_KIND) {
            continue;
        }
        let candidate_id = stored_text(&doc, fields.candidate_id).ok_or_else(|| {
            CoreError::Storage(
                "lexical: scope candidate doc missing candidate_id field".to_string(),
            )
        })?;
        let doc_id = stored_text_authority_doc_id(&doc, fields, &candidate_id)?;
        candidates.push(IndexedTextDoc {
            candidate_id,
            doc_id,
        });
    }
    Ok(candidates)
}

/// Every live text document with its doc id and stored text, in doc-id
/// order: the input of a full text-authority rebuild.
fn collect_text_authority_docs(
    index: &Index,
    fields: &SchemaFields,
) -> Result<Vec<AddedTextDoc>, CoreError> {
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
    let mut docs: Vec<AddedTextDoc> = Vec::new();
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
        let text = stored_text(&doc, fields.chunk_text).ok_or_else(|| {
            CoreError::Storage("lexical: text authority doc missing chunk_text field".to_string())
        })?;
        let doc_id = stored_text_authority_doc_id(&doc, fields, &candidate_id)?;
        docs.push(AddedTextDoc {
            doc_id,
            candidate_id,
            text,
        });
    }
    docs.sort_by_key(|doc| doc.doc_id);
    Ok(docs)
}

/// Hands out text-authority doc ids while a batch's ops are applied, and
/// keeps what it handed out so the sidecar update can add exactly the
/// documents the index received.
///
/// Ids continue from the watermark the plan read — the prior manifest's
/// `max_doc_id`, or the highest id the index stores when there is no prior
/// authority — and are never reused within the generation's chain, so a
/// batch's new documents share the last shard or open new ones.
struct TextDocAllocator {
    next_doc_id: u64,
    added: Vec<AddedTextDoc>,
}

impl TextDocAllocator {
    fn from_watermark(watermark: u64) -> Result<Self, CoreError> {
        Ok(Self {
            next_doc_id: watermark.checked_add(1).ok_or_else(|| {
                CoreError::InvalidContract("lexical: text authority doc id overflow".to_string())
            })?,
            added: Vec::new(),
        })
    }

    /// The id for one text document the batch writes.
    fn allocate(&mut self, candidate_id: &str, text: &str) -> Result<u64, CoreError> {
        let doc_id = self.next_doc_id;
        if doc_id > text_authority::MAX_DOC_ID {
            return Err(CoreError::InvalidContract(format!(
                "lexical: text authority doc id {doc_id} exceeds the encodable range"
            )));
        }
        self.next_doc_id = doc_id.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract("lexical: text authority doc id overflow".to_string())
        })?;
        self.added.push(AddedTextDoc {
            doc_id,
            candidate_id: candidate_id.to_string(),
            text: text.to_string(),
        });
        Ok(doc_id)
    }

    /// The watermark after every allocation so far.
    fn max_doc_id(&self) -> u64 {
        self.next_doc_id.saturating_sub(1)
    }
}

/// How a batch's text-authority impact is written after the commit.
enum TextAuthorityWrite {
    /// Nothing in the batch touched indexed text.
    None,
    /// Every shard is derived from a full scan of the live index: the
    /// generation has no prior authority yet, the batch clears the text
    /// surface or uses the legacy per-chunk ops, or the index names a
    /// retired document the prior authority never saw (a publish that
    /// crashed after its commit).
    Rebuild,
    /// The batch replaces or tombstones scopes only: the retired documents
    /// are removed from, and the added ones appended to, exactly the shards
    /// `touched_shards` names; every other shard is listed unchanged.
    Incremental {
        /// Retired doc id → candidate id, read from the pre-mutation index.
        retired: BTreeMap<u64, String>,
        touched_shards: BTreeSet<u64>,
    },
}

/// What a batch does to the text authority, decided before any op runs.
struct TextAuthorityPlan {
    write: TextAuthorityWrite,
    allocator: TextDocAllocator,
    prior: Option<TextAuthorityManifest>,
}

/// The text ops of a batch, classified.
struct TextOpSummary {
    /// Some op adds, replaces or retires text documents.
    touches_text: bool,
    /// Some op retires the text surface wholesale or uses the legacy
    /// per-chunk ops, whose retirements are not scope-addressed.
    forces_rebuild: bool,
    /// Paths whose text documents the batch retires.
    retired_paths: Vec<String>,
    /// Text documents the batch will write.
    added_count: u64,
}

fn summarize_text_ops(ops: &[LexicalChannelOp]) -> Result<TextOpSummary, CoreError> {
    let mut summary = TextOpSummary {
        touches_text: false,
        forces_rebuild: false,
        retired_paths: Vec::new(),
        added_count: 0,
    };
    for op in ops {
        match op {
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                summary.touches_text = true;
                let (_mode, _base, scope) = decode_replace_scope_payload(&payload.payload)?;
                summary
                    .retired_paths
                    .push(scope.scope.repo_relative_path.as_str().to_string());
                let chunks = u64::try_from(scope.chunks.len()).map_err(|err| {
                    CoreError::InvalidContract(format!("lexical: scope chunk count: {err}"))
                })?;
                summary.added_count = summary.added_count.saturating_add(chunks);
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                summary.touches_text = true;
                let (_mode, _base, scope) = decode_tombstone_scope_payload(&payload.payload)?;
                summary
                    .retired_paths
                    .push(scope.scope.repo_relative_path.as_str().to_string());
            }
            LexicalChannelOp::ClearLexicalSurface(payload) => {
                // Only the chunk surface holds text documents; clearing
                // symbols leaves the text authority as it is.
                if payload.surface == SearchScopeSurface::Chunk {
                    summary.touches_text = true;
                    summary.forces_rebuild = true;
                }
            }
            LexicalChannelOp::UpsertChunk(_) => {
                summary.touches_text = true;
                summary.forces_rebuild = true;
                summary.added_count = summary.added_count.saturating_add(1);
            }
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => {}
        }
    }
    Ok(summary)
}

/// The highest text-authority doc id the index stores, or 0 when it holds
/// no text document: the watermark when the authority cannot supply one.
///
/// A full scan, taken only on the rebuild paths that scan anyway.
fn stored_doc_id_watermark(index: &Index, fields: &SchemaFields) -> Result<u64, CoreError> {
    Ok(collect_text_authority_docs(index, fields)?
        .iter()
        .map(|doc| doc.doc_id)
        .max()
        .unwrap_or(0))
}

/// Classify a batch before it is applied and fix the doc ids it will hand
/// out, capturing the documents each scope mutation retires while the
/// pre-mutation index can still name them.
///
/// A generation without a prior authority, or whose index is ahead of it,
/// takes its watermark from the index itself (the highest stored doc id,
/// from a full scan), so a publish that crashed after its commit never
/// hands out an id twice.
fn plan_text_authority_delta(
    index: &Index,
    fields: &SchemaFields,
    ops: &[LexicalChannelOp],
    generation_dir: &Path,
) -> Result<TextAuthorityPlan, CoreError> {
    let summary = summarize_text_ops(ops)?;
    let prior = text_authority::read_manifest(generation_dir)?;
    if !summary.touches_text {
        let watermark = prior.as_ref().map_or(0, |manifest| manifest.max_doc_id);
        return Ok(TextAuthorityPlan {
            write: TextAuthorityWrite::None,
            allocator: TextDocAllocator::from_watermark(watermark)?,
            prior,
        });
    }
    let Some(manifest) = prior.as_ref() else {
        let watermark = stored_doc_id_watermark(index, fields)?;
        return Ok(TextAuthorityPlan {
            write: TextAuthorityWrite::Rebuild,
            allocator: TextDocAllocator::from_watermark(watermark)?,
            prior,
        });
    };
    let watermark = manifest.max_doc_id;
    let allocator = TextDocAllocator::from_watermark(watermark)?;
    if summary.forces_rebuild {
        return Ok(TextAuthorityPlan {
            write: TextAuthorityWrite::Rebuild,
            allocator,
            prior,
        });
    }
    let mut retired: BTreeMap<u64, String> = BTreeMap::new();
    for path in &summary.retired_paths {
        for candidate in text_candidates_at_path(index, fields, path)? {
            if candidate.doc_id > watermark {
                // The index holds a document the prior authority never
                // listed: a publish crashed between its commit and its
                // sidecar write. Only a full derivation can catch up, and
                // new ids must continue past what the index already
                // stores, not past the stale manifest.
                let watermark = stored_doc_id_watermark(index, fields)?.max(watermark);
                return Ok(TextAuthorityPlan {
                    write: TextAuthorityWrite::Rebuild,
                    allocator: TextDocAllocator::from_watermark(watermark)?,
                    prior,
                });
            }
            let _prior = retired.insert(candidate.doc_id, candidate.candidate_id);
        }
    }
    let mut touched_shards: BTreeSet<u64> = retired
        .keys()
        .map(|doc_id| shard_index_of(*doc_id))
        .collect();
    let first_new = allocator.next_doc_id;
    let last_new = watermark.checked_add(summary.added_count).ok_or_else(|| {
        CoreError::InvalidContract("lexical: text authority doc id overflow".to_string())
    })?;
    if summary.added_count > 0 {
        for shard in shard_index_of(first_new)..=shard_index_of(last_new) {
            let _inserted = touched_shards.insert(shard);
        }
    }
    Ok(TextAuthorityPlan {
        write: TextAuthorityWrite::Incremental {
            retired,
            touched_shards,
        },
        allocator,
        prior,
    })
}

fn lexical_delta_base_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(LEXICAL_DELTA_BASE_FILE_NAME)
}

/// The base generation this directory already carried forward, if any.
fn read_lexical_delta_base(generation_dir: &Path) -> Result<Option<ManifestGeneration>, CoreError> {
    let path = lexical_delta_base_path(generation_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "lexical: read delta base marker {}: {error}",
                path.display()
            )));
        }
    };
    let raw: u64 = ciborium::from_reader(bytes.as_slice()).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: decode delta base marker {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(ManifestGeneration::new(raw)))
}

fn persist_lexical_delta_base(
    generation_dir: &Path,
    base_generation: ManifestGeneration,
) -> Result<(), CoreError> {
    let mut bytes = Vec::new();
    ciborium::into_writer(&base_generation.get(), &mut bytes).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: encode delta base marker for generation {}: {error}",
            base_generation.get()
        ))
    })?;
    write_atomic_durable(
        &lexical_delta_base_path(generation_dir),
        &bytes,
        "delta base marker",
    )
}

/// Whether this directory already holds a materialized lexical index.
fn lexical_index_content_exists(generation_dir: &Path) -> bool {
    generation_dir.join(TANTIVY_INDEX_META_FILE_NAME).is_file()
}

/// Whether a generation-directory entry must be a private copy, not a link.
///
/// Tantivy rewrites these two under their existing names, so sharing the inode
/// would let one generation's commit mutate what another generation still
/// reads. Every other entry is either an immutable segment file or is replaced
/// by atomic rename (`write_atomic_durable`), both of which leave a hard link
/// pointing at the bytes it was created for.
fn is_generation_local_entry(file_name: &str) -> bool {
    matches!(
        file_name,
        TANTIVY_INDEX_META_FILE_NAME | TANTIVY_MANAGED_FILE_NAME
    )
}

/// Whether an entry belongs to a live writer and must not be inherited at all.
fn is_writer_lock_entry(file_name: &str) -> bool {
    file_name.starts_with(TANTIVY_LOCK_FILE_PREFIX)
}

/// Whether an entry is the base's seal (identity or content manifest) or
/// one of its scrub receipts.
///
/// A delta is unsealed until its own seal writes its own pair; inheriting
/// the base's would make a half-built delta claim the base's identity on
/// disk, which a crash before the seal would leave behind for the boot
/// scanner to refuse. The scrub receipts record a pass over, or a
/// corruption of, the base's committed bytes, which says nothing about
/// the delta.
fn is_seal_marker_entry(file_name: &str) -> bool {
    matches!(
        file_name,
        LEXICAL_SEALED_IDENTITY_FILE_NAME
            | LEXICAL_SEALED_MANIFEST_FILE_NAME
            | LEXICAL_SCRUB_RECEIPT_FILE_NAME
            | LEXICAL_QUARANTINE_RECEIPT_FILE_NAME
    )
}

/// Materializes one inherited entry: link the immutable ones, copy the rest.
fn inherit_generation_entry(source: &Path, target: &Path) -> Result<(), CoreError> {
    let file_name = source
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "lexical: base generation entry has no usable name: {}",
                source.display()
            ))
        })?;
    if is_generation_local_entry(file_name) {
        let _bytes_copied: u64 = std::fs::copy(source, target).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: copy generation-local entry {} -> {}: {err}",
                source.display(),
                target.display()
            ))
        })?;
        return Ok(());
    }
    // Both paths live under one state root, so they are always on one device.
    // A failure here is a real storage fault, not a reason to quietly fall back
    // to a full byte copy and drop the incremental guarantee without saying so.
    std::fs::hard_link(source, target).map_err(|err| {
        CoreError::Storage(format!(
            "lexical: link inherited entry {} -> {}: {err}",
            source.display(),
            target.display()
        ))
    })
}

/// Refuse a delta base this build could not serve.
///
/// A delta inherits the base's index and text-authority shards byte for
/// byte, so a sealed base must have been built under the current text
/// normalizer and the current text-authority format; the typed manifest
/// refusals propagate. An unsealed base has no manifest yet and is
/// admitted — its own seal stamps the current versions.
fn ensure_base_generation_is_servable(base_dir: &Path) -> Result<(), CoreError> {
    if sealed_generation::manifest_path(base_dir).is_file() {
        let _manifest = sealed_generation::read_manifest(base_dir)?;
    }
    Ok(())
}

/// Materializes `src` into `dst` without replacing anything already present.
///
/// Delta semantics: an authority this generation published for itself outranks
/// the base's copy of the same authority, so an existing destination entry
/// wins. Only entries the target does not have are inherited.
///
/// Inherited entries are hard-linked rather than copied, so a delta's write
/// cost is proportional to what it changes instead of to the size of its base
/// (QI-BB-006). Tantivy segment files are immutable across commits and the
/// sidecars are replaced by atomic rename, so a shared inode is only ever read
/// through, never written through. The two entries Tantivy does rewrite in
/// place are copied instead — see [`is_generation_local_entry`]. Gate evidence
/// for the immutability claim lives in
/// `docs/bugbash/sep-16/adr/G0-L-tantivy-snapshot-reuse.md`.
fn clone_generation_directory_preserving_existing(src: &Path, dst: &Path) -> Result<(), CoreError> {
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
            clone_generation_directory_preserving_existing(&entry_path, &target_path)?;
            continue;
        }
        if target_path.exists() {
            continue;
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| is_writer_lock_entry(name) || is_seal_marker_entry(name))
        {
            continue;
        }
        inherit_generation_entry(&entry_path, &target_path)?;
    }
    Ok(())
}

/// Tantivy-backed lexical adapter.
pub struct LexicalAdapter {
    state_root: PathBuf,
    fields: SchemaFields,
    writers: Arc<Mutex<WriterCache>>,
    regex_match_cache: Arc<Mutex<RegexMatchCache>>,
    /// How much text-authority work the adapter has done (QI-BB-006):
    /// rebuilds versus in-place updates, in documents derived and retired
    /// and in shards written and inherited.
    text_authority_updates: Arc<Mutex<TextAuthorityUpdateStats>>,
    /// What the adapter's seals read to commit their generations
    /// (QI-BB-006 보완 #4).
    seal_commitments: Arc<Mutex<LexicalSealCommitmentStats>>,
    /// Per-deployment regex policy injected at construction time.
    ///
    /// Owned by the adapter (not fabricated at the leaf call site) so all
    /// content-side regex leaves see the same dialect/literal/trigram-cap
    /// configuration. The defaults from [`RegexPolicy::defaults`] are fine
    /// for the in-tree configuration; operator-facing tightening is plumbed
    /// here rather than at the call site.
    regex_policy: RegexPolicy,
    /// Per-deployment cap on what one execution may materialize
    /// (QI-BB-005), threaded to every opened searcher.
    execution_budget: LexicalExecutionBudgetV1,
    /// Serializes the two things that act on a sealed generation's
    /// directory outside a query: an integrity-scrub step, which reads
    /// every committed file and writes its receipt there, and the removal
    /// of the directory (reclaim, quarantine discard). Without it a
    /// reclaim racing a scrub step makes the step meet files vanishing
    /// under it — a false corruption whose quarantine receipt lands in a
    /// directory being deleted. Under it a step either finds the
    /// generation whole or finds it gone, typed.
    directory_lifecycle: Mutex<()>,
}

impl LexicalAdapter {
    /// Construct an adapter rooted at the given directory with the default
    /// [`RegexPolicy`] and [`LexicalExecutionBudgetV1`]. The directory will be
    /// created lazily as generations are materialized.
    #[must_use]
    pub fn with_state_root(state_root: PathBuf) -> Self {
        Self::with_state_root_and_policies(
            state_root,
            RegexPolicy::defaults(),
            LexicalExecutionBudgetV1::DEFAULT,
            RegexMatchCachePolicy::DEFAULT,
            LexicalWriterPolicy::DEFAULT,
        )
    }

    /// Construct an adapter rooted at the given directory with explicit
    /// policies. Use this constructor when the deployment needs to tighten or
    /// relax the regex dialect / candidate cap / trigram-missing threshold
    /// defaults, the examined-candidate budget, or the writer envelope.
    #[must_use]
    pub fn with_state_root_and_policies(
        state_root: PathBuf,
        regex_policy: RegexPolicy,
        execution_budget: LexicalExecutionBudgetV1,
        regex_match_cache_policy: RegexMatchCachePolicy,
        writer_policy: LexicalWriterPolicy,
    ) -> Self {
        Self {
            state_root,
            fields: SchemaFields::build(),
            writers: Arc::new(Mutex::new(WriterCache::new(writer_policy))),
            regex_match_cache: Arc::new(Mutex::new(RegexMatchCache::new(regex_match_cache_policy))),
            text_authority_updates: Arc::new(Mutex::new(TextAuthorityUpdateStats::default())),
            seal_commitments: Arc::new(Mutex::new(LexicalSealCommitmentStats::default())),
            regex_policy,
            execution_budget,
            directory_lifecycle: Mutex::new(()),
        }
    }

    /// Hold the sealed-generation directory lifecycle (see the field).
    fn directory_lifecycle_guard(&self) -> Result<std::sync::MutexGuard<'_, ()>, CoreError> {
        self.directory_lifecycle.lock().map_err(|err| {
            CoreError::Storage(format!(
                "lexical: generation directory lifecycle lock poisoned: {err}"
            ))
        })
    }

    /// How much text-authority derivation the adapter has done so far
    /// (QI-BB-006).
    pub fn text_authority_update_stats(&self) -> Result<TextAuthorityUpdateStats, CoreError> {
        self.text_authority_updates
            .lock()
            .map(|stats| *stats)
            .map_err(|err| {
                CoreError::Storage(format!("lexical text authority stats poisoned: {err}"))
            })
    }

    /// Fold one sidecar write into the adapter's running totals.
    fn record_text_authority_write(
        &self,
        rebuilt: bool,
        receipt: TextAuthorityWriteReceipt,
    ) -> Result<(), CoreError> {
        let mut stats = self.text_authority_updates.lock().map_err(|err| {
            CoreError::Storage(format!("lexical text authority stats poisoned: {err}"))
        })?;
        if rebuilt {
            stats.rebuilds = stats.rebuilds.saturating_add(1);
        } else {
            stats.incremental_updates = stats.incremental_updates.saturating_add(1);
        }
        stats.docs_derived = stats.docs_derived.saturating_add(receipt.docs_derived);
        stats.docs_retired = stats.docs_retired.saturating_add(receipt.docs_retired);
        stats.shards_written = stats.shards_written.saturating_add(receipt.shards_written);
        stats.shards_inherited = stats
            .shards_inherited
            .saturating_add(receipt.shards_inherited);
        drop(stats);
        Ok(())
    }

    /// What the adapter's seals read and inherited so far
    /// (QI-BB-006 보완 #4).
    pub fn seal_commitment_stats(&self) -> Result<LexicalSealCommitmentStats, CoreError> {
        self.seal_commitments
            .lock()
            .map(|stats| *stats)
            .map_err(|err| CoreError::Storage(format!("lexical seal stats poisoned: {err}")))
    }

    /// Fold one seal's measurement into the adapter's running totals.
    fn record_seal_measurement(&self, seal: LexicalSealCommitmentStats) -> Result<(), CoreError> {
        self.seal_commitments
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical seal stats poisoned: {err}")))?
            .absorb(seal);
        Ok(())
    }

    /// What the regex match cache has done so far (QI-BB-024).
    pub fn regex_match_cache_stats(&self) -> Result<RegexMatchCacheStats, CoreError> {
        self.regex_match_cache
            .lock()
            .map(|cache| cache.stats())
            .map_err(|err| CoreError::Storage(format!("lexical regex cache poisoned: {err}")))
    }

    fn index_path(&self, key: &GenKey) -> PathBuf {
        GenerationStorageKeyV1::for_repo_revision(&key.repo_id, &key.revision_id)
            .generation_dir(&self.state_root, key.generation)
    }

    /// Make the sealed index final.
    ///
    /// Opens the writer (creating an empty index for a generation that
    /// indexed nothing, which must still be openable), commits once, waits
    /// for its merges so the sealed `meta.json` is the last one any writer
    /// produces, and drops the writer from the cache so nothing can commit
    /// to this generation again. A sealed generation is never built again,
    /// so its heap goes back to the envelope now.
    fn finalize_index_for_seal(&self, key: &GenKey) -> Result<(), CoreError> {
        let handle = self.writer_handle(key)?;
        drop(handle);
        self.writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?
            .release(key, WriterRelease::Seal)
    }

    fn writer_handle(&self, key: &GenKey) -> Result<Arc<Mutex<GenerationWriter>>, CoreError> {
        let path = self.index_path(key);
        let mut guard = self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
        guard.get_or_open(key, &self.fields, &path, Instant::now())
    }

    /// Commit and release every writer nothing has touched for the policy's
    /// idle interval (QI-BB-016); returns how many were released. The
    /// adapter sweeps after every batch, and the composition root's
    /// maintenance timer sweeps on its own schedule through
    /// [`WriterIdleSweepPort`].
    pub fn release_idle_writers(&self) -> Result<u64, CoreError> {
        self.writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?
            .release_idle(Instant::now())
    }

    /// Install the gate every new writer open is checked against
    /// (QI-BB-016); the adapter starts with the unbounded one.
    pub fn with_writer_admission(
        self,
        admission: Arc<dyn WriterAdmissionPort>,
    ) -> Result<Self, CoreError> {
        self.writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?
            .admission = admission;
        Ok(self)
    }

    /// What the writer cache holds and has done (QI-BB-016).
    pub fn writer_cache_stats(&self) -> Result<LexicalWriterCacheStats, CoreError> {
        Ok(self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?
            .stats())
    }

    fn invalidate_regex_match_cache_generation(&self, key: &GenKey) -> Result<(), CoreError> {
        self.regex_match_cache
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical regex cache poisoned: {err}")))?
            .invalidate_generation(key);
        Ok(())
    }

    /// Materializes the base generation this batch declares, exactly once.
    ///
    /// The recorded marker is the authority for "already carried forward", not
    /// directory existence: every per-generation authority sidecar creates the
    /// generation directory as a side effect, so a sidecar published before the
    /// lexical delta would otherwise skip the base clone and silently produce a
    /// generation holding only the delta.
    fn prepare_generation_for_ops(
        &self,
        key: &GenKey,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        let Some(requested_base) = declared_delta_base_generation(ops)? else {
            return Ok(());
        };
        let target_path = self.index_path(key);
        if let Some(recorded_base) = read_lexical_delta_base(&target_path)? {
            if recorded_base == requested_base {
                return Ok(());
            }
            return Err(CoreError::Typed {
                code: "DELTA_BASE_CONFLICT".to_string(),
                message: format!(
                    "lexical: generation {} already carried forward base {}; refusing a batch that declares base {}",
                    key.generation.get(),
                    recorded_base.get(),
                    requested_base.get()
                ),
            });
        }
        if lexical_index_content_exists(&target_path) {
            return Err(CoreError::Typed {
                code: "DELTA_BASE_UNRESOLVED".to_string(),
                message: format!(
                    "lexical: generation {} already holds index content with no recorded base; cannot prove base {} was carried forward",
                    key.generation.get(),
                    requested_base.get()
                ),
            });
        }
        let base_key = GenKey {
            repo_id: key.repo_id.clone(),
            revision_id: key.revision_id.clone(),
            generation: requested_base,
        };
        let base_path = self.index_path(&base_key);
        ensure_base_generation_is_servable(&base_path)?;
        clone_generation_directory_preserving_existing(&base_path, &target_path)?;
        persist_lexical_delta_base(&target_path, requested_base)
    }

    fn delete_scope_docs(&self, writer: &IndexWriter, repo_relative_path: &RepoRelativePath) {
        let term =
            Term::from_field_text(self.fields.repo_relative_path, repo_relative_path.as_str());
        let _opstamp = writer.delete_term(term);
    }

    fn clear_surface_docs(&self, writer: &IndexWriter, surface: SearchScopeSurface) -> bool {
        let doc_kind = match surface {
            SearchScopeSurface::Chunk => TEXT_DOC_KIND,
            SearchScopeSurface::Symbol => SYMBOL_DOC_KIND,
            // File and Module semantic rows have no lexical representation.
            SearchScopeSurface::File | SearchScopeSurface::Module => return false,
        };
        let term = Term::from_field_text(self.fields.doc_kind, doc_kind);
        let _opstamp = writer.delete_term(term);
        true
    }

    /// Apply an op that writes a generation-local overlay rather than the
    /// index. Returns `false` (nothing to commit) for every op it handles and
    /// for ops this adapter does not act on.
    ///
    /// The `FullBundle` payload is the repo-metadata overlay: a typed
    /// payload replaces the snapshot, an empty one removes it. Both land
    /// durably, and only before the seal.
    fn apply_snapshot_op(&self, key: &GenKey, op: &LexicalChannelOp) -> Result<bool, CoreError> {
        if let LexicalChannelOp::FullBundle(bundle) = op {
            let generation_dir = self.index_path(key);
            match decode_repo_metadata_payload(&bundle.payload)? {
                Some(metadata) => persist_overlay(
                    &generation_dir,
                    OverlayFamily::RepoMetadata,
                    &encode_repo_metadata_payload(&metadata)?,
                )?,
                None => remove_overlay(&generation_dir, OverlayFamily::RepoMetadata)?,
            }
        }
        Ok(false)
    }

    /// Publish one overlay family's snapshot into an unsealed generation.
    ///
    /// Every aux ingest route lands here: the batch is encoded by its
    /// family, written durably, and receipted with one accepted scope per
    /// entry. A sealed generation refuses the publish typed before any byte
    /// is written; its manifest committed to the overlays it has.
    fn publish_overlay(
        &self,
        key: &GenKey,
        family: OverlayFamily,
        bytes: &[u8],
        batch_digest: String,
        entries: usize,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let generation_dir = self.index_path(key);
        ensure_unsealed(&generation_dir, key.generation, family.file_name())?;
        persist_overlay(&generation_dir, family, bytes)?;
        let mut receipt = quanta_index_contract::BatchPublishReceipt::empty_for(
            key.generation,
            None,
            batch_digest,
        );
        for _entry in 0..entries {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }

    /// The generation directory a sealed candidate names, and the identity
    /// found there: the preamble every door and every scrub shares.
    fn sealed_generation_dir_for(
        &self,
        candidate: &GenerationSnapshot,
        what: &str,
    ) -> Result<(PathBuf, GenerationSnapshot), CoreError> {
        if candidate.track != SearchPlaneTrackKind::Lexical {
            return Err(CoreError::InvalidContract(format!(
                "lexical {what} received {:?} track",
                candidate.track
            )));
        }
        let key = GenKey {
            repo_id: candidate.repo_id.clone(),
            revision_id: candidate.revision_id.clone(),
            generation: candidate.manifest_generation,
        };
        let generation_dir = self.index_path(&key);
        if !generation_dir.is_dir() {
            return Err(CoreError::NotFound(format!(
                "lexical: generation directory is absent: {}",
                generation_dir.display()
            )));
        }
        let observed = read_lexical_sealed_identity(&generation_dir)?;
        validate_lexical_sealed_identity(&observed, candidate)?;
        Ok((generation_dir, observed))
    }

    /// Apply one op to the writer; `true` when something must be committed.
    ///
    /// Every text document written here takes its text-authority doc id
    /// from `allocator`, which also keeps the document for the sidecar
    /// update, so the index and the text authority agree on the id by
    /// construction.
    fn apply_op(
        &self,
        writer: &IndexWriter,
        key: &GenKey,
        op: &LexicalChannelOp,
        allocator: &mut TextDocAllocator,
    ) -> Result<bool, CoreError> {
        match op {
            LexicalChannelOp::UpsertChunk(upsert) => {
                let candidate_id = upsert.chunk_id.as_str();
                let term = Term::from_field_text(self.fields.candidate_id, candidate_id);
                let _opstamp = writer.delete_term(term);
                let chunk = decode_chunk_payload(&upsert.payload)?;
                let doc_id = allocator.allocate(candidate_id, chunk.text.as_ref())?;
                let mut doc = TantivyDocument::new();
                doc.add_u64(self.fields.text_authority_doc_id, doc_id);
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
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_replace_scope_payload(&payload.payload)?;
                self.delete_scope_docs(writer, &scope.scope.repo_relative_path);
                for chunk in &scope.chunks {
                    let doc_id =
                        allocator.allocate(chunk.chunk_id.as_str(), chunk.text.as_ref())?;
                    let mut doc = TantivyDocument::new();
                    doc.add_u64(self.fields.text_authority_doc_id, doc_id);
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
            LexicalChannelOp::ClearLexicalSurface(payload) => {
                Ok(self.clear_surface_docs(writer, payload.surface))
            }
            // FullBundle/Seal carry no document-level effect (dispatcher's
            // ledger update observes Seal).
            LexicalChannelOp::FullBundle(_) => self.apply_snapshot_op(key, op),
            LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => Ok(false),
        }
    }
}

/// The base generation declared by the first scope-bearing op in the batch.
///
/// `Ok(None)` means the batch replaces the generation outright and has no base
/// to inherit. Only the first scope-bearing op is consulted: a single batch
/// addresses one `(repo, revision, generation)` target and the dispatcher emits
/// one base per batch, so a later disagreement is a contract violation rather
/// than a second base to merge.
fn declared_delta_base_generation(
    ops: &[LexicalChannelOp],
) -> Result<Option<ManifestGeneration>, CoreError> {
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
            LexicalChannelOp::ClearLexicalSurface(payload) => payload.base_generation,
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertChunk(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => continue,
        };
        return Ok(base_generation);
    }
    Ok(None)
}

fn legacy_ops_for_batch(
    batch: &SearchCorpusIngestBatch,
    include_seal: bool,
) -> Result<Vec<LexicalChannelOp>, CoreError> {
    batch
        .validate_surface_mutations_v1()
        .map_err(|err| CoreError::InvalidContract(format!("lexical: {err}")))?;
    if batch.mode == BatchIngestMode::Delta && batch.base_generation.is_none() {
        return Err(CoreError::InvalidContract(
            "lexical: Delta search-corpus batch requires base_generation".to_string(),
        ));
    }
    let op_capacity = batch
        .bundle_payload
        .as_ref()
        .map_or(0usize, |_payload| 1usize)
        .saturating_add(
            batch
                .replace_scopes
                .len()
                .saturating_add(batch.clear_surfaces.len())
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
    for surface in &batch.clear_surfaces {
        ops.push(LexicalChannelOp::ClearLexicalSurface(ClearLexicalSurface {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            base_generation: batch.base_generation,
            surface: *surface,
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

/// The writer envelope and the regex match cache as scrape points,
/// `lexical_…` (QI-BB-015).
impl WriterIdleSweepPort for LexicalAdapter {
    fn sweep_idle_writers(&self) -> Result<u64, CoreError> {
        self.release_idle_writers()
    }
}

/// Every generation directory under the lexical track root, measured by
/// the same walker a reclaim and an open use (QI-BB-015).
impl TrackDiskUsagePort for LexicalAdapter {
    /// Bytes the lexical state root occupies on disk, by unique inode (a
    /// delta's hard-linked base segments count once), measured while
    /// ingest, seals and reclaims keep running.
    fn track_disk_bytes(&self) -> Result<u64, CoreError> {
        if !self.state_root.exists() {
            return Ok(0);
        }
        unique_inode_tree_bytes(
            std::slice::from_ref(&self.state_root),
            &is_writer_lock_entry,
        )
        .map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure state root {}: {err}",
                self.state_root.display()
            ))
        })
    }
}

impl MetricSourcePort for LexicalAdapter {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let writers = self.writer_cache_stats()?;
        let regex = self.regex_match_cache_stats()?;
        let text_authority = self.text_authority_update_stats()?;
        let seals = self.seal_commitment_stats()?;
        Ok(vec![
            MetricPointV1::counter("lexical_seals_total", seals.seals),
            MetricPointV1::counter("lexical_seal_files_hashed_total", seals.files_hashed),
            MetricPointV1::counter("lexical_seal_bytes_hashed_total", seals.bytes_hashed),
            MetricPointV1::counter("lexical_seal_files_inherited_total", seals.files_inherited),
            MetricPointV1::counter("lexical_seal_bytes_inherited_total", seals.bytes_inherited),
            MetricPointV1::counter(
                "lexical_text_authority_rebuilds_total",
                text_authority.rebuilds,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_incremental_updates_total",
                text_authority.incremental_updates,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_docs_derived_total",
                text_authority.docs_derived,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_docs_retired_total",
                text_authority.docs_retired,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_shards_written_total",
                text_authority.shards_written,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_shards_inherited_total",
                text_authority.shards_inherited,
            ),
            MetricPointV1::gauge_count(
                "lexical_writers_open",
                count_from_usize(writers.open_writers),
            ),
            MetricPointV1::gauge_count(
                "lexical_writers_max",
                count_from_usize(writers.max_writers),
            ),
            MetricPointV1::gauge_count(
                "lexical_writers_allocated_heap_bytes",
                writers.allocated_heap_bytes,
            ),
            MetricPointV1::counter("lexical_writer_lru_releases_total", writers.lru_releases),
            MetricPointV1::counter("lexical_writer_idle_releases_total", writers.idle_releases),
            MetricPointV1::counter("lexical_writer_seal_releases_total", writers.seal_releases),
            MetricPointV1::counter("lexical_regex_cache_hits_total", regex.hits),
            MetricPointV1::counter("lexical_regex_cache_misses_total", regex.misses),
            MetricPointV1::gauge_count(
                "lexical_regex_cache_entries",
                count_from_usize(regex.entries),
            ),
            MetricPointV1::gauge_count("lexical_regex_cache_resident_bytes", regex.resident_bytes),
            MetricPointV1::counter("lexical_regex_cache_evictions_total", regex.evictions),
            MetricPointV1::counter(
                "lexical_regex_cache_refused_cardinality_total",
                regex.refused_cardinality,
            ),
            MetricPointV1::counter(
                "lexical_regex_cache_refused_bytes_total",
                regex.refused_bytes,
            ),
        ])
    }
}

impl SearchCorpusBatchBuildPort for LexicalAdapter {
    fn build_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        let candidate = GenerationSnapshot {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: batch.generation,
            manifest_digest: batch.manifest_digest.clone(),
        };
        let generation_dir = self.index_path(&GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        });
        if lexical_sealed_identity_path(&generation_dir).exists() {
            if !batch.seal {
                return Err(CoreError::Typed {
                    code: GENERATION_IMMUTABLE_CODE.to_string(),
                    message: format!(
                        "lexical: generation {} is already sealed; refusing non-seal mutation",
                        batch.generation.get()
                    ),
                });
            }
            self.validate_generation_identity(&candidate)?;
            return Ok(());
        }
        let ops = legacy_ops_for_batch(batch, batch.seal)?;
        self.build(&batch.repo_id, &batch.revision_id, batch.generation, &ops)?;
        if batch.seal {
            // Finalize and retire the writer before measuring: a cached
            // writer would commit again on eviction and rewrite `meta.json`
            // behind the manifest. Then manifest first, identity last: the
            // identity's presence is the promotion point and implies a
            // durable manifest.
            let key = GenKey {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
            };
            self.finalize_index_for_seal(&key)?;
            let base_dir = read_lexical_delta_base(&generation_dir)?.map(|base| {
                self.index_path(&GenKey {
                    repo_id: key.repo_id.clone(),
                    revision_id: key.revision_id.clone(),
                    generation: base,
                })
            });
            let measured = seal_generation(
                &generation_dir,
                &self.fields,
                &candidate,
                base_dir.as_deref(),
            )?;
            self.record_seal_measurement(measured)?;
            persist_lexical_sealed_identity(&generation_dir, &candidate)?;
        }
        // Every batch is a chance to give an abandoned generation's heap back
        // to the envelope (QI-BB-016).
        let _released = self.release_idle_writers()?;
        Ok(())
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
        let bytes = encode_repo_commit_recency_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::CommitRecency,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
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
        let bytes = encode_repo_meta_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::Meta,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
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
        let bytes = encode_repo_topic_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::Topic,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
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
        let bytes = encode_repo_description_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::Description,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
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
        let bytes = encode_file_ownership_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::FileOwnership,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
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
        let bytes = encode_file_contributor_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::Contributor,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
    }
}

/// Whether an op changes the Tantivy index (and therefore needs a writer and
/// a commit), as opposed to writing a generation-local overlay file or
/// being a no-op for this adapter.
///
/// The classification decides whether a batch opens a writer; whether a
/// batch may land at all is [`op_writes_generation`]'s.
const fn op_mutates_index(op: &LexicalChannelOp) -> bool {
    match op {
        LexicalChannelOp::UpsertChunk(_)
        | LexicalChannelOp::UpsertSymbol(_)
        | LexicalChannelOp::ReplaceLexicalScope(_)
        | LexicalChannelOp::TombstoneLexicalScope(_)
        | LexicalChannelOp::ClearLexicalSurface(_) => true,
        LexicalChannelOp::FullBundle(_)
        | LexicalChannelOp::Seal(_)
        | LexicalChannelOp::UpsertCommit(_)
        | LexicalChannelOp::UpsertRef(_)
        | LexicalChannelOp::UpsertParseTree(_)
        | LexicalChannelOp::ReplaceStructuralScope(_) => false,
    }
}

/// Whether an op writes anything into the generation directory: the index
/// or the repo-metadata overlay. After the seal neither may land; the
/// remaining ops are no-ops for this adapter and stay no-ops.
const fn op_writes_generation(op: &LexicalChannelOp) -> bool {
    op_mutates_index(op) || matches!(op, LexicalChannelOp::FullBundle(_))
}

/// Refuse typed anything that would change a sealed generation.
fn ensure_unsealed(
    generation_dir: &Path,
    generation: ManifestGeneration,
    what: &str,
) -> Result<(), CoreError> {
    if lexical_sealed_identity_path(generation_dir).exists() {
        return Err(CoreError::Typed {
            code: GENERATION_IMMUTABLE_CODE.to_string(),
            message: format!(
                "lexical: generation {} is sealed; refusing to write {what} behind its sealed manifest",
                generation.get()
            ),
        });
    }
    Ok(())
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
        let mutates_index = ops.iter().any(op_mutates_index);
        if ops.iter().any(op_writes_generation) {
            ensure_unsealed(&self.index_path(&key), generation, "an index or overlay op")?;
        }
        self.prepare_generation_for_ops(&key, ops)?;
        if !mutates_index {
            // Overlay-only ops never touch the index, so they must not open
            // a writer: a writer left in the cache would commit on eviction
            // and rewrite `meta.json` under a sealed manifest.
            for op in ops {
                let _committed = self.apply_snapshot_op(&key, op)?;
            }
            return Ok(());
        }
        let handle = self.writer_handle(&key)?;
        self.commit_ops_under_lock(&handle, &key, ops)
    }
}

impl LexicalAdapter {
    /// The writer guard spans op-apply, commit, and the text-authority
    /// write so partial commits cannot interleave with sibling builds for
    /// the same generation; the text-authority write reads `guarded.index`
    /// after commit.
    fn commit_ops_under_lock(
        &self,
        handle: &Arc<Mutex<GenerationWriter>>,
        key: &GenKey,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        let mut guarded = handle
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writer poisoned: {err}")))?;
        let generation_dir = self.index_path(key);
        // Planned before any op runs: the retired documents are only
        // nameable while the pre-mutation index still holds them, and the
        // doc ids the ops store come from the plan's watermark.
        let mut plan =
            plan_text_authority_delta(&guarded.index, &self.fields, ops, &generation_dir)?;
        let mut needs_commit = false;
        for op in ops {
            if self.apply_op(&guarded.writer, key, op, &mut plan.allocator)? {
                needs_commit = true;
            }
        }
        if !needs_commit {
            return Ok(());
        }
        let _opstamp = guarded
            .writer
            .commit()
            .map_err(|err| CoreError::Storage(format!("lexical: commit: {err}")))?;
        // The text-authority write happens under the writer lock (it reads
        // the committed index); the accounting is folded in after the lock
        // is released so the stats lock is never nested inside the writer's.
        let written = self.write_text_authority(&generation_dir, key, &guarded.index, plan)?;
        drop(guarded);
        if let Some((rebuilt, receipt)) = written {
            self.record_text_authority_write(rebuilt, receipt)?;
        }
        Ok(())
    }

    /// Publish the text authority the plan decided on, after the commit.
    ///
    /// Returns whether it was a rebuild and what it did, or `None` when the
    /// batch touched no text.
    fn write_text_authority(
        &self,
        generation_dir: &Path,
        key: &GenKey,
        index: &Index,
        plan: TextAuthorityPlan,
    ) -> Result<Option<(bool, TextAuthorityWriteReceipt)>, CoreError> {
        let max_doc_id = plan.allocator.max_doc_id();
        match plan.write {
            TextAuthorityWrite::None => Ok(None),
            TextAuthorityWrite::Rebuild => {
                self.invalidate_regex_match_cache_generation(key)?;
                let docs = collect_text_authority_docs(index, &self.fields)?;
                let receipt = text_authority::rebuild(
                    generation_dir,
                    key.generation,
                    docs,
                    plan.prior.as_ref(),
                    max_doc_id,
                )?;
                Ok(Some((true, receipt)))
            }
            TextAuthorityWrite::Incremental {
                retired,
                touched_shards,
            } => {
                self.invalidate_regex_match_cache_generation(key)?;
                let Some(prior) = plan.prior.as_ref() else {
                    return Err(CoreError::InvalidContract(
                        "lexical: incremental text authority update planned without a prior manifest"
                            .to_string(),
                    ));
                };
                let receipt = text_authority::update(
                    generation_dir,
                    key.generation,
                    prior,
                    &retired,
                    &plan.allocator.added,
                    &touched_shards,
                    max_doc_id,
                )?;
                Ok(Some((false, receipt)))
            }
        }
    }
}

/// The open's visitor: keeps every proved and decoded file to become a
/// searcher.
#[derive(Default)]
struct LoadedGeneration {
    shards: Vec<(u64, ShardBody)>,
    repo_metadata: Option<LexicalRepoMetadataPayload>,
    repo_commit_recency: Option<RepoCommitRecencyShard>,
    repo_meta: Option<RepoMetaShard>,
    repo_topic: Option<RepoTopicShard>,
    repo_description: Option<RepoDescriptionShard>,
    file_ownership: Option<FileOwnershipShard>,
    file_contributor: Option<FileContributorShard>,
}

impl SealedGenerationVisitor for LoadedGeneration {
    fn text_authority_shard(&mut self, index: u64, body: ShardBody) -> Result<(), CoreError> {
        self.shards.push((index, body));
        Ok(())
    }

    fn overlay(&mut self, snapshot: OverlaySnapshot) -> Result<(), CoreError> {
        match snapshot {
            OverlaySnapshot::RepoMetadata(payload) => self.repo_metadata = Some(payload),
            OverlaySnapshot::CommitRecency(shard) => self.repo_commit_recency = Some(shard),
            OverlaySnapshot::Meta(shard) => self.repo_meta = Some(shard),
            OverlaySnapshot::Topic(shard) => self.repo_topic = Some(shard),
            OverlaySnapshot::Description(shard) => self.repo_description = Some(shard),
            OverlaySnapshot::FileOwnership(shard) => self.file_ownership = Some(shard),
            OverlaySnapshot::Contributor(shard) => self.file_contributor = Some(shard),
        }
        Ok(())
    }
}

/// The repo-metadata authorities a sealed manifest says the generation
/// carries: the explicit capability set a query's typed refusals are
/// answered from.
fn materialized_authorities(overlays: &[OverlayFamily]) -> RepoMetadataAuthoritiesV1 {
    overlays.iter().fold(
        RepoMetadataAuthoritiesV1::NONE,
        |set, family| match family {
            OverlayFamily::RepoMetadata => set,
            OverlayFamily::CommitRecency => set.with(RepoMetadataAuthorityV1::CommitRecency),
            OverlayFamily::Meta => set.with(RepoMetadataAuthorityV1::Meta),
            OverlayFamily::Topic => set.with(RepoMetadataAuthorityV1::Topic),
            OverlayFamily::Description => set.with(RepoMetadataAuthorityV1::Description),
            OverlayFamily::FileOwnership => set.with(RepoMetadataAuthorityV1::FileOwnership),
            OverlayFamily::Contributor => set.with(RepoMetadataAuthorityV1::Contributor),
        },
    )
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
        if !path.is_dir() {
            return Err(CoreError::NotFound(format!(
                "lexical: no index at {}",
                path.display()
            )));
        }
        let identity = read_lexical_sealed_identity(&path)?;
        if identity.repo_id != *repo
            || identity.revision_id != *revision
            || identity.track != SearchPlaneTrackKind::Lexical
            || identity.manifest_generation != generation
        {
            return Err(CoreError::Typed {
                code: "GENERATION_IDENTITY_SCOPE_MISMATCH".to_string(),
                message: format!(
                    "lexical: active generation identity scope disagrees with path {}",
                    path.display()
                ),
            });
        }
        self.open_sealed(&path, &identity)
    }

    fn open_proven(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let (generation_dir, observed) =
            self.sealed_generation_dir_for(candidate, "proven open")?;
        let searcher = self.open_sealed(&generation_dir, &observed)?;
        sync_generation_directory(&generation_dir)?;
        Ok(searcher)
    }
}

impl LexicalAdapter {
    /// Open the sealed generation at `path` whose identity is `identity`:
    /// the walk every door shares, keeping what it decodes.
    ///
    /// Every file a query decodes is read once, proved and decoded here;
    /// the index is opened from the proved commit and its segment files are
    /// proved present at their committed length.
    fn open_sealed(
        &self,
        path: &Path,
        identity: &GenerationSnapshot,
    ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
        let mut loaded = LoadedGeneration::default();
        let verified = walk_sealed_generation(path, identity, &mut loaded)?;
        let reader: IndexReader = verified
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|err| CoreError::Storage(format!("lexical: reader: {err}")))?;
        reader
            .reload()
            .map_err(|err| CoreError::Storage(format!("lexical: reader reload: {err}")))?;
        let text_authority = verified
            .manifest
            .text_authority
            .as_ref()
            .map(|_files| ShardedTextAuthority::from_proved_shards(loaded.shards))
            .transpose()?;
        let overlays: Vec<OverlayFamily> = verified
            .manifest
            .overlay_commitments()
            .map(|(family, _artifact)| family)
            .collect();
        let resident_bytes_estimate = resident_bytes_estimate(path, text_authority.as_ref())?;
        let artifact_identity = LexicalArtifactIdentityV1 {
            manifest_digest: verified.manifest.manifest_digest.clone(),
            normalizer: TextNormalizerVersionV1 {
                major: verified.manifest.normalizer.major,
                minor: verified.manifest.normalizer.minor,
            },
            repo_metadata: materialized_authorities(&overlays),
        };
        Ok(Box::new(TantivySearcher {
            repo_id: identity.repo_id.clone(),
            revision_id: identity.revision_id.clone(),
            generation: identity.manifest_generation,
            fields: self.fields.clone(),
            reader,
            repo_metadata: loaded.repo_metadata,
            regex_match_cache: Arc::clone(&self.regex_match_cache),
            regex_policy: self.regex_policy,
            execution_budget: self.execution_budget,
            text_authority,
            repo_commit_recency: loaded.repo_commit_recency,
            repo_meta: loaded.repo_meta,
            repo_topic: loaded.repo_topic,
            repo_description: loaded.repo_description,
            file_ownership: loaded.file_ownership,
            file_contributor: loaded.file_contributor,
            resident_bytes_estimate,
            artifact_identity,
        }))
    }
}

/// Make the generation directory's entries durable before a door admits it.
fn sync_generation_directory(generation_dir: &Path) -> Result<(), CoreError> {
    File::open(generation_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            CoreError::Storage(format!(
                "lexical: revalidate generation-directory durability {}: {error}",
                generation_dir.display()
            ))
        })
}

impl GenerationIdentityValidatePort for LexicalAdapter {
    fn validate_generation_identity(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<(), CoreError> {
        let (generation_dir, observed) =
            self.sealed_generation_dir_for(candidate, "identity validator")?;
        // The very walk a query's open runs, over the same manifest: every
        // decodable file read once, proved and decoded; the index opened
        // from the proved commit; the segment files proved present at their
        // committed length. What is admitted here is what a query can open.
        let _verified = walk_sealed_generation(&generation_dir, &observed, &mut DiscardingVisitor)?;
        sync_generation_directory(&generation_dir)
    }
}

impl IntegrityScrubPort for LexicalAdapter {
    fn scrub_candidates(&self) -> Result<Vec<IntegrityScrubCandidateV1>, CoreError> {
        inventory_sealed_generations(&self.state_root)?
            .sealed
            .into_iter()
            .map(|sealed| {
                let last_completed_unix = last_completed_scrub(&sealed.path, &sealed.identity)?;
                Ok(IntegrityScrubCandidateV1 {
                    identity: sealed.identity,
                    last_completed_unix,
                })
            })
            .collect()
    }

    fn scrub(
        &self,
        generation: &GenerationSnapshot,
        cursor: Option<IntegrityScrubCursorV1>,
        budget: IntegrityScrubBudgetV1,
    ) -> Result<IntegrityScrubReportV1, CoreError> {
        let _lifecycle = self.directory_lifecycle_guard()?;
        let (generation_dir, observed) = self.sealed_generation_dir_for(generation, "scrub")?;
        scrub_step(&generation_dir, &observed, cursor, budget)
    }
}

impl SealedGenerationScanPort for LexicalAdapter {
    fn inventory_sealed_generations(&self) -> Result<SealedGenerationInventoryV1, CoreError> {
        inventory_sealed_generations(&self.state_root)
    }
}

impl QuarantinedGenerationDiscardPort for LexicalAdapter {
    fn discard_quarantined_generation(
        &self,
        entry: &QuarantinedGenerationV1,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        if entry.track != SearchPlaneTrackKind::Lexical {
            return Err(CoreError::InvalidContract(format!(
                "lexical quarantine discard received {:?} track",
                entry.track
            )));
        }
        let _lifecycle = self.directory_lifecycle_guard()?;
        discard_quarantined_directory(
            &self.state_root,
            &inventory_sealed_generations(&self.state_root)?.quarantined,
            entry,
        )
    }
}

impl IncompleteGenerationDiscardPort for LexicalAdapter {
    fn discard_incomplete_generation(
        &self,
        candidate: &GenerationSnapshot,
    ) -> Result<IncompleteGenerationDiscardOutcomeV1, CoreError> {
        if candidate.track != SearchPlaneTrackKind::Lexical {
            return Err(CoreError::InvalidContract(format!(
                "lexical incomplete-generation discard received {:?} track",
                candidate.track
            )));
        }
        let key = GenKey {
            repo_id: candidate.repo_id.clone(),
            revision_id: candidate.revision_id.clone(),
            generation: candidate.manifest_generation,
        };
        let generation_dir = self.index_path(&key);
        let mut writers = self
            .writers
            .lock()
            .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
        if !generation_dir.exists() {
            let _stale_writer = writers.remove(&key);
            return Ok(IncompleteGenerationDiscardOutcomeV1::Absent);
        }
        if lexical_sealed_identity_path(&generation_dir).exists() {
            let observed = read_lexical_sealed_identity(&generation_dir)?;
            validate_lexical_sealed_identity(&observed, candidate)?;
            return Err(CoreError::Typed {
                code: "GENERATION_IMMUTABLE".to_string(),
                message: format!(
                    "lexical: refusing to discard sealed generation {}",
                    candidate.manifest_generation.get()
                ),
            });
        }
        let writer = writers.remove(&key);
        let writer_guard = writer
            .as_ref()
            .map(|handle| {
                handle.lock().map_err(|err| {
                    CoreError::Storage(format!("lexical writer poisoned during discard: {err}"))
                })
            })
            .transpose()?;
        std::fs::remove_dir_all(&generation_dir).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: discard incomplete generation {}: {error}",
                generation_dir.display()
            ))
        })?;
        drop(writer_guard);
        drop(writer);
        drop(writers);
        self.invalidate_regex_match_cache_generation(&key)?;
        Ok(IncompleteGenerationDiscardOutcomeV1::Discarded)
    }
}

impl SealedGenerationReclaimPort for LexicalAdapter {
    fn reclaim_sealed_generation(
        &self,
        retired: &GenerationSnapshot,
    ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
        if retired.track != SearchPlaneTrackKind::Lexical {
            return Err(CoreError::InvalidContract(format!(
                "lexical sealed-generation reclaim received {:?} track",
                retired.track
            )));
        }
        let key = GenKey {
            repo_id: retired.repo_id.clone(),
            revision_id: retired.revision_id.clone(),
            generation: retired.manifest_generation,
        };
        let generation_dir = self.index_path(&key);
        let _lifecycle = self.directory_lifecycle_guard()?;
        if !generation_dir.exists() {
            return Ok(SealedGenerationReclaimOutcomeV1::Absent);
        }
        // Only a sealed generation is this port's to remove; an unsealed
        // directory belongs to the incomplete-generation protocol.
        if !lexical_sealed_identity_path(&generation_dir).exists() {
            return Err(CoreError::Typed {
                code: "GENERATION_NOT_SEALED".to_string(),
                message: format!(
                    "lexical: refusing to reclaim unsealed generation {} as retired history",
                    retired.manifest_generation.get()
                ),
            });
        }
        let observed = read_lexical_sealed_identity(&generation_dir)?;
        validate_lexical_sealed_identity(&observed, retired)?;
        let bytes = generation_tree_bytes(&generation_dir)?;
        // A sealed generation has no live writer, but a stale handle from an
        // earlier attempt must not outlive the directory.
        {
            let mut writers = self
                .writers
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical writers poisoned: {err}")))?;
            let _stale_writer = writers.remove(&key);
        }
        std::fs::remove_dir_all(&generation_dir).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: reclaim sealed generation {}: {error}",
                generation_dir.display()
            ))
        })?;
        if let Some(parent) = generation_dir.parent() {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "lexical: fsync pair directory {} after reclaim: {error}",
                        parent.display()
                    ))
                })?;
        }
        self.invalidate_regex_match_cache_generation(&key)?;
        Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes })
    }

    fn sealed_generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<GenerationSnapshot>, CoreError> {
        let pair_dir = self
            .state_root
            .join(GenerationStorageKeyV1::for_repo_revision(repo_id, revision_id).as_str());
        let mut out = Vec::new();
        if !pair_dir.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&pair_dir).map_err(|error| {
            CoreError::Storage(format!("lexical: list {}: {error}", pair_dir.display()))
        })? {
            let entry = entry.map_err(|error| {
                CoreError::Storage(format!("lexical: read generation entry: {error}"))
            })?;
            let generation_dir = entry.path();
            if !generation_dir.is_dir() || !lexical_sealed_identity_path(&generation_dir).exists() {
                continue;
            }
            let identity = read_lexical_sealed_identity(&generation_dir)?;
            if identity.repo_id != *repo_id
                || identity.revision_id != *revision_id
                || identity.track != SearchPlaneTrackKind::Lexical
                || self.index_path(&GenKey {
                    repo_id: identity.repo_id.clone(),
                    revision_id: identity.revision_id.clone(),
                    generation: identity.manifest_generation,
                }) != generation_dir
            {
                return Err(CoreError::Typed {
                    code: "GENERATION_IDENTITY_SCOPE_MISMATCH".to_string(),
                    message: format!(
                        "lexical: persisted identity does not own physical path {}",
                        generation_dir.display()
                    ),
                });
            }
            out.push(identity);
        }
        out.sort_by_key(|identity| identity.manifest_generation);
        Ok(out)
    }

    /// Bytes the named generation directories occupy together, by unique
    /// inode: a delta's hard-linked base segments count once. Writer lock
    /// entries are not bytes of the generation.
    fn measure_sealed_generations(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        let mut roots = Vec::with_capacity(generations.len());
        let mut absent = BTreeSet::new();
        for generation in generations {
            let generation_dir = self.index_path(&GenKey {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                generation: *generation,
            });
            if generation_dir.is_dir() {
                roots.push(generation_dir);
            } else {
                let _new = absent.insert(*generation);
            }
        }
        let bytes = unique_inode_tree_bytes(&roots, &is_writer_lock_entry).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure sealed generations of repo={} revision={}: {err}",
                repo_id.as_str(),
                revision_id.as_str()
            ))
        })?;
        Ok(SealedGenerationBytesV1 { bytes, absent })
    }
}

/// What an opened handle keeps resident.
///
/// Every mapped or decoded file under the generation directory except the
/// text-authority sidecars (each inode counted once), plus the decoded text
/// authority's heap estimate in their place.
fn resident_bytes_estimate(
    generation_dir: &Path,
    text_authority: Option<&ShardedTextAuthority>,
) -> Result<u64, CoreError> {
    let skip = |name: &str| is_writer_lock_entry(name) || name == TEXT_AUTHORITY_DIR_NAME;
    let mapped =
        unique_inode_tree_bytes(&[generation_dir.to_path_buf()], &skip).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure resident bytes of {}: {err}",
                generation_dir.display()
            ))
        })?;
    let decoded = text_authority.map_or(0, ShardedTextAuthority::heap_bytes_estimate);
    Ok(mapped.saturating_add(decoded))
}

/// Sum of regular-file sizes under `root`, recursively.
///
/// What a reclaim or a quarantine discard reports giving back. A file
/// hard-linked into another generation counts here too; the disk frees it
/// when its last link goes. Writer lock files are transient and excluded.
fn generation_tree_bytes(root: &Path) -> Result<u64, CoreError> {
    let mut total = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure generation dir {}: {err}",
                directory.display()
            ))
        })?;
        for entry in entries {
            let entry = entry.map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: measure generation entry in {}: {err}",
                    directory.display()
                ))
            })?;
            if is_writer_lock_entry(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let metadata = entry.metadata().map_err(|err| {
                CoreError::Storage(format!(
                    "lexical: measure generation entry {}: {err}",
                    entry.path().display()
                ))
            })?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

/// Inventory the sealed generations under a lexical state root (QI-BB-026).
///
/// Reads each generation's sealed identity and nothing else: no sidecar is
/// hashed and no index is opened, so the cost is one small file per sealed
/// generation. A directory that is not a canonical family or `g<N>`, an
/// identity that cannot be read or decoded, and an identity that does not
/// own its directory are each reported as quarantined with the path and the
/// reason, and boot continues without them. Generations without a sealed
/// identity are in-progress builds and are skipped silently, as before. Only
/// a directory listing that fails is an error.
pub fn inventory_sealed_generations(
    lexical_root: &Path,
) -> Result<SealedGenerationInventoryV1, CoreError> {
    let mut inventory = SealedGenerationInventoryV1::default();
    if !lexical_root.exists() {
        return Ok(inventory);
    }
    for family_entry in std::fs::read_dir(lexical_root).map_err(|error| {
        CoreError::Storage(format!("lexical: list {}: {error}", lexical_root.display()))
    })? {
        let family_entry = family_entry.map_err(|error| {
            CoreError::Storage(format!("lexical: read generation-family entry: {error}"))
        })?;
        if !family_entry
            .file_type()
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: inspect {}: {error}",
                    family_entry.path().display()
                ))
            })?
            .is_dir()
        {
            continue;
        }
        let family_name = family_entry.file_name();
        if !family_name
            .to_str()
            .is_some_and(GenerationStorageKeyV1::is_canonical_name)
        {
            inventory.quarantined.push(quarantine(
                family_entry.path(),
                GenerationQuarantineReasonV1::NonCanonicalLayout,
                "directory is not a canonical generation family; it needs explicit migration"
                    .to_string(),
            ));
            continue;
        }
        for generation_entry in std::fs::read_dir(family_entry.path()).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: list {}: {error}",
                family_entry.path().display()
            ))
        })? {
            let generation_entry = generation_entry.map_err(|error| {
                CoreError::Storage(format!("lexical: read generation entry: {error}"))
            })?;
            if !generation_entry
                .file_type()
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "lexical: inspect {}: {error}",
                        generation_entry.path().display()
                    ))
                })?
                .is_dir()
            {
                continue;
            }
            let generation_dir = generation_entry.path();
            match inventory_generation_dir(lexical_root, &generation_dir) {
                // A generation the scrub proved corrupt is quarantined by its
                // receipt, not served (QI-BB-017, QI-BB-026).
                Ok(Some(identity)) => match quarantined_by_scrub(&generation_dir)? {
                    Some(quarantined) => inventory.quarantined.push(quarantined),
                    None => inventory.sealed.push(InventoriedSealedGenerationV1 {
                        identity,
                        path: generation_dir,
                    }),
                },
                Ok(None) => {}
                Err(quarantined) => inventory.quarantined.push(quarantined),
            }
        }
    }
    Ok(inventory)
}

/// Remove `entry.path` if `quarantined_now` — the track's inventory taken
/// this instant — names it under the same reason (QI-BB-026).
///
/// The inventory built every quarantined path from a directory walk under
/// `track_root`, so a path it names is under the root by construction; the
/// containment check below is the belt to that brace. Anything the
/// inventory does not name now is refused typed, never removed: a
/// directory repaired or sealed since the caller listed it stays.
fn discard_quarantined_directory(
    track_root: &Path,
    quarantined_now: &[QuarantinedGenerationV1],
    entry: &QuarantinedGenerationV1,
) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
    let not_quarantined = |why: String| CoreError::Typed {
        code: QUARANTINE_TARGET_NOT_QUARANTINED_CODE.to_string(),
        message: format!(
            "lexical: refusing to discard {}: {why}",
            entry.path.display()
        ),
    };
    let Some(current) = quarantined_now
        .iter()
        .find(|quarantined| quarantined.path == entry.path)
    else {
        if std::fs::symlink_metadata(&entry.path).is_ok() {
            return Err(not_quarantined(
                "the path is not quarantined now; a sealed, in-progress or repaired directory is not this port's to remove"
                    .to_string(),
            ));
        }
        return Ok(QuarantineDiscardOutcomeV1::Absent);
    };
    if current.reason != entry.reason {
        return Err(not_quarantined(format!(
            "it is quarantined as {} now, not {} as listed; list again",
            current.reason.as_code_str(),
            entry.reason.as_code_str()
        )));
    }
    if !entry.path.starts_with(track_root) {
        return Err(not_quarantined(format!(
            "the path is outside the track root {}",
            track_root.display()
        )));
    }
    let metadata = std::fs::symlink_metadata(&entry.path).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: inspect quarantined {}: {error}",
            entry.path.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Err(not_quarantined(
            "the path is not a directory; the inventory quarantines directories only".to_string(),
        ));
    }
    let bytes = generation_tree_bytes(&entry.path)?;
    std::fs::remove_dir_all(&entry.path).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: discard quarantined {}: {error}",
            entry.path.display()
        ))
    })?;
    if let Some(parent) = entry.path.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: fsync {} after discarding quarantine: {error}",
                    parent.display()
                ))
            })?;
    }
    Ok(QuarantineDiscardOutcomeV1::Discarded { bytes })
}

/// One generation directory's inventory outcome: `Ok(Some)` for a sealed
/// identity that owns the directory, `Ok(None)` for an in-progress build,
/// `Err` for a quarantine.
fn inventory_generation_dir(
    lexical_root: &Path,
    generation_dir: &Path,
) -> Result<Option<GenerationSnapshot>, QuarantinedGenerationV1> {
    let Some(generation_name) = generation_dir.file_name().and_then(|name| name.to_str()) else {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory name is not UTF-8".to_string(),
        ));
    };
    let canonical_generation_name = generation_name.strip_prefix('g').is_some_and(|raw| {
        raw.parse::<u64>()
            .is_ok_and(|generation| format!("g{generation}") == generation_name)
    });
    if !canonical_generation_name {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory is not `g<N>`; it needs explicit migration".to_string(),
        ));
    }
    if !lexical_sealed_identity_path(generation_dir).exists() {
        return Ok(None);
    }
    let identity = read_lexical_sealed_identity(generation_dir).map_err(|error| {
        quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
            error.to_string(),
        )
    })?;
    if identity.track != SearchPlaneTrackKind::Lexical
        || GenerationStorageKeyV1::for_repo_revision(&identity.repo_id, &identity.revision_id)
            .generation_dir(lexical_root, identity.manifest_generation)
            != generation_dir
    {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::ScopeMismatch,
            format!(
                "sealed identity names {:?} repo={} revision={} generation={}, which does not own this directory",
                identity.track,
                identity.repo_id.as_str(),
                identity.revision_id.as_str(),
                identity.manifest_generation.get()
            ),
        ));
    }
    Ok(Some(identity))
}

fn quarantine(
    path: PathBuf,
    reason: GenerationQuarantineReasonV1,
    detail: String,
) -> QuarantinedGenerationV1 {
    QuarantinedGenerationV1 {
        track: SearchPlaneTrackKind::Lexical,
        path,
        reason,
        detail,
    }
}

struct TantivySearcher {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    fields: SchemaFields,
    reader: IndexReader,
    repo_metadata: Option<LexicalRepoMetadataPayload>,
    regex_match_cache: Arc<Mutex<RegexMatchCache>>,
    /// Deployment-scoped regex policy threaded from the adapter at open time.
    /// Read at the regex-leaf compile site rather than fabricated there, so
    /// the dialect/literal/trigram-cap knobs are a single source of truth.
    regex_policy: RegexPolicy,
    /// Examined-candidate budget every exact-set execution runs under.
    execution_budget: LexicalExecutionBudgetV1,
    text_authority: Option<ShardedTextAuthority>,
    repo_commit_recency: Option<RepoCommitRecencyShard>,
    repo_meta: Option<RepoMetaShard>,
    repo_topic: Option<RepoTopicShard>,
    repo_description: Option<RepoDescriptionShard>,
    file_ownership: Option<FileOwnershipShard>,
    file_contributor: Option<FileContributorShard>,
    /// What this handle keeps resident, estimated at open: the index and
    /// snapshot files it maps or decoded (each inode once) plus the decoded
    /// text authority's heap footprint — not the authority's on-disk CBOR,
    /// which is smaller than the folded copies and expanded postings the
    /// handle actually holds.
    resident_bytes_estimate: u64,
    /// What this handle is, for the read view: the sealed manifest digest
    /// the open proved, the normalizer, and which of the six source-repo
    /// metadata snapshots above were present to decode.
    artifact_identity: LexicalArtifactIdentityV1,
}

struct PreparedPredicatePlan {
    expr: LqExpr,
    allowed_paths: Option<BTreeSet<String>>,
    allowed_repo_ids: Option<BTreeSet<String>>,
    allowed_candidate_ids: Option<BTreeSet<String>>,
    force_empty: bool,
}

/// What one bounded native collect produced.
struct CollectedHits {
    hits: Vec<(f32, DocAddress)>,
    /// Exact match count from the count collector, when the query asked.
    exact_total: Option<u64>,
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
    if name != LexicalPredicateV1::SymbolHasName.name() {
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
    /// The DSL's one case default (`LqOptions::case_mode`), read here.
    fn is_case_sensitive(options: &LqOptions) -> bool {
        Self::case_mode(options) == CaseMode::Sensitive
    }

    fn case_mode(options: &LqOptions) -> CaseMode {
        options.case_mode()
    }

    /// Candidates a page needs: `top_k`, or `min(top_k, N)` under `count:N`.
    ///
    /// `count:all` no longer widens the page (QI-BB-005): rows are the
    /// caller's `top_k` and the total is reported through the count
    /// collector instead of by materializing every match.
    fn page_limit(query: &LqQuery, requested: usize) -> usize {
        match query.options.count {
            Some(quanta_index_contract::LqCountBound::Bounded(bound)) => {
                usize::try_from(bound).map_or(requested, |bound| requested.min(bound))
            }
            Some(quanta_index_contract::LqCountBound::All) | None => requested,
        }
    }

    /// Whether an execution must see the whole match set to be exact: a
    /// projection collapses groups (a page cut before collapse would drop
    /// groups), and a bounded count orders the whole set before cutting.
    fn needs_whole_match_set(query: &LqQuery) -> bool {
        Self::projects_repo_surface(query)
            || Self::projects_path_surface(query)
            || Self::selects_file_projection(query)
            || matches!(
                query.options.count,
                Some(quanta_index_contract::LqCountBound::Bounded(_))
            )
    }

    fn wants_exact_total(query: &LqQuery) -> bool {
        query.options.count.is_some()
    }

    fn corpus_docs(searcher: &tantivy::Searcher, surface: &str) -> Result<usize, CoreError> {
        usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!("lexical: num_docs overflow in {surface}: {err}"))
        })
    }

    /// Run one native collect under the examined-candidate budget.
    ///
    /// A whole-set execution collects `min(num_docs, budget + 1)` so that an
    /// overrun is observable and refused; a page execution collects the page
    /// plus one continuation probe. When the query asks for a count, the
    /// exact total comes from the count collector in the same pass, which
    /// touches postings but materializes nothing. The collected hits are put
    /// in the total order the page contract promises; equal scores past the
    /// collected set are left to the index's own order, which is the
    /// documented limit of boundary determinism (plan §4.12).
    fn collect_bounded(
        &self,
        searcher: &tantivy::Searcher,
        compiled: &dyn Query,
        query: &LqQuery,
        page_limit: usize,
        whole_set: bool,
        surface: &str,
        budget: &RequestBudgetV1,
    ) -> Result<CollectedHits, CoreError> {
        let examined_budget = self.execution_budget.max_examined_candidates();
        let num_docs = Self::corpus_docs(searcher, surface)?;
        let collect_limit = if whole_set {
            num_docs.min(examined_budget.saturating_add(1))
        } else {
            page_limit
                .saturating_add(1)
                .min(num_docs)
                .min(examined_budget.saturating_add(1))
        };
        if collect_limit == 0 {
            return Ok(CollectedHits {
                hits: Vec::new(),
                exact_total: Self::wants_exact_total(query).then_some(0),
            });
        }
        // The request budget is observed inside the collect (W5 phase 2):
        // an interruption unwinds the native walk and answers typed at
        // `lexical:collect`.
        let (count, hits) = if Self::wants_exact_total(query) {
            let (count, hits) = budgeted_search(
                searcher,
                compiled,
                &(Count, TopDocs::with_limit(collect_limit)),
                budget,
                "lexical:collect",
            )?;
            (Some(count), hits)
        } else {
            let hits = budgeted_search(
                searcher,
                compiled,
                &TopDocs::with_limit(collect_limit),
                budget,
                "lexical:collect",
            )?;
            (None, hits)
        };
        if whole_set && hits.len() > examined_budget {
            return Err(self.execution_budget.exceeded(surface));
        }
        let exact_total = match count {
            Some(count) => Some(u64::try_from(count).map_err(|err| {
                CoreError::InvalidContract(format!("lexical: {surface} count overflow: {err}"))
            })?),
            None => None,
        };
        Ok(CollectedHits { hits, exact_total })
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

    /// Put the collected candidates in the one total order every lexical
    /// page follows (score, then path, lines, id) and cut the page.
    fn stabilize_and_cap_hits(
        mut hits: Vec<LexicalCandidate>,
        limit: usize,
    ) -> Vec<LexicalCandidate> {
        hits.sort_by(|left, right| {
            if Self::candidate_precedes(left, right) {
                std::cmp::Ordering::Less
            } else if Self::candidate_precedes(right, left) {
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

    fn uses_unindexed_scan(options: &LqOptions) -> bool {
        matches!(options.index_mode, Some(LqYesNoOnly::No))
    }

    #[expect(
        clippy::as_conversions,
        clippy::cast_precision_loss,
        reason = "boost_millis is a small score multiplier in milli-units; the u32->f32 widen then /1000.0 scales it back into a score factor with negligible precision impact for real boost magnitudes"
    )]
    fn boost_factor(options: &LqOptions) -> f32 {
        options
            .boost_millis
            .map_or(1.0, |millis| millis as f32 / 1_000.0)
    }

    fn apply_query_boost_score(score: f32, options: &LqOptions) -> f32 {
        score * Self::boost_factor(options)
    }

    /// Whether `haystack` holds the token sequence of `text`, on the
    /// `index:no` route.
    ///
    /// The same normalizer as the inverted index and the position sidecar:
    /// a keyword is its token sequence and a phrase is a contiguous run of
    /// it, so the scan answers exactly what the indexed route answers,
    /// including the typed refusal of token-less or over-long literals.
    fn manual_token_sequence_matches(
        text: &str,
        haystack: &str,
        case: CaseMode,
    ) -> Result<bool, CoreError> {
        let wanted = text_query_tokens(text, case)?;
        let present: Vec<normalize::Token> = normalize::tokenize(haystack, case)
            .indexable()
            .cloned()
            .collect();
        Ok(normalize::contains_phrase(&present, &wanted))
    }

    fn doc_content_text(&self, doc: &TantivyDocument) -> String {
        stored_text(doc, self.fields.chunk_text)
            .or_else(|| stored_text(doc, self.fields.snippet))
            .unwrap_or_default()
    }

    fn doc_language(&self, doc: &TantivyDocument, repo_relative_path: &str) -> Option<String> {
        stored_text(doc, self.fields.language).or_else(|| {
            language_from_path_hint(repo_relative_path).map(std::string::ToString::to_string)
        })
    }

    fn manual_filter_regex(
        &self,
        pattern: &str,
        filter_name: &str,
    ) -> Result<RegexExecutor, CoreError> {
        RegexExecutor::compile(pattern).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: {filter_name} regex filter compile: {err}"
            ))
        })
    }

    fn manual_regex_matches(
        &self,
        source: &str,
        options: &LqOptions,
        haystack: &str,
    ) -> Result<bool, CoreError> {
        let normalized_source = Self::regex_source_for_options(source, options);
        let executor =
            RegexExecutor::compile(&normalized_source).map_err(|err| CoreError::Typed {
                code: format!("LEX_REGEX_{}", err.code.as_code_str()),
                message: format!(
                    "lexical: regex {source:?} failed to compile on unindexed scan route: {err}"
                ),
            })?;
        Ok(executor.verify(haystack.as_bytes()))
    }

    fn manual_doc_restrictions_allow(
        &self,
        prepared: &PreparedPredicatePlan,
        candidate_id: &str,
        repo_id: &str,
        repo_relative_path: &str,
    ) -> bool {
        if prepared
            .allowed_candidate_ids
            .as_ref()
            .is_some_and(|ids| !ids.contains(candidate_id))
        {
            return false;
        }
        if prepared
            .allowed_repo_ids
            .as_ref()
            .is_some_and(|ids| !ids.contains(repo_id))
        {
            return false;
        }
        if prepared
            .allowed_paths
            .as_ref()
            .is_some_and(|paths| !paths.contains(repo_relative_path))
        {
            return false;
        }
        true
    }

    fn manual_repo_gate_matches(&self, repo_ids: &BTreeSet<String>, repo_id: &str) -> bool {
        repo_ids.contains(repo_id)
    }

    fn manual_file_owner_matches(
        &self,
        arg: &FileOwnerArg,
        source_repo_id: &str,
        repo_relative_path: &str,
    ) -> Result<bool, CoreError> {
        let authority = self.file_ownership_authority()?;
        let Some(owners) = authority
            .owners_by_repo_id
            .get(source_repo_id)
            .and_then(|by_path| by_path.get(repo_relative_path))
        else {
            return Ok(false);
        };
        Ok(arg
            .owner
            .as_ref()
            .map_or(!owners.is_empty(), |owner| owners.contains(owner)))
    }

    fn manual_file_contributor_matches(
        &self,
        arg: &FileContributorArg,
        source_repo_id: &str,
        repo_relative_path: &str,
    ) -> Result<bool, CoreError> {
        let authority = self.file_contributor_authority()?;
        let Some(contributors) = authority
            .contributors_by_repo_id
            .get(source_repo_id)
            .and_then(|by_path| by_path.get(repo_relative_path))
        else {
            return Ok(false);
        };
        match &arg.contributor {
            ContributorPattern::Exact(contributor) => Ok(contributors
                .iter()
                .any(|identity| identity.canonical == *contributor)),
            ContributorPattern::Regex(source) => {
                let executor = RegexExecutor::compile(source).map_err(|err| CoreError::Typed {
                    code: format!("LEX_REGEX_{}", err.code.as_code_str()),
                    message: format!(
                        "lexical: file.has.contributor regex {source:?} failed to compile: {err}"
                    ),
                })?;
                Ok(contributors.iter().any(|identity| {
                    identity
                        .name
                        .as_deref()
                        .is_some_and(|name| executor.verify(name.as_bytes()))
                        || identity
                            .email
                            .as_deref()
                            .is_some_and(|email| executor.verify(email.as_bytes()))
                }))
            }
        }
    }

    fn manual_file_filter_matches(
        &self,
        pattern: &str,
        scope: LqFileScope,
        repo_relative_path: &str,
    ) -> Result<bool, CoreError> {
        let executor = self.manual_filter_regex(pattern, "file")?;
        let path_match = executor.verify(repo_relative_path.as_bytes());
        Ok(match scope {
            LqFileScope::PathOnly => path_match,
            LqFileScope::NameOnly => file_name_for_path(repo_relative_path)
                .is_some_and(|name| executor.verify(name.as_bytes())),
            LqFileScope::NameAndPath => {
                path_match
                    || file_name_for_path(repo_relative_path)
                        .is_some_and(|name| executor.verify(name.as_bytes()))
            }
        })
    }

    fn manual_content_predicate_matches(
        &self,
        constraint: &ContentPredicateConstraint,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        if let Some(ContentPathScope { pattern, scope }) = constraint.path_scope.as_ref()
            && !self.manual_file_filter_matches(pattern, *scope, repo_relative_path)?
        {
            return Ok(false);
        }
        if let Some(language) = constraint.language.as_ref() {
            let Some(normalized) = normalize_language(language) else {
                return Err(CoreError::InvalidContract(
                    "lexical: scoped content predicate escaped with an empty lang value"
                        .to_string(),
                ));
            };
            if self
                .doc_language(&TantivyDocument::new(), repo_relative_path)
                .as_deref()
                != Some(normalized.as_str())
            {
                return Ok(false);
            }
        }
        let lowered = self.predicate_content_leaf_from_constraint(constraint);
        self.manual_leaf_matches(
            &lowered,
            options,
            source_repo_id,
            repo_relative_path,
            content,
            false,
            budget,
        )
    }

    fn manual_predicate_matches(
        &self,
        name: &str,
        args: &[LqPredicateArg],
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        let Some((canonical_name, canonical_args)) =
            self.canonicalize_predicate_call(name, args)?
        else {
            return Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            )));
        };
        match kind_of(&canonical_name) {
            Some(PredicateKind::RepoFileGate) => {
                let constraint = self.repo_has_file_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_file(&constraint, options, budget)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoContentGate) => {
                let leaf = self.repo_content_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_content(&leaf, options, budget)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoCommitRecencyGate) => {
                let timeref =
                    self.repo_commit_after_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_commit_after(&timeref)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoMetaGate) => {
                let arg = self.repo_meta_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_meta(&arg)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoTopicGate) => {
                let arg = self.repo_topic_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_topic(&arg)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::RepoDescriptionGate) => {
                let arg = self.repo_description_constraint(&canonical_name, &canonical_args)?;
                Ok(self.manual_repo_gate_matches(
                    &self.collect_repo_ids_for_repo_has_description(&arg)?,
                    source_repo_id,
                ))
            }
            Some(PredicateKind::FileOwnerGate) => {
                let arg = self.file_owner_constraint(&canonical_name, &canonical_args)?;
                self.manual_file_owner_matches(&arg, source_repo_id, repo_relative_path)
            }
            Some(PredicateKind::FileContributorGate) => {
                let arg = self.file_contributor_constraint(&canonical_name, &canonical_args)?;
                self.manual_file_contributor_matches(&arg, source_repo_id, repo_relative_path)
            }
            Some(PredicateKind::ContentLeaf) => {
                let constraint =
                    self.content_predicate_constraint(&canonical_name, &canonical_args)?;
                self.manual_content_predicate_matches(
                    &constraint,
                    options,
                    source_repo_id,
                    repo_relative_path,
                    content,
                    budget,
                )
            }
            None => Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{canonical_name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            ))),
        }
    }

    fn manual_leaf_matches(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        include_path_terms: bool,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        let case = Self::case_mode(options);
        match leaf {
            LqLeaf::Keyword(text) => {
                if options.pattern_type == LqPatternType::Regexp {
                    return self.manual_regex_matches(text, options, content);
                }
                Ok(Self::manual_token_sequence_matches(text, content, case)?
                    || (include_path_terms
                        && Self::manual_token_sequence_matches(text, repo_relative_path, case)?))
            }
            LqLeaf::Phrase(text) => Self::manual_token_sequence_matches(text, content, case),
            LqLeaf::RawString(text) => {
                if options.pattern_type == LqPatternType::Regexp {
                    return self.manual_regex_matches(text, options, content);
                }
                Ok(normalize::contains_substring(content, text, case))
            }
            LqLeaf::Regex(text) => self.manual_regex_matches(text, options, content),
            LqLeaf::StructuralBlock(_) => Err(CoreError::Typed {
                code: "STR_PRODUCER_PARSE_TREE_UNAVAILABLE".to_string(),
                message: "lexical: structural leaf cannot execute on the unindexed scan route"
                    .to_string(),
            }),
            LqLeaf::Predicate { name, args } => self.manual_predicate_matches(
                name,
                args,
                options,
                source_repo_id,
                repo_relative_path,
                content,
                budget,
            ),
        }
    }

    fn manual_expr_matches(
        &self,
        expr: &LqExpr,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        include_path_terms: bool,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        match expr {
            LqExpr::Empty => Ok(true),
            LqExpr::Leaf(leaf) => self.manual_leaf_matches(
                leaf,
                options,
                source_repo_id,
                repo_relative_path,
                content,
                include_path_terms,
                budget,
            ),
            LqExpr::All(children) => {
                for child in children {
                    if !self.manual_expr_matches(
                        child,
                        options,
                        source_repo_id,
                        repo_relative_path,
                        content,
                        false,
                        budget,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            LqExpr::Any(children) => {
                for child in children {
                    if self.manual_expr_matches(
                        child,
                        options,
                        source_repo_id,
                        repo_relative_path,
                        content,
                        false,
                        budget,
                    )? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            LqExpr::Not(inner) => Ok(!self.manual_expr_matches(
                inner,
                options,
                source_repo_id,
                repo_relative_path,
                content,
                false,
                budget,
            )?),
        }
    }

    fn manual_filter_matches(
        &self,
        filter: &LqFilter,
        options: &LqOptions,
        source_repo_id: &str,
        repo_relative_path: &str,
        content: &str,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        match filter {
            LqFilter::Repo { pattern, revs } => {
                if !revs.is_empty() {
                    return Err(CoreError::Typed {
                        code: crate::filters::codes::REV_UNAVAILABLE.to_string(),
                        message: "lexical: repo filter revisions require a history producer"
                            .to_string(),
                    });
                }
                let executor = self.manual_filter_regex(pattern, "repo")?;
                Ok(executor.verify(source_repo_id.as_bytes()))
            }
            LqFilter::File { pattern, scope } => {
                self.manual_file_filter_matches(pattern, *scope, repo_relative_path)
            }
            LqFilter::Content { leaf } => self.manual_leaf_matches(
                leaf,
                options,
                source_repo_id,
                repo_relative_path,
                content,
                false,
                budget,
            ),
            LqFilter::Lang { id } => {
                let Some(language) = normalize_language(id.as_str()) else {
                    return Err(CoreError::InvalidContract(
                        "lexical: lang filter value cannot be empty".to_string(),
                    ));
                };
                let doc_language = self.doc_language(&TantivyDocument::new(), repo_relative_path);
                Ok(
                    language_from_path_hint(repo_relative_path).or(doc_language.as_deref())
                        == Some(language.as_str()),
                )
            }
            LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => Ok(true),
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
            LqFilter::Type { .. } | LqFilter::Select { .. } => Err(CoreError::Typed {
                code: "LEX_FILTER_UNROUTED".to_string(),
                message: format!(
                    "lexical: type/select filters must be routed through doc-kind preparation, got `{filter:?}`"
                ),
            }),
        }
    }

    /// Explicit `index:no` execution: every document of the generation is
    /// fetched and matched in memory, so the corpus itself is the examined
    /// set and must fit the budget before the scan starts.
    fn manual_text_search(
        &self,
        query: &LqQuery,
        prepared: &PreparedExecutableQuery,
        constraints: &QueryConstraintSetV1,
        limit: usize,
        apply_select_projection: bool,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        let searcher = self.reader.searcher();
        let doc_limit = Self::corpus_docs(&searcher, "unindexed text scan")?;
        if doc_limit > self.execution_budget.max_examined_candidates() {
            return Err(self
                .execution_budget
                .exceeded("unindexed text scan (index:no)"));
        }
        if doc_limit == 0 {
            return Ok(LexicalSearchPageV1 {
                candidates: Vec::new(),
                exact_total: Self::wants_exact_total(query).then_some(0),
            });
        }
        let hits = budgeted_search(
            &searcher,
            &AllQuery,
            &TopDocs::with_limit(doc_limit),
            budget,
            "lexical:scan",
        )?;
        let boosted_score = Self::apply_query_boost_score(1.0, &query.options);
        let center_terms = snippet_center_terms(query);
        let mut out: Vec<LexicalCandidate> = Vec::new();
        // The scan matches every document against the plan itself; the
        // budget is observed between documents (W5 phase 2).
        let probe = BudgetProbe::new(budget);
        for (_score, doc_address) in hits {
            if probe.tick()
                && let Some(interruption) = probe.interruption_error("lexical:scan")
            {
                return Err(interruption);
            }
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if !self.manual_doc_matches(&doc, query, prepared, constraints, budget)? {
                continue;
            }
            out.push(self.document_to_candidate(&doc, boosted_score, &center_terms)?);
        }
        let out = if apply_select_projection {
            Self::collapse_select_projection(query, out)
        } else {
            out
        };
        // The scan visited every document, so the total is exact for free.
        let exact_total = Some(u64::try_from(out.len()).map_err(|err| {
            CoreError::InvalidContract(format!("lexical: unindexed scan total overflow: {err}"))
        })?);
        Ok(LexicalSearchPageV1 {
            candidates: Self::stabilize_and_cap_hits(out, limit),
            exact_total,
        })
    }

    /// Whether one stored document matches the unindexed-scan plan.
    ///
    /// The scan and the per-candidate explanation (QI-BB-022) share this so
    /// a candidate explains through exactly the matcher that ranked it.
    fn manual_doc_matches(
        &self,
        doc: &TantivyDocument,
        query: &LqQuery,
        prepared: &PreparedExecutableQuery,
        constraints: &QueryConstraintSetV1,
        budget: &RequestBudgetV1,
    ) -> Result<bool, CoreError> {
        if stored_text(doc, self.fields.doc_kind).as_deref() != Some(prepared.doc_kind.as_str()) {
            return Ok(false);
        }
        let Some(candidate_id) = stored_text(doc, self.fields.candidate_id) else {
            return Ok(false);
        };
        let Some(source_repo_id) = stored_text(doc, self.fields.repo_id) else {
            return Ok(false);
        };
        let Some(repo_relative_path) = stored_text(doc, self.fields.repo_relative_path) else {
            return Ok(false);
        };
        if !Self::manual_exact_path_allows(&repo_relative_path, constraints) {
            return Ok(false);
        }
        if !self.manual_doc_restrictions_allow(
            &prepared.predicate_plan,
            &candidate_id,
            &source_repo_id,
            &repo_relative_path,
        ) {
            return Ok(false);
        }
        let include_path_terms =
            Self::enables_path_term_surface(&prepared.predicate_plan.expr, &query.options);
        let content = self.doc_content_text(doc);
        if !self.manual_expr_matches(
            &prepared.predicate_plan.expr,
            &query.options,
            &source_repo_id,
            &repo_relative_path,
            &content,
            include_path_terms,
            budget,
        )? {
            return Ok(false);
        }
        for filter in &prepared.query.filters {
            if !self.manual_filter_matches(
                filter,
                &query.options,
                &source_repo_id,
                &repo_relative_path,
                &content,
                budget,
            )? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The one live document with `candidate_id` of `doc_kind`, by exact
    /// term lookup (QI-BB-022).
    ///
    /// Two live documents under one id would mean the writer's
    /// delete-then-add upsert did not hold; that is a corrupt index, not a
    /// choice to make here.
    fn locate_candidate(
        &self,
        searcher: &tantivy::Searcher,
        candidate_id: &str,
        doc_kind: &str,
    ) -> Result<Option<(DocAddress, TantivyDocument)>, CoreError> {
        let id_clause: Box<dyn Query> = Box::new(TermQuery::new(
            Term::from_field_text(self.fields.candidate_id, candidate_id),
            IndexRecordOption::Basic,
        ));
        let kind_clause: Box<dyn Query> = Box::new(TermQuery::new(
            Term::from_field_text(self.fields.doc_kind, doc_kind),
            IndexRecordOption::Basic,
        ));
        let lookup = BooleanQuery::new(vec![(Occur::Must, id_clause), (Occur::Must, kind_clause)]);
        let hits = searcher
            .search(&lookup, &TopDocs::with_limit(2))
            .map_err(|err| CoreError::Storage(format!("lexical: candidate lookup: {err}")))?;
        if hits.len() > 1 {
            return Err(CoreError::Storage(format!(
                "lexical: candidate id `{candidate_id}` names {} live documents",
                hits.len()
            )));
        }
        let Some((_score, doc_address)) = hits.into_iter().next() else {
            return Ok(None);
        };
        let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
            CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
        })?;
        Ok(Some((doc_address, doc)))
    }

    /// Score one live document through the compiled plan, exactly as the
    /// ranked collector would: the plan's scorer positioned on the document.
    fn score_one_document(
        searcher: &tantivy::Searcher,
        compiled: &dyn Query,
        doc_address: DocAddress,
    ) -> Result<Option<f32>, CoreError> {
        let weight = compiled
            .weight(EnableScoring::enabled_from_searcher(searcher))
            .map_err(|err| CoreError::Storage(format!("lexical: explain weight: {err}")))?;
        let reader = searcher.segment_reader(doc_address.segment_ord);
        let mut scorer = weight
            .scorer(reader, 1.0)
            .map_err(|err| CoreError::Storage(format!("lexical: explain scorer: {err}")))?;
        // A fresh scorer sits on its first matching document; `seek` only
        // moves forward, so a document before that first match is simply not
        // matched.
        let target = doc_address.doc_id;
        let first = scorer.doc();
        let landed = if first >= target {
            first
        } else {
            scorer.seek(target)
        };
        if landed != target {
            return Ok(None);
        }
        Ok(Some(scorer.score()))
    }

    fn manual_symbol_search(
        &self,
        query: &LqQuery,
        prepared: &PreparedExecutableQuery,
        constraints: &QueryConstraintSetV1,
        limit: usize,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        let searcher = self.reader.searcher();
        let doc_limit = Self::corpus_docs(&searcher, "unindexed symbol scan")?;
        if doc_limit > self.execution_budget.max_examined_candidates() {
            return Err(self
                .execution_budget
                .exceeded("unindexed symbol scan (index:no)"));
        }
        if doc_limit == 0 {
            return Ok(Vec::new());
        }
        let hits = budgeted_search(
            &searcher,
            &AllQuery,
            &TopDocs::with_limit(doc_limit),
            budget,
            "lexical:scan",
        )?;
        let include_path_terms =
            Self::enables_path_term_surface(&prepared.predicate_plan.expr, &query.options);
        let boosted_score = Self::apply_query_boost_score(1.0, &query.options);
        let center_terms = snippet_center_terms(query);
        let mut out: Vec<SymbolCandidate> = Vec::new();
        let probe = BudgetProbe::new(budget);
        for (_score, doc_address) in hits {
            if probe.tick()
                && let Some(interruption) = probe.interruption_error("lexical:scan")
            {
                return Err(interruption);
            }
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if stored_text(&doc, self.fields.doc_kind).as_deref()
                != Some(prepared.doc_kind.as_str())
            {
                continue;
            }
            let Some(candidate_id) = stored_text(&doc, self.fields.candidate_id) else {
                continue;
            };
            let Some(source_repo_id) = stored_text(&doc, self.fields.repo_id) else {
                continue;
            };
            let Some(repo_relative_path) = stored_text(&doc, self.fields.repo_relative_path) else {
                continue;
            };
            if !Self::manual_exact_path_allows(&repo_relative_path, constraints) {
                continue;
            }
            if !self.manual_doc_restrictions_allow(
                &prepared.predicate_plan,
                &candidate_id,
                &source_repo_id,
                &repo_relative_path,
            ) {
                continue;
            }
            let content = self.doc_content_text(&doc);
            if !self.manual_expr_matches(
                &prepared.predicate_plan.expr,
                &query.options,
                &source_repo_id,
                &repo_relative_path,
                &content,
                include_path_terms,
                budget,
            )? {
                continue;
            }
            let mut allowed = true;
            for filter in &prepared.query.filters {
                if !self.manual_filter_matches(
                    filter,
                    &query.options,
                    &source_repo_id,
                    &repo_relative_path,
                    &content,
                    budget,
                )? {
                    allowed = false;
                    break;
                }
            }
            if !allowed {
                continue;
            }
            out.push(self.document_to_symbol_candidate(&doc, boosted_score, &center_terms)?);
        }
        Ok(Self::stabilize_and_cap_symbol_hits(out, limit))
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
        let name_query = self.regex_text_query(self.fields.file_name, pattern)?;
        match scope {
            LqFileScope::PathOnly => Ok(path_query),
            LqFileScope::NameOnly => Ok(name_query),
            LqFileScope::NameAndPath => Ok(Box::new(BooleanQuery::new(vec![
                (Occur::Should, path_query),
                (Occur::Should, name_query),
            ]))),
        }
    }

    /// The fields a keyword is looked up in.
    ///
    /// Content, plus the tokenized path when the leaf surface allows path
    /// terms, in the case mode's analyzer.
    fn keyword_fields(&self, options: &LqOptions, include_path_terms: bool) -> Vec<Field> {
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
        fields
    }

    /// Lower a keyword leaf onto the inverted index.
    ///
    /// The literal is tokenized by the shared normalizer — never re-parsed
    /// by a second query grammar, so `-`, `:`, `^` and the like inside a
    /// keyword are boundaries, not operators. One token is a term query;
    /// several are a phrase query over the field's positions, which is what
    /// the position sidecar answers for the same text. A literal with no
    /// token, or with a run past the term cap, is refused typed.
    fn compile_keyword_leaf(
        &self,
        text: &str,
        options: &LqOptions,
        include_path_terms: bool,
    ) -> Result<Box<dyn Query>, CoreError> {
        let tokens = text_query_tokens(text, Self::case_mode(options))?;
        let mut per_field: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for field in self.keyword_fields(options, include_path_terms) {
            let query = Self::token_sequence_query(field, &tokens)
                .ok_or_else(|| map_text_query_error(&TextQueryError::NoTokens))?;
            per_field.push((Occur::Should, query));
        }
        if per_field.len() == 1
            && let Some((_, query)) = per_field.pop()
        {
            return Ok(query);
        }
        Ok(Box::new(BooleanQuery::new(per_field)))
    }

    /// A term query for one token, a phrase query for a sequence, nothing
    /// for an empty sequence.
    fn token_sequence_query(field: Field, tokens: &[normalize::Token]) -> Option<Box<dyn Query>> {
        let (first, rest) = tokens.split_first()?;
        let term = |token: &normalize::Token| Term::from_field_text(field, &token.text);
        if rest.is_empty() {
            return Some(Box::new(TermQuery::new(
                term(first),
                IndexRecordOption::WithFreqs,
            )));
        }
        Some(Box::new(PhraseQuery::new(
            tokens.iter().map(term).collect(),
        )))
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

    fn language_any_of_query(&self, constraints: &QueryConstraintSetV1) -> Box<dyn Query> {
        let mut clauses = Vec::with_capacity(constraints.language_any_of.len());
        for language in &constraints.language_any_of {
            clauses.push((
                Occur::Should,
                self.exact_text_query(self.fields.language, language.as_str()),
            ));
        }
        Box::new(BooleanQuery::new(clauses))
    }

    fn manual_exact_path_allows(
        repo_relative_path: &str,
        constraints: &QueryConstraintSetV1,
    ) -> bool {
        constraints
            .repo_relative_path_exact
            .as_ref()
            .is_none_or(|expected| expected.as_str() == repo_relative_path)
    }

    fn ensure_manual_scan_supports_constraints(
        constraints: &QueryConstraintSetV1,
        surface: &str,
    ) -> Result<(), CoreError> {
        if constraints.language_any_of.is_empty() {
            return Ok(());
        }
        Err(CoreError::NotImplemented(format!(
            "{surface}: typed language constraints require indexed execution; index:no cannot read the non-stored language field"
        )))
    }

    fn with_doc_kind_and_constraints(
        &self,
        query: Box<dyn Query>,
        doc_kind: &str,
        constraints: &QueryConstraintSetV1,
    ) -> Box<dyn Query> {
        let typed = self.with_doc_kind(query, doc_kind);
        if constraints.is_unconstrained() {
            return typed;
        }
        let mut clauses = vec![(Occur::Must, typed)];
        if !constraints.language_any_of.is_empty() {
            clauses.push((Occur::Must, self.language_any_of_query(constraints)));
        }
        if let Some(path) = &constraints.repo_relative_path_exact {
            clauses.push((
                Occur::Must,
                self.exact_text_query(self.fields.repo_relative_path, path.as_str()),
            ));
        }
        Box::new(BooleanQuery::new(clauses))
    }

    fn collect_matching_paths_for_leaf(
        &self,
        leaf: &LqLeaf,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        // Predicate scope collection (`file.contains` / `file.has.content`)
        // intentionally compiles with `LqPatternType::Standard` regardless of
        // the caller's pattern type: it is a path-discovery prelude, not a
        // user-facing leaf evaluation. The full caller options carry through
        // to the user-facing executor pass downstream.
        let scope_options = standard_pattern_options();
        let compiled = self.with_doc_kind(
            self.compile_leaf(leaf, &scope_options, false, budget)?,
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
        let hits = budgeted_search(
            &searcher,
            &*compiled,
            &TopDocs::with_limit(limit),
            budget,
            "lexical:predicate-scope",
        )?;
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

    fn text_authority(&self, feature: &str) -> Result<&ShardedTextAuthority, CoreError> {
        self.text_authority.as_ref().ok_or_else(|| CoreError::Typed {
            code: format!("{feature}_INDEX_MISSING"),
            message: format!(
                "lexical: {feature} execution requires a materialized text authority sidecar for this generation"
            ),
        })
    }

    fn repo_commit_recency_authority(&self) -> Result<&RepoCommitRecencyShard, CoreError> {
        self.repo_commit_recency.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::CommitRecency
                .unavailable_code()
                .to_string(),
            message: "lexical: repo.has.commit.after execution requires materialized source-repo commit recency authority for this generation".to_string(),
        })
    }

    fn repo_meta_authority(&self) -> Result<&RepoMetaShard, CoreError> {
        self.repo_meta.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::Meta.unavailable_code().to_string(),
            message: "lexical: repo.has.meta execution requires materialized source-repo repo metadata authority for this generation".to_string(),
        })
    }

    fn repo_topic_authority(&self) -> Result<&RepoTopicShard, CoreError> {
        self.repo_topic.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::Topic.unavailable_code().to_string(),
            message: "lexical: repo.has.topic execution requires materialized source-repo repo topic authority for this generation".to_string(),
        })
    }

    fn repo_description_authority(&self) -> Result<&RepoDescriptionShard, CoreError> {
        self.repo_description.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::Description
                .unavailable_code()
                .to_string(),
            message: "lexical: repo.has.description execution requires materialized source-repo repo description authority for this generation".to_string(),
        })
    }

    fn file_ownership_authority(&self) -> Result<&FileOwnershipShard, CoreError> {
        self.file_ownership.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::FileOwnership
                .unavailable_code()
                .to_string(),
            message: "lexical: file.has.owner execution requires materialized source-repo file ownership authority for this generation".to_string(),
        })
    }

    fn file_contributor_authority(&self) -> Result<&FileContributorShard, CoreError> {
        self.file_contributor.as_ref().ok_or_else(|| CoreError::Typed {
            code: RepoMetadataAuthorityV1::Contributor
                .unavailable_code()
                .to_string(),
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
        let trigram_index = authority.trigram_index(folded);
        let resolver = authority.resolver(folded);
        let needle = normalize::nfc(needle);
        let query_bytes = normalize::apply_case(needle.as_ref(), Self::case_mode(options))
            .into_owned()
            .into_bytes();
        let verified_doc_ids = query_raw_substring(&trigram_index, &query_bytes, &resolver)
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

    /// The regex source as executed.
    ///
    /// NFC-normalized as text (the documents are NFC, so a decomposed literal
    /// could never match), with the engine's own `(?i)` under `case:no`.
    fn regex_source_for_options(source: &str, options: &LqOptions) -> String {
        let source = normalize::nfc(source);
        if Self::is_case_sensitive(options) {
            return source.into_owned();
        }
        format!("(?i){source}")
    }

    fn regex_timeout_budget_ms(options: &LqOptions) -> Option<u64> {
        options.timeout_ms
    }

    fn regex_match_cache_key(&self, normalized_source: &str) -> RegexMatchCacheKey {
        RegexMatchCacheKey {
            generation: GenKey {
                repo_id: self.repo_id.clone(),
                revision_id: self.revision_id.clone(),
                generation: self.generation,
            },
            normalized_source: normalized_source.to_string(),
        }
    }

    fn collect_matching_candidate_ids_for_regex(
        &self,
        source: &str,
        options: &LqOptions,
        budget: &RequestBudgetV1,
    ) -> Result<Arc<BTreeSet<String>>, CoreError> {
        let normalized_source = Self::regex_source_for_options(source, options);
        let cache_key = options
            .timeout_ms
            .is_none()
            .then(|| self.regex_match_cache_key(&normalized_source));
        if let Some(cache_key) = cache_key.as_ref() {
            let mut cache = self.regex_match_cache.lock().map_err(|err| {
                CoreError::Storage(format!("lexical regex cache poisoned: {err}"))
            })?;
            // A hit shares the set; nothing is cloned per candidate.
            if let Some(cached) = cache.get(cache_key) {
                return Ok(cached);
            }
        }
        let authority = self.text_authority("LEX_REGEX_TRIGRAM")?;
        let plan = crate::regex::plan_regex(&normalized_source, options, &self.regex_policy)
            .map_err(map_regex_plan_error)?;
        let executor = RegexExecutor::compile(&normalized_source).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: regex plan/verify mismatch for {source:?}: {err}"
            ))
        })?;
        let folded = !Self::is_case_sensitive(options);
        let trigram_index = authority.trigram_index(folded);
        // The planner hands back an alternation, not a conjunction: a match
        // needs one of these literals. Case-insensitive patterns make that
        // concrete — `(?i)fresh` extracts `fresh` and `freſh` — so the prefilter
        // unions per alternative. AND-ing them filtered every document away.
        // Under `case:no` the folded trigram copy is searched, so every
        // alternative is folded with the same per-char fold that built it;
        // an alternative is a char-boundary prefix of some match, so its
        // fold is a prefix of the match's fold and the prefilter stays sound.
        let literal_alternation = if folded {
            plan.literal_alternation()
                .iter()
                .map(|literal| fold_literal_prefix(literal))
                .collect::<Vec<_>>()
        } else {
            plan.literal_alternation().to_vec()
        };
        let prefiltered_doc_ids = match regex_prefilter_any_of(&trigram_index, &literal_alternation)
        {
            Ok(doc_ids) => doc_ids,
            Err(err) if err.code == TrigramErrorCode::RegexPrefilterUnusable => {
                authority.doc_ids().map(TrigramDocId).collect()
            }
            Err(err) => return Err(map_trigram_error("regex prefilter", &err)),
        };
        let resolver = authority.resolver(false);
        let budget_ms = Self::regex_timeout_budget_ms(options).unwrap_or(0);
        if options.timeout_ms == Some(0) && !prefiltered_doc_ids.is_empty() {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::QueryTimeout.as_code_str().to_string(),
                message:
                    "lexical: regex verify timed out before candidate verification began (budget 0ms)"
                        .to_string(),
            });
        }
        // The request budget is observed between candidates (W5 phase 2);
        // the probe looks every interval so a large candidate set costs
        // nothing extra per document.
        let probe = BudgetProbe::new(budget);
        let verified_doc_ids = executor
            .execute_interruptible(&prefiltered_doc_ids, &resolver, budget_ms, &|| {
                probe.tick()
            })
            .map_err(|err| match err.code {
                quanta_index_lq_regex::RegexErrorCode::QueryTimeout => CoreError::Typed {
                    code: LexicalErrorCode::QueryTimeout.as_code_str().to_string(),
                    message: format!("lexical: regex verify timed out: {err}"),
                },
                quanta_index_lq_regex::RegexErrorCode::Interrupted => probe
                    .interruption_error("lexical:regex-verify")
                    .unwrap_or_else(|| {
                        CoreError::Storage(format!(
                            "lexical: regex verify reported an interruption the budget probe did not observe: {err}"
                        ))
                    }),
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
        let out = Arc::new(out);
        if let Some(cache_key) = cache_key {
            let mut cache = self.regex_match_cache.lock().map_err(|err| {
                CoreError::Storage(format!("lexical regex cache poisoned: {err}"))
            })?;
            // A result too wide for one entry is served but not kept; the
            // refusal is counted in the stats rather than logged.
            let _kept: Result<(), RegexMatchCacheRefusal> =
                cache.insert(cache_key, Arc::clone(&out));
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
        .map_err(map_phrase_plan_error)?;
        let positions_index = authority.positions_index(plan.case_sensitive);
        let terms = plan.tokens.iter().map(String::as_str).collect::<Vec<_>>();
        let matches = query_phrase(&positions_index, &terms)
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
        budget: &RequestBudgetV1,
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
                    self.compile_leaf(&leaf, options, false, budget)?
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
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        let compiled = self.repo_has_file_path_query(constraint, options, budget)?;
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting repo.has.file scope: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(BTreeSet::new());
        }
        let hits = budgeted_search(
            &searcher,
            &*compiled,
            &TopDocs::with_limit(limit),
            budget,
            "lexical:repo-scope",
        )?;
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
                "lexical: predicate leaf `{name}` only supports one content scalar plus optional file:/path:/name:/lang: scope filters (owner: {PREDICATE_OWNER})"
            )),
            ContentPredicateArgError::DuplicatePathScope => unimplemented_predicate(format!(
                "lexical: predicate leaf `{name}` only supports one file:/path:/name: scope filter (owner: {PREDICATE_OWNER})"
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
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn Query>, CoreError> {
        Ok(self.with_doc_kind(
            self.compile_leaf(leaf, options, false, budget)?,
            TEXT_DOC_KIND,
        ))
    }

    fn collect_repo_ids_for_repo_has_content(
        &self,
        leaf: &LqLeaf,
        options: &LqOptions,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        let compiled = self.repo_has_content_query(leaf, options, budget)?;
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting repo.has.content scope: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(BTreeSet::new());
        }
        let hits = budgeted_search(
            &searcher,
            &*compiled,
            &TopDocs::with_limit(limit),
            budget,
            "lexical:repo-scope",
        )?;
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
                let key = normalize_repo_meta_pattern(arg.key)?;
                let value = arg.value.map(normalize_repo_meta_pattern).transpose()?;
                Ok(RepoMetaArg {
                    key,
                    value,
                })
            })
            .map_err(|err| match err {
                RepoMetaArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one metadata argument (owner: {PREDICATE_OWNER})"
                )),
                RepoMetaArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports `key:value`, bare `key`, `tag:`, or slash-delimited regex key/value metadata shapes (owner: {PREDICATE_OWNER})"
                )),
                RepoMetaArgError::EmptyKey => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires a non-empty metadata key (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    fn compile_repo_meta_pattern(
        &self,
        field_name: &str,
        pattern: &MetaPattern,
    ) -> Result<Option<RegexExecutor>, CoreError> {
        let MetaPattern::Regex(source) = pattern else {
            return Ok(None);
        };
        RegexExecutor::compile(source)
            .map(Some)
            .map_err(|err| CoreError::Typed {
                code: format!("LEX_REGEX_{}", err.code.as_code_str()),
                message: format!(
                    "lexical: repo.has.meta {field_name} regex {source:?} failed to compile: {err}"
                ),
            })
    }

    fn repo_meta_pattern_matches(
        &self,
        pattern: &MetaPattern,
        executor: Option<&RegexExecutor>,
        candidate: &str,
    ) -> bool {
        match (pattern, executor) {
            (MetaPattern::Exact(expected), _) => candidate == expected,
            (MetaPattern::Regex(_), Some(executor)) => executor.verify(candidate.as_bytes()),
            (MetaPattern::Regex(_), None) => false,
        }
    }

    fn collect_repo_ids_for_repo_has_meta(
        &self,
        arg: &RepoMetaArg,
    ) -> Result<BTreeSet<String>, CoreError> {
        let authority = self.repo_meta_authority()?;
        let key_executor = self.compile_repo_meta_pattern("key", &arg.key)?;
        let value_executor = arg
            .value
            .as_ref()
            .map(|pattern| self.compile_repo_meta_pattern("value", pattern))
            .transpose()?
            .flatten();
        Ok(authority
            .meta_by_repo_id
            .iter()
            .filter(|(_, by_key)| {
                by_key.iter().any(|(key, value)| {
                    self.repo_meta_pattern_matches(&arg.key, key_executor.as_ref(), key)
                        && arg.value.as_ref().is_none_or(|pattern| {
                            self.repo_meta_pattern_matches(pattern, value_executor.as_ref(), value)
                        })
                })
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
        budget: &RequestBudgetV1,
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
        let hits = budgeted_search(
            &searcher,
            &AllQuery,
            &TopDocs::with_limit(limit),
            budget,
            "lexical:authority-scan",
        )?;
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
            .and_then(|arg| match arg.contributor {
                ContributorPattern::Exact(contributor) => {
                    let Some(contributor) = normalize_contributor_identity(&contributor) else {
                        return Err(FileContributorArgError::EmptyContributor);
                    };
                    Ok(FileContributorArg {
                        contributor: ContributorPattern::Exact(contributor),
                    })
                }
                ContributorPattern::Regex(source) => Ok(FileContributorArg {
                    contributor: ContributorPattern::Regex(source.to_ascii_lowercase()),
                }),
            })
            .map_err(|err| match err {
                FileContributorArgError::WrongArity => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` requires exactly one contributor identity argument (owner: {PREDICATE_OWNER})"
                )),
                FileContributorArgError::UnsupportedArg => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` only supports one keyword/phrase/raw-string contributor identity or `/.../` regex contributor pattern (owner: {PREDICATE_OWNER})"
                )),
                FileContributorArgError::EmptyContributor => unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` contributor identity cannot be empty (owner: {PREDICATE_OWNER})"
                )),
            })
    }

    fn collect_candidate_ids_for_file_has_contributor(
        &self,
        arg: &FileContributorArg,
        budget: &RequestBudgetV1,
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
        let contributor_regex = match &arg.contributor {
            ContributorPattern::Regex(source) => Some(RegexExecutor::compile(source).map_err(
                |err| CoreError::Typed {
                    code: format!("LEX_REGEX_{}", err.code.as_code_str()),
                    message: format!(
                        "lexical: file.has.contributor regex {source:?} failed to compile: {err}"
                    ),
                },
            )?),
            ContributorPattern::Exact(_) => None,
        };
        let hits = budgeted_search(
            &searcher,
            &AllQuery,
            &TopDocs::with_limit(limit),
            budget,
            "lexical:authority-scan",
        )?;
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
            let matches = match &arg.contributor {
                ContributorPattern::Exact(contributor) => contributors
                    .iter()
                    .any(|identity| identity.canonical == *contributor),
                ContributorPattern::Regex(_) => {
                    let Some(executor) = contributor_regex.as_ref() else {
                        return Err(CoreError::Storage(
                            "lexical: contributor regex executor missing for a compiled regex pattern"
                                .to_string(),
                        ));
                    };
                    contributors.iter().any(|identity| {
                        identity
                            .name
                            .as_deref()
                            .is_some_and(|name| executor.verify(name.as_bytes()))
                            || identity
                                .email
                                .as_deref()
                                .is_some_and(|email| executor.verify(email.as_bytes()))
                    })
                }
            };
            if matches {
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
        budget: &RequestBudgetV1,
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
        let hits = budgeted_search(
            &searcher,
            &*compiled,
            &TopDocs::with_limit(limit),
            budget,
            "lexical:content-scope",
        )?;
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

    fn collect_candidate_ids_for_paths(
        &self,
        paths: &BTreeSet<String>,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        if paths.is_empty() {
            return Ok(BTreeSet::new());
        }
        let compiled = self.with_doc_kind(self.path_restriction_query(paths), TEXT_DOC_KIND);
        let searcher = self.reader.searcher();
        let limit = usize::try_from(searcher.num_docs()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "lexical: num_docs overflow while collecting scoped content candidate ids: {err}"
            ))
        })?;
        if limit == 0 {
            return Ok(BTreeSet::new());
        }
        let hits = budgeted_search(
            &searcher,
            &*compiled,
            &TopDocs::with_limit(limit),
            budget,
            "lexical:content-scope",
        )?;
        let mut out: BTreeSet<String> = BTreeSet::new();
        for (_, doc_address) in hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            if let Some(candidate_id) = stored_text(&doc, self.fields.candidate_id) {
                let _inserted = out.insert(candidate_id);
            }
        }
        Ok(out)
    }

    fn allowed_paths_for_content_predicate(
        &self,
        constraint: &ContentPredicateConstraint,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        let content_leaf = self.predicate_content_leaf_from_constraint(constraint);
        let content_paths = self.collect_matching_paths_for_leaf(&content_leaf, budget)?;
        let Some(scope_paths) =
            self.collect_matching_paths_for_content_scope(constraint, budget)?
        else {
            return Ok(content_paths);
        };
        Ok(content_paths.intersection(&scope_paths).cloned().collect())
    }

    fn allowed_candidate_ids_for_content_predicate(
        &self,
        constraint: &ContentPredicateConstraint,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        let allowed_paths = self.allowed_paths_for_content_predicate(constraint, budget)?;
        self.collect_candidate_ids_for_paths(&allowed_paths, budget)
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
        budget: &RequestBudgetV1,
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
                drop(self.allowed_candidate_ids_for_content_predicate(&constraint, budget)?);
                Ok(LqExpr::Leaf(LqLeaf::Predicate {
                    name: canonical_name,
                    args: canonical_args,
                }))
            }
            None => Err(unimplemented_predicate(format!(
                "lexical: predicate leaf `{canonical_name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
            ))),
        }
    }

    fn lower_predicates_for_boolean_scope(
        &self,
        expr: &LqExpr,
        budget: &RequestBudgetV1,
    ) -> Result<LqExpr, CoreError> {
        match expr {
            LqExpr::Leaf(LqLeaf::Predicate { name, args }) => {
                self.lower_predicate_for_boolean_scope(name, args, budget)
            }
            LqExpr::Empty | LqExpr::Leaf(_) => Ok(expr.clone()),
            LqExpr::All(parts) => {
                let mut out: Vec<LqExpr> = Vec::with_capacity(parts.len());
                for part in parts {
                    out.push(self.lower_predicates_for_boolean_scope(part, budget)?);
                }
                Ok(collapse_exprs(out, true))
            }
            LqExpr::Any(parts) => {
                let mut out: Vec<LqExpr> = Vec::with_capacity(parts.len());
                for part in parts {
                    out.push(self.lower_predicates_for_boolean_scope(part, budget)?);
                }
                Ok(collapse_exprs(out, false))
            }
            LqExpr::Not(inner) => Ok(LqExpr::Not(Box::new(
                self.lower_predicates_for_boolean_scope(inner, budget)?,
            ))),
        }
    }

    fn extract_predicate_plan(
        &self,
        expr: &LqExpr,
        budget: &RequestBudgetV1,
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
                    let (lowered, repo_parts, file_parts) =
                        self.extract_predicate_plan(part, budget)?;
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
                self.lower_predicates_for_boolean_scope(expr, budget)?,
                Vec::new(),
                Vec::new(),
            )),
        }
    }

    fn prepare_predicate_plan(
        &self,
        query: &LqQuery,
        budget: &RequestBudgetV1,
    ) -> Result<PreparedPredicatePlan, CoreError> {
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
                let allowed_paths =
                    self.collect_matching_paths_for_content_scope(&constraint, budget)?;
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
        let (expr, repo_constraints, file_predicates) =
            self.extract_predicate_plan(&query.expr, budget)?;
        let mut allowed_repo_ids: Option<BTreeSet<String>> = None;
        for constraint in &repo_constraints {
            let repo_ids = match constraint {
                RepoScopeConstraint::File(file) => {
                    self.collect_repo_ids_for_repo_has_file(file, &query.options, budget)?
                }
                RepoScopeConstraint::Content(leaf) => {
                    self.collect_repo_ids_for_repo_has_content(leaf, &query.options, budget)?
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
            let paths = self.allowed_paths_for_content_predicate(predicate, budget)?;
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
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn Query>, CoreError> {
        match expr {
            LqExpr::Empty => Ok(Box::new(AllQuery)),
            LqExpr::Leaf(leaf) => self.compile_leaf(leaf, options, include_path_terms, budget),
            LqExpr::All(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((
                        Occur::Must,
                        self.compile_expr(part, options, false, budget)?,
                    ));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Any(parts) => {
                let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(parts.len());
                for part in parts {
                    clauses.push((
                        Occur::Should,
                        self.compile_expr(part, options, false, budget)?,
                    ));
                }
                Ok(Box::new(BooleanQuery::new(clauses)))
            }
            LqExpr::Not(inner) => {
                let inner_q = self.compile_expr(inner, options, false, budget)?;
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
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn Query>, CoreError> {
        let candidate_ids =
            self.collect_matching_candidate_ids_for_regex(source, options, budget)?;
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
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn Query>, CoreError> {
        match leaf {
            LqLeaf::Keyword(text) | LqLeaf::RawString(text) => {
                // Both AST shapes route through the planner-gated regex
                // pipeline when the caller's options pin
                // `LqPatternType::Regexp`; bypassing this would skip the
                // dialect filter, the trigram-missing threshold, and the
                // typed `LEX_REGEX_*` error codes.
                if options.pattern_type == LqPatternType::Regexp {
                    return self.compile_regex_content_leaf(text, options, budget);
                }
                if matches!(leaf, LqLeaf::RawString(_)) {
                    let candidate_ids =
                        self.collect_matching_candidate_ids_for_raw_substring(text, options)?;
                    if candidate_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    return Ok(self.candidate_restriction_query(&candidate_ids));
                }
                self.compile_keyword_leaf(text, options, include_path_terms)
            }
            LqLeaf::Phrase(text) => {
                let candidate_ids =
                    self.collect_matching_candidate_ids_for_phrase(text, options)?;
                if candidate_ids.is_empty() {
                    return Ok(self.match_none_query());
                }
                Ok(self.candidate_restriction_query(&candidate_ids))
            }
            LqLeaf::Regex(text) => self.compile_regex_content_leaf(text, options, budget),
            LqLeaf::StructuralBlock(_) => Err(CoreError::Typed {
                code: "STR_PRODUCER_PARSE_TREE_UNAVAILABLE".to_string(),
                message: "lexical: structural leaf cannot compile without producer parse-tree ops"
                    .to_string(),
            }),
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
                    let repo_ids =
                        self.collect_repo_ids_for_repo_has_file(&constraint, options, budget)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.repo_id_restriction_query(&repo_ids))
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
                    let repo_ids =
                        self.collect_repo_ids_for_repo_has_content(&leaf, options, budget)?;
                    if repo_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.repo_id_restriction_query(&repo_ids))
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
                    Ok(self.repo_id_restriction_query(&repo_ids))
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
                    Ok(self.repo_id_restriction_query(&repo_ids))
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
                    let candidate_ids =
                        self.collect_candidate_ids_for_file_has_owner(&arg, budget)?;
                    if candidate_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.candidate_restriction_query(&candidate_ids))
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
                        self.collect_candidate_ids_for_file_has_contributor(&arg, budget)?;
                    if candidate_ids.is_empty() {
                        return Ok(self.match_none_query());
                    }
                    Ok(self.candidate_restriction_query(&candidate_ids))
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
                    Ok(self.repo_id_restriction_query(&repo_ids))
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
                    Ok(self.repo_id_restriction_query(&repo_ids))
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
                        let candidate_ids =
                            self.allowed_candidate_ids_for_content_predicate(&constraint, budget)?;
                        if candidate_ids.is_empty() {
                            return Ok(self.match_none_query());
                        }
                        return Ok(self.candidate_restriction_query(&candidate_ids));
                    }
                    let lowered = self.predicate_content_leaf_from_constraint(&constraint);
                    self.compile_leaf(&lowered, options, false, budget)
                }
                None => Err(unimplemented_predicate(format!(
                    "lexical: predicate leaf `{name}` is not executable on Tantivy adapter (owner: {PREDICATE_OWNER})"
                ))),
            },
        }
    }

    fn compile_filter(
        &self,
        filter: &LqFilter,
        options: &LqOptions,
        budget: &RequestBudgetV1,
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
            LqFilter::Content { leaf } => {
                Ok(Some(self.compile_leaf(leaf, options, false, budget)?))
            }
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
        budget: &RequestBudgetV1,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        if prepared.force_empty {
            return Ok(None);
        }
        let include_path_terms = Self::enables_path_term_surface(&prepared.expr, &query.options);
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        if !matches!(prepared.expr, LqExpr::Empty) {
            clauses.push((
                Occur::Must,
                self.compile_expr(&prepared.expr, &query.options, include_path_terms, budget)?,
            ));
        }
        for filter in &query.filters {
            if let Some(compiled_filter) = self.compile_filter(filter, &query.options, budget)? {
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

    fn compile_query_with_constraints(
        &self,
        query: &LqQuery,
        prepared: &PreparedPredicatePlan,
        constraints: &QueryConstraintSetV1,
        budget: &RequestBudgetV1,
    ) -> Result<Option<Box<dyn Query>>, CoreError> {
        if matches!(prepared.expr, LqExpr::Empty) && constraints.repo_relative_path_exact.is_some()
        {
            return Ok(Some(Box::new(AllQuery)));
        }
        self.compile_query_from_prepared(query, prepared, budget)
    }

    fn prepare_executable_query(
        &self,
        query: &LqQuery,
        default_doc_kind: QueryDocKind,
        budget: &RequestBudgetV1,
    ) -> Result<Option<PreparedExecutableQuery>, CoreError> {
        let (prepared_query, doc_kind) =
            self.prepare_query_for_doc_kind(query, default_doc_kind)?;
        let predicate_plan = self.prepare_predicate_plan(&prepared_query, budget)?;
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
        center_terms: &[String],
    ) -> Result<LexicalCandidate, CoreError> {
        let candidate_id = stored_text(doc, self.fields.candidate_id).ok_or_else(|| {
            CoreError::Storage("lexical: stored doc missing candidate_id field".to_string())
        })?;
        let stored_snippet = stored_text(doc, self.fields.snippet)
            .or_else(|| stored_text(doc, self.fields.chunk_text))
            .ok_or_else(|| {
                CoreError::Storage(
                    "lexical: stored doc missing snippet/chunk_text field".to_string(),
                )
            })?;
        // J7Q-02: emit a hit-centered, deterministically-bounded window so a long
        // source line never streams an unbounded blob at the head of the result.
        // J7Q-07: the window also reports the primary hit's byte offset plus every
        // matched-hit span for UI highlighting, so a consumer never re-derives the
        // matches from raw text.
        let (snippet, snippet_hit_offset, highlights) =
            window_snippet(&stored_snippet, center_terms);
        let repo_relative_path =
            stored_text(doc, self.fields.repo_relative_path).ok_or_else(|| {
                CoreError::Storage(
                    "lexical: stored doc missing repo_relative_path field".to_string(),
                )
            })?;
        let start_line = stored_u32(doc, self.fields.start_line)?.ok_or_else(|| {
            CoreError::Storage("lexical: stored doc missing start_line field".to_string())
        })?;
        let end_line = stored_u32(doc, self.fields.end_line)?.ok_or_else(|| {
            CoreError::Storage("lexical: stored doc missing end_line field".to_string())
        })?;
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
            snippet_hit_offset,
            highlights,
        })
    }

    fn document_to_symbol_candidate(
        &self,
        doc: &TantivyDocument,
        score: f32,
        center_terms: &[String],
    ) -> Result<SymbolCandidate, CoreError> {
        let candidate = self.document_to_candidate(doc, score, center_terms)?;
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

/// Maximum emitted lexical snippet length in bytes.
///
/// A snippet at or below this length is emitted whole; a longer one is truncated
/// to a hit-centered window of at most this many bytes. This MUST agree with the
/// snippet-quality gate's `MAX_SNIPPET_LEN` in
/// `quanta-index-searchd-harness::snippet`, so a snippet the engine emits passes
/// the rail's bounded-window check (J7Q-02).
const SNIPPET_WINDOW_BYTES: usize = 240;

/// Leading-context budget when centering a window on a hit.
///
/// Equals `SNIPPET_WINDOW_BYTES / 2`, precomputed to avoid integer division; the
/// trailing side takes the remainder, so a centered hit always carries leading
/// context.
const SNIPPET_LEAD_BYTES: usize = 120;

/// Collect the literal substrings a snippet may be centered on, in query order.
///
/// Only literal-bearing leaves contribute (`Keyword` / `Phrase` / `RawString`);
/// regex, structural, and predicate leaves carry no single literal to center on.
fn collect_snippet_center_terms(expr: &LqExpr, out: &mut Vec<String>) {
    match expr {
        LqExpr::Leaf(LqLeaf::Keyword(text) | LqLeaf::Phrase(text) | LqLeaf::RawString(text)) => {
            if !text.is_empty() {
                out.push(text.clone());
            }
        }
        LqExpr::Leaf(LqLeaf::Regex(_) | LqLeaf::StructuralBlock(_) | LqLeaf::Predicate { .. })
        | LqExpr::Empty => {}
        LqExpr::Not(inner) => collect_snippet_center_terms(inner, out),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for child in children {
                collect_snippet_center_terms(child, out);
            }
        }
    }
}

/// The center terms for a query, in query order.
fn snippet_center_terms(query: &LqQuery) -> Vec<String> {
    let mut terms = Vec::new();
    collect_snippet_center_terms(&query.expr, &mut terms);
    terms
}

/// Step `index` down to the nearest UTF-8 char boundary at or below it.
///
/// `str::floor_char_boundary` is unstable, so this is a stable hand-rolled
/// equivalent. `index` is always clamped into `0..=len` by callers.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut i = index.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i = i.saturating_sub(1);
    }
    i
}

/// Narrow a within-snippet byte offset to `u32` for the candidate field.
///
/// The offset is always bounded by [`SNIPPET_WINDOW_BYTES`] (≤ 240) in the
/// truncated case, or by the short snippet's own length otherwise, so it is far
/// below `u32::MAX` and the narrowing is exact.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "the offset is always into the emitted text, which is at most SNIPPET_WINDOW_BYTES (240) bytes — the full stored snippet when it is <= 240 bytes, otherwise a windowed excerpt — so the usize->u32 narrowing is exact"
)]
fn snippet_offset_u32(within: usize) -> u32 {
    within as u32
}

/// Collect every center-term occurrence within `emitted` as a highlight span,
/// in ascending start order with exact duplicates removed.
///
/// Spans are computed over the *emitted* text (post-windowing), so their offsets
/// are valid against the snippet a consumer actually receives (J7Q-07).
fn collect_highlights(emitted: &str, center_terms: &[String]) -> Vec<HighlightSpan> {
    let mut spans: Vec<HighlightSpan> = Vec::new();
    for term in center_terms {
        if term.is_empty() {
            continue;
        }
        for (pos, matched) in emitted.match_indices(term.as_str()) {
            spans.push(HighlightSpan {
                start: snippet_offset_u32(pos),
                len: snippet_offset_u32(matched.len()),
            });
        }
    }
    spans.sort_by_key(|span| span.start);
    spans.dedup();
    spans
}

/// Produce the emitted snippet for a stored chunk, plus the primary hit offset
/// and every matched-hit span within it (for UI highlight anchoring, J7Q-02 +
/// J7Q-07).
///
/// The whole text is returned when it already fits [`SNIPPET_WINDOW_BYTES`].
/// Otherwise a window of at most that many bytes is taken, centered on the first
/// present center term (so the hit keeps leading and trailing context) and
/// clamped to UTF-8 char boundaries. When no center term is present in an
/// over-long snippet, the leading window is kept so the result is still bounded —
/// never an unbounded blob. Highlight spans and the primary offset are computed
/// over the emitted text, so the primary offset equals the first span's `start`.
/// Fully determined by `(stored, center_terms)`, so two runs over identical
/// inputs emit byte-identical windows, offsets, and spans.
fn window_snippet(
    stored: &str,
    center_terms: &[String],
) -> (String, Option<u32>, Vec<HighlightSpan>) {
    let text = if stored.len() <= SNIPPET_WINDOW_BYTES {
        stored.to_string()
    } else {
        let first_hit = center_terms
            .iter()
            .filter_map(|term| stored.find(term.as_str()))
            .min();
        let start = first_hit.map_or(0, |hit| {
            floor_char_boundary(stored, hit.saturating_sub(SNIPPET_LEAD_BYTES))
        });
        let raw_end = start.saturating_add(SNIPPET_WINDOW_BYTES).min(stored.len());
        let end = floor_char_boundary(stored, raw_end);
        // start <= end <= stored.len(), both floor_char_boundary results, so this is unreachable; "" keeps the fallback bounded.
        stored.get(start..end).unwrap_or("").to_string()
    };
    let highlights = collect_highlights(&text, center_terms);
    let primary = highlights.first().map(|span| span.start);
    (text, primary, highlights)
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
fn planner_preflight_expr(
    query: &LqQuery,
    expr: &LqExpr,
    has_repo_metadata: bool,
) -> Result<(), CoreError> {
    let filter_plan = crate::filters::plan_filters(&query.filters, &query.options)
        .map_err(|err| map_planner_error(&crate::planner::LexicalPlannerError::FilterPlan(err)))?;
    for entry in &filter_plan.typed_unavailable {
        if is_unavailable_suppressed_by_metadata(entry.code, has_repo_metadata) {
            continue;
        }
        return Err(CoreError::Typed {
            code: entry.code.to_string(),
            message: entry.reason.to_string(),
        });
    }
    crate::planner::LexicalPlanner::validate_expr(query, expr)
        .map_err(|err| map_planner_error(&err))
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
/// * `PhrasePlan` lowers through [`map_phrase_plan_error`]: the pre-flight
///   tokenizes phrase text with the shared normalizer, so a token-less or
///   over-long phrase is refused here under the same `LEX_TEXT_QUERY_*`
///   codes the keyword path uses, before any executor lowering runs.
/// * The other leaf-planner failures (`RegexPlan`, `TrigramPlan`,
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
        // The pre-flight plans phrase leaves for real (it tokenizes them), so
        // its literal refusals must carry the same typed codes the executor
        // would have produced for the keyword shape of the same text.
        LexicalPlannerError::PhrasePlan(phrase) => map_phrase_plan_error(phrase.clone()),
        // The remaining leaf-planner errors reaching this site would mean
        // the pre-flight disagreed with the live executor's leaf-side
        // mapping. They are never produced on the current pipeline (those
        // leaves compile during executor lowering, not during pre-flight
        // planning); the arm is kept for completeness and lowers to
        // `InvalidContract` so the mismatch is visible rather than swallowed.
        LexicalPlannerError::RegexPlan(_)
        | LexicalPlannerError::TrigramPlan(_)
        | LexicalPlannerError::SymbolPlan(_) => {
            CoreError::InvalidContract(format!("lexical: planner: {err}"))
        }
    }
}

impl LexicalSearcher for TantivySearcher {
    fn resident_bytes_estimate(&self) -> u64 {
        self.resident_bytes_estimate
    }

    fn artifact_identity(&self) -> LexicalArtifactIdentityV1 {
        self.artifact_identity.clone()
    }

    fn search_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalSearchPageV1, CoreError> {
        // This is the single text-query execution path. The unconstrained
        // port method delegates here so constraint support cannot drift into
        // a second planner/search implementation.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query_with_constraints(&effective_query, constraints)?;
        let empty_page = || LexicalSearchPageV1 {
            candidates: Vec::new(),
            exact_total: Self::wants_exact_total(&effective_query).then_some(0),
        };
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Text, budget)?
        else {
            return Ok(empty_page());
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            return Ok(empty_page());
        }
        let requested = usize::try_from(top_k)
            .map_err(|err| CoreError::InvalidContract(format!("lexical: top_k: {err}")))?;
        let limit = Self::page_limit(&effective_query, requested);
        if limit == 0 {
            return Ok(empty_page());
        }
        if Self::uses_unindexed_scan(&effective_query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "lexical")?;
            return self.manual_text_search(
                &effective_query,
                &prepared_query,
                constraints,
                limit,
                true,
                budget,
            );
        }
        let Some(base) = self.compile_query_with_constraints(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            constraints,
            budget,
        )?
        else {
            return Ok(empty_page());
        };
        let compiled =
            self.with_doc_kind_and_constraints(base, prepared_query.doc_kind.as_str(), constraints);
        let boosted_options = &effective_query.options;
        let searcher = self.reader.searcher();
        let whole_set = Self::needs_whole_match_set(&effective_query);
        let collected = self.collect_bounded(
            &searcher,
            &*compiled,
            &effective_query,
            limit,
            whole_set,
            if whole_set {
                "exact-set text search (projection or bounded count)"
            } else {
                "text search"
            },
            budget,
        )?;
        let center_terms = snippet_center_terms(&effective_query);
        let mut out = Vec::with_capacity(collected.hits.len());
        for (score, doc_address) in collected.hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_candidate(
                &doc,
                Self::apply_query_boost_score(score, boosted_options),
                &center_terms,
            )?);
        }
        let projects = Self::projects_repo_surface(&effective_query)
            || Self::projects_path_surface(&effective_query)
            || Self::selects_file_projection(&effective_query);
        let projected = Self::collapse_select_projection(&effective_query, out);
        // A projection collapses the whole (budgeted) match set, so its row
        // universe is known exactly whether or not a count was requested;
        // the document count from the collector would be the wrong number.
        let exact_total = if projects {
            Some(u64::try_from(projected.len()).map_err(|err| {
                CoreError::InvalidContract(format!("lexical: projection total overflow: {err}"))
            })?)
        } else {
            collected.exact_total
        };
        Ok(LexicalSearchPageV1 {
            candidates: Self::stabilize_and_cap_hits(projected, limit),
            exact_total,
        })
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
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        self.search_symbols_constrained(
            query,
            &QueryConstraintSetV1::unconstrained(),
            top_k,
            budget,
        )
    }

    fn search_symbols_constrained(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        // Single symbol-query execution path; the unconstrained entrypoint
        // delegates here to prevent planner and scoring drift.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query_with_constraints(&effective_query, constraints)?;
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Symbol, budget)?
        else {
            return Ok(Vec::new());
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            return Ok(Vec::new());
        }
        let requested = usize::try_from(top_k)
            .map_err(|err| CoreError::InvalidContract(format!("symbol: top_k: {err}")))?;
        let limit = Self::page_limit(&effective_query, requested);
        if limit == 0 {
            return Ok(Vec::new());
        }
        if Self::uses_unindexed_scan(&effective_query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "symbol")?;
            return self.manual_symbol_search(
                &effective_query,
                &prepared_query,
                constraints,
                limit,
                budget,
            );
        }
        let Some(base) = self.compile_query_with_constraints(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            constraints,
            budget,
        )?
        else {
            return Ok(Vec::new());
        };
        let compiled =
            self.with_doc_kind_and_constraints(base, prepared_query.doc_kind.as_str(), constraints);
        let boosted_options = &effective_query.options;
        let searcher = self.reader.searcher();
        let whole_set = Self::needs_whole_match_set(&effective_query);
        let collected = self.collect_bounded(
            &searcher,
            &*compiled,
            &effective_query,
            limit,
            whole_set,
            if whole_set {
                "exact-set symbol search (bounded count)"
            } else {
                "symbol search"
            },
            budget,
        )?;
        let center_terms = snippet_center_terms(&effective_query);
        let mut out = Vec::with_capacity(collected.hits.len());
        for (score, doc_address) in collected.hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_symbol_candidate(
                &doc,
                Self::apply_query_boost_score(score, boosted_options),
                &center_terms,
            )?);
        }
        Ok(Self::stabilize_and_cap_symbol_hits(out, limit))
    }

    fn search_symbols_all(
        &self,
        query: &LqQuery,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<SymbolCandidate>, CoreError> {
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query(&effective_query)?;
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Symbol, budget)?
        else {
            return Ok(Vec::new());
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let requested = Self::corpus_docs(&searcher, "symbol scope materialization")?;
        let limit = Self::page_limit(&effective_query, requested);
        if limit == 0 {
            return Ok(Vec::new());
        }
        if Self::uses_unindexed_scan(&effective_query.options) {
            return self.manual_symbol_search(
                &effective_query,
                &prepared_query,
                &QueryConstraintSetV1::unconstrained(),
                limit,
                budget,
            );
        }
        let Some(base) = self.compile_query_from_prepared(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            budget,
        )?
        else {
            return Ok(Vec::new());
        };
        let compiled = self.with_doc_kind(base, prepared_query.doc_kind.as_str());
        let collected = self.collect_bounded(
            &searcher,
            &*compiled,
            &effective_query,
            limit,
            true,
            "symbol scope materialization",
            budget,
        )?;
        let center_terms = snippet_center_terms(&effective_query);
        let mut out = Vec::with_capacity(collected.hits.len());
        for (score, doc_address) in collected.hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_symbol_candidate(
                &doc,
                Self::apply_query_boost_score(score, &effective_query.options),
                &center_terms,
            )?);
        }
        Ok(Self::stabilize_and_cap_symbol_hits(out, limit))
    }

    fn search_all(
        &self,
        query: &LqQuery,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<LexicalCandidate>, CoreError> {
        let constraints = &QueryConstraintSetV1::unconstrained();
        LexicalPolicy::validate_query_with_constraints(query, constraints)?;
        let Some(prepared_query) =
            self.prepare_executable_query(query, QueryDocKind::Text, budget)?
        else {
            return Ok(Vec::new());
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(query)? {
            return Ok(Vec::new());
        }
        let searcher = self.reader.searcher();
        let requested = Self::corpus_docs(&searcher, "structural scope materialization")?;
        let limit = Self::page_limit(query, requested);
        if limit == 0 {
            return Ok(Vec::new());
        }
        if Self::uses_unindexed_scan(&query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "lexical")?;
            let page =
                self.manual_text_search(query, &prepared_query, constraints, limit, false, budget)?;
            return Ok(Self::collapse_repo_projection(query, page.candidates));
        }
        let Some(base) = self.compile_query_with_constraints(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            constraints,
            budget,
        )?
        else {
            return Ok(Vec::new());
        };
        let compiled =
            self.with_doc_kind_and_constraints(base, prepared_query.doc_kind.as_str(), constraints);
        let collected = self.collect_bounded(
            &searcher,
            &*compiled,
            query,
            limit,
            true,
            "structural scope materialization",
            budget,
        )?;
        let center_terms = snippet_center_terms(query);
        let mut out = Vec::with_capacity(collected.hits.len());
        for (score, doc_address) in collected.hits {
            let doc: TantivyDocument = searcher.doc(doc_address).map_err(|err| {
                CoreError::Storage(format!("lexical: fetch doc {doc_address:?}: {err}"))
            })?;
            out.push(self.document_to_candidate(
                &doc,
                Self::apply_query_boost_score(score, &query.options),
                &center_terms,
            )?);
        }
        Ok(Self::collapse_repo_projection(
            query,
            Self::stabilize_and_cap_hits(out, limit),
        ))
    }

    fn candidate_presence(&self, candidate_id: &str) -> Result<CandidatePresenceV1, CoreError> {
        let searcher = self.reader.searcher();
        Ok(
            match self.locate_candidate(&searcher, candidate_id, TEXT_DOC_KIND)? {
                Some(_) => CandidatePresenceV1::Indexed,
                None => CandidatePresenceV1::NotIndexed,
            },
        )
    }

    fn explain_candidate(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        candidate_id: &str,
        budget: &RequestBudgetV1,
    ) -> Result<LexicalCandidateExplanationV1, CoreError> {
        // The same preparation as `search_constrained`, step for step, so
        // the plan that scores this one document is the plan that ranked it.
        let effective_query =
            rewrite_symbol_name_predicate_query(query)?.unwrap_or_else(|| query.clone());
        LexicalPolicy::validate_query_with_constraints(&effective_query, constraints)?;
        let searcher = self.reader.searcher();
        let not_matched = |reason: &str| {
            Ok(LexicalCandidateExplanationV1::NotMatched {
                reason: reason.to_string(),
            })
        };
        let Some(prepared_query) =
            self.prepare_executable_query(&effective_query, QueryDocKind::Text, budget)?
        else {
            return match self.locate_candidate(&searcher, candidate_id, TEXT_DOC_KIND)? {
                Some(_) => not_matched("the plan matches no document"),
                None => Ok(LexicalCandidateExplanationV1::NotIndexed),
            };
        };
        let doc_kind = prepared_query.doc_kind.as_str();
        let Some((doc_address, doc)) = self.locate_candidate(&searcher, candidate_id, doc_kind)?
        else {
            return Ok(LexicalCandidateExplanationV1::NotIndexed);
        };
        planner_preflight_expr(
            &prepared_query.query,
            &prepared_query.predicate_plan.expr,
            self.repo_metadata.is_some(),
        )?;
        if !self.repo_filters_allow(&effective_query)? {
            return not_matched("a repo filter excludes this generation");
        }
        let boost_factor = Self::boost_factor(&effective_query.options);
        if Self::uses_unindexed_scan(&effective_query.options) {
            Self::ensure_manual_scan_supports_constraints(constraints, "lexical")?;
            if !self.manual_doc_matches(
                &doc,
                &effective_query,
                &prepared_query,
                constraints,
                budget,
            )? {
                return not_matched("the unindexed scan does not match the document");
            }
            return Ok(LexicalCandidateExplanationV1::Matched(
                LexicalScoreTraceV1 {
                    engine: LexicalScoreEngineV1::UnindexedScan,
                    engine_score: 1.0,
                    boost_factor,
                    emitted_score: Self::apply_query_boost_score(1.0, &effective_query.options),
                },
            ));
        }
        let Some(base) = self.compile_query_with_constraints(
            &prepared_query.query,
            &prepared_query.predicate_plan,
            constraints,
            budget,
        )?
        else {
            return not_matched("the plan compiles to nothing");
        };
        let compiled = self.with_doc_kind_and_constraints(base, doc_kind, constraints);
        let Some(engine_score) = Self::score_one_document(&searcher, &*compiled, doc_address)?
        else {
            return not_matched("the plan does not match the document");
        };
        Ok(LexicalCandidateExplanationV1::Matched(
            LexicalScoreTraceV1 {
                engine: LexicalScoreEngineV1::Bm25,
                engine_score,
                boost_factor,
                emitted_score: Self::apply_query_boost_score(
                    engine_score,
                    &effective_query.options,
                ),
            },
        ))
    }

    fn admitted_candidates(
        &self,
        query: &LqQuery,
        constraints: &QueryConstraintSetV1,
        candidate_ids: &BTreeSet<String>,
        budget: &RequestBudgetV1,
    ) -> Result<BTreeSet<String>, CoreError> {
        self.admitted_candidates_v1(query, constraints, candidate_ids, budget)
    }
}

#[cfg(test)]
mod regex_match_cache_tests {
    use super::*;

    fn sample_generation(generation: u64) -> GenKey {
        GenKey {
            repo_id: RepoId::new("repo-alpha"),
            revision_id: RevisionId::new("rev-alpha"),
            generation: ManifestGeneration::new(generation),
        }
    }

    fn sample_matches(candidate_id: &str) -> BTreeSet<String> {
        std::iter::once(candidate_id.to_string()).collect()
    }

    fn sample_identity(generation: u64, digest: &str) -> GenerationSnapshot {
        GenerationSnapshot {
            repo_id: RepoId::new("../../repo-alpha"),
            revision_id: RevisionId::new("/rev-alpha"),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        }
    }

    #[test]
    fn regex_match_cache_invalidates_only_target_generation() {
        let generation_one = sample_generation(7);
        let generation_two = sample_generation(8);
        let key_one = RegexMatchCacheKey {
            generation: generation_one.clone(),
            normalized_source: "foo".to_string(),
        };
        let key_two = RegexMatchCacheKey {
            generation: generation_one.clone(),
            normalized_source: "bar".to_string(),
        };
        let key_three = RegexMatchCacheKey {
            generation: generation_two,
            normalized_source: "foo".to_string(),
        };
        let mut cache = RegexMatchCache::new(RegexMatchCachePolicy::DEFAULT);
        cache
            .insert(key_one.clone(), Arc::new(sample_matches("cand-1")))
            .expect("fits");
        cache
            .insert(key_two.clone(), Arc::new(sample_matches("cand-2")))
            .expect("fits");
        cache
            .insert(key_three.clone(), Arc::new(sample_matches("cand-3")))
            .expect("fits");
        assert_eq!(cache.len(), 3);

        cache.invalidate_generation(&generation_one);

        assert_eq!(cache.len(), 1);
        assert!(cache.get(&key_one).is_none());
        assert!(cache.get(&key_two).is_none());
        assert_eq!(
            cache.get(&key_three).as_deref(),
            Some(&sample_matches("cand-3"))
        );
        assert_eq!(
            cache.stats().resident_bytes,
            regex_match_set_bytes(&sample_matches("cand-3")),
            "invalidation gives back the evicted generation's bytes"
        );
    }

    /// The cache is bounded by bytes and cardinality, not entries alone.
    ///
    /// A set wider than the policy is refused and counted, inserts evict
    /// least-recently-used entries until the byte bound holds, and a hit is
    /// the shared set rather than a copy (QI-BB-024).
    #[test]
    fn regex_match_cache_is_byte_bounded_and_shares_hits() {
        let generation = sample_generation(1);
        let key = |name: &str| RegexMatchCacheKey {
            generation: generation.clone(),
            normalized_source: name.to_string(),
        };
        let set = |ids: &[&str]| -> Arc<BTreeSet<String>> {
            Arc::new(ids.iter().map(|id| (*id).to_string()).collect())
        };
        let one_entry_bytes = regex_match_set_bytes(&set(&["cand-1"]));
        let policy =
            RegexMatchCachePolicy::new(8, one_entry_bytes.saturating_mul(2).saturating_add(1), 2)
                .expect("valid policy");
        let mut cache = RegexMatchCache::new(policy);

        // Too many matches for one entry: refused, counted, not resident.
        assert_eq!(
            cache.insert(key("broad"), set(&["cand-1", "cand-2", "cand-3"])),
            Err(RegexMatchCacheRefusal::Cardinality { matches: 3 })
        );
        assert_eq!(cache.stats().refused_cardinality, 1);
        assert_eq!(cache.stats().resident_bytes, 0);

        // Two single-candidate entries fit under the byte bound.
        cache.insert(key("a"), set(&["cand-1"])).expect("fits");
        cache.insert(key("b"), set(&["cand-2"])).expect("fits");
        assert_eq!(cache.stats().entries, 2);
        assert_eq!(cache.stats().evictions, 0);

        // A third does not: the least recently used ("a") goes.
        cache
            .insert(key("c"), set(&["cand-3"]))
            .expect("fits after eviction");
        let stats = cache.stats();
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.evictions, 1);
        assert!(stats.resident_bytes <= policy.max_resident_bytes());
        assert!(cache.get(&key("a")).is_none());
        assert_eq!(cache.stats().misses, 1);

        // A hit is the same allocation the cache holds.
        let first = cache.get(&key("b")).expect("b is resident");
        let second = cache.get(&key("b")).expect("b is still resident");
        assert!(Arc::ptr_eq(&first, &second), "a hit must share, not clone");
        assert_eq!(cache.stats().hits, 2);

        // A single set wider than the whole byte bound is refused on bytes.
        let wide = set(&["cand-x", "cand-y"]);
        assert!(regex_match_set_bytes(&wide) <= policy.max_resident_bytes());
        let mut tiny = RegexMatchCache::new(
            RegexMatchCachePolicy::new(8, one_entry_bytes.saturating_sub(1), 8).expect("policy"),
        );
        assert_eq!(
            tiny.insert(key("z"), set(&["cand-1"])),
            Err(RegexMatchCacheRefusal::Bytes {
                bytes: one_entry_bytes
            })
        );
        assert_eq!(tiny.stats().refused_bytes, 1);
    }

    #[test]
    fn incomplete_generation_discard_is_contained_and_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
        let candidate = sample_identity(7, "digest-a");
        let key = GenKey {
            repo_id: candidate.repo_id.clone(),
            revision_id: candidate.revision_id.clone(),
            generation: candidate.manifest_generation,
        };
        let generation_dir = adapter.index_path(&key);
        std::fs::create_dir_all(&generation_dir).expect("create incomplete generation");
        std::fs::write(generation_dir.join("partial"), b"partial").expect("write partial");

        assert_eq!(
            adapter
                .discard_incomplete_generation(&candidate)
                .expect("discard incomplete"),
            IncompleteGenerationDiscardOutcomeV1::Discarded
        );
        assert!(!generation_dir.exists());
        assert!(generation_dir.starts_with(temp.path()));
        assert_eq!(
            adapter
                .discard_incomplete_generation(&candidate)
                .expect("idempotent absent"),
            IncompleteGenerationDiscardOutcomeV1::Absent
        );
    }

    #[test]
    fn incomplete_generation_discard_refuses_sealed_exact_and_conflict() {
        let temp = tempfile::tempdir().expect("tempdir");
        let adapter = LexicalAdapter::with_state_root(temp.path().to_path_buf());
        let sealed = sample_identity(8, "digest-a");
        let key = GenKey {
            repo_id: sealed.repo_id.clone(),
            revision_id: sealed.revision_id.clone(),
            generation: sealed.manifest_generation,
        };
        let generation_dir = adapter.index_path(&key);
        std::fs::create_dir_all(&generation_dir).expect("create generation");
        persist_lexical_sealed_identity(&generation_dir, &sealed).expect("persist identity");

        let exact_error = adapter
            .discard_incomplete_generation(&sealed)
            .expect_err("sealed exact must be immutable");
        assert!(matches!(
            exact_error,
            CoreError::Typed { ref code, .. } if code == "GENERATION_IMMUTABLE"
        ));
        let mut conflict = sealed;
        conflict.manifest_digest = "digest-b".to_string();
        let conflict_error = adapter
            .discard_incomplete_generation(&conflict)
            .expect_err("sealed conflict must fail closed");
        assert!(matches!(
            conflict_error,
            CoreError::Typed { ref code, .. }
                if code == "GENERATION_IDENTITY_DIGEST_MISMATCH"
        ));
        assert!(generation_dir.exists());
    }
}
