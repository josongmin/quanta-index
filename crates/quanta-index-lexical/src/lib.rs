//! Lexical adapter — Tantivy 0.22-backed inverted index.
//!
//! Implements [`quanta_index_core::LexicalIndexBuildPort`] and
//! [`quanta_index_core::LexicalIndexOpenPort`].
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
//! the Tantivy commit and the segment files it references, the immutable
//! ranked-key tables, the text-authority tree, and the overlays — each with
//! length and digest. Nothing may land in a sealed generation afterwards:
//! an index-mutating op and an
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

/// Global distinct path and content trigram posting memberships admitted by
/// the canonical F15 file authority. Scale preflight uses the same bound.
pub const FILE_AUTHORITY_POSTING_MEMBERSHIP_LIMIT: u32 = 20_000_000;

mod analyzer;

mod authority_doc_set;

mod budgeted_search;

mod dense_admission;

mod doc_census;

mod file_authority;

pub mod filters;

pub mod history_text_index;

pub mod phrase;

pub mod plan;

pub mod planner;

mod predicate_registry;

mod ranked_keys;
mod ranked_page;

pub mod regex;

mod regex_match_cache;

mod sealed_generation;

pub mod symbol;
mod symbol_components;

mod text_authority;

pub mod trigram_plan;

mod adapter;
mod adapter_ingest;
mod adapter_lifecycle;
mod adapter_open;
mod causal_profile;
mod channel_payloads;
mod documents;
mod generation_dir;
mod index_store;
mod inventory;
mod metadata_normalize;
mod overlay_codec;
mod query_admission;
mod query_errors;
mod schema;
mod stage_timing;
// The searcher facade declares its submodules `pub(crate)` (the crate's
// other modules name their items; `unreachable_pub = deny` forbids a bare
// `pub`). The expectation sits here because module discipline keeps a
// facade file to one-line attributes.
#[expect(
    clippy::redundant_pub_crate,
    reason = "the searcher's submodules are private to the crate; `pub(crate)` is the visibility the crate's other modules need"
)]
mod searcher;
mod text_authority_plan;
mod text_docs;
mod writer_cache;

#[cfg(test)]
mod test_support;

/// The one text normalization contract (QI-BB-011), shared with the query DSL
/// and the search plane; every text surface of this crate lowers through it.
pub(crate) use quanta_index_lq_text_normalizer as normalize;

pub use sealed_generation::LexicalSealCommitmentStats;
pub use sealed_generation::coverage::{LexicalCoverageReadByPhaseStats, LexicalCoverageReadStats};

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use std::path::PathBuf;

use std::sync::{Arc, Mutex, RwLock};

use std::time::Instant;

use quanta_index_contract::channel::LexicalChannelOp;

use quanta_index_contract::{
    FileContributorIdentityEntry, LexicalCursor, LqExpr, LqLeaf, LqQuery, LqVisibility,
    ManifestGeneration, RepoId, RevisionId,
};

use quanta_index_core::{
    LexicalArtifactIdentityV1, LexicalExecutionBudgetV1, LexicalWriterPolicy,
    TextAuthorityUpdateStats, WriterAdmissionPort,
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

use crate::predicate_registry::{
    RepoDescriptionArg, RepoFileConstraint, RepoMetaArg, RepoTopicArg,
};

use crate::ranked_page::ProjectionGroup;

use crate::regex::RegexPolicy;

use crate::regex_match_cache::RegexMatchCache;

use crate::text_authority::{AddedTextDoc, ShardBody, ShardedTextAuthority, TextAuthorityManifest};

use tantivy::schema::{Field, Schema};

use tantivy::{Index, IndexReader, IndexWriter};

pub use crate::inventory::inventory_sealed_generations;

const TEXT_DOC_KIND: &str = "text";

const SYMBOL_DOC_KIND: &str = "symbol";

/// Marker inside the name of a durable write's temporary file.
const DURABLE_WRITE_TEMPORARY_MARKER: &str = ".tmp-";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueryDocKind {
    Text,
    Symbol,
}

/// Schema field handles for the lexical index. Cloned cheaply into every
/// searcher; constructed once per adapter instance.
#[derive(Clone)]
struct SchemaFields {
    schema: Schema,
    candidate_id: Field,
    repo_id: Field,
    revision_id: Field,
    source_revision_id: Field,
    source_sha256: Field,
    chunk_start_byte: Field,
    chunk_end_byte: Field,
    chunk_raw_sha256: Field,
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
    symbol_local_name: Field,
    symbol_local_name_folded: Field,
    symbol_component_folded: Field,
    symbol_qualified_name: Field,
    symbol_qualified_name_folded: Field,
    symbol_local_name_original: Field,
    symbol_qualified_name_original: Field,
    symbol_signature: Field,
    symbol_definition_start_byte: Field,
    symbol_definition_end_byte: Field,
    /// The text-authority doc id of a text document (absent on symbols):
    /// the shard-addressing key the sidecar and the index share, assigned
    /// once when the document is written and never reused within the
    /// generation's chain. Indexed and a fast column too, so a set of doc
    /// ids restricts a query without naming a single candidate
    /// (`authority_doc_set`).
    text_authority_doc_id: Field,
    /// Exact indexed-field census captured from this document before Tantivy
    /// consumes it; used only when a later delta retires this physical doc.
    live_bm25_doc_census: Field,
}

/// Per-generation cache key.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GenKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
}

const GENERATION_MUTATION_LOCK_STRIPES: usize = 256;

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

/// Legacy `FullBundle` placeholder payload.
///
/// Older producers used the literal `b"manifest"` as an opaque marker.
/// Later producers emit a CBOR map with repo metadata. The two are
/// distinguished here explicitly so that real decode failures surface as
/// `InvalidContract` rather than silently routing to "no metadata".
const LEGACY_FULL_BUNDLE_PAYLOAD: &[u8] = b"manifest";

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

/// One text document as the index stores it: candidate id and the
/// text-authority doc id the sidecar shares with it.
struct IndexedTextDoc {
    candidate_id: String,
    doc_id: u64,
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
    retired_files: Vec<quanta_index_contract::SourceFileKey>,
    /// Text documents the batch will write.
    added_count: u64,
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
    coverage_reads: Arc<Mutex<LexicalCoverageReadByPhaseStats>>,
    coverage_decode_cache: Mutex<sealed_generation::coverage::CoverageDecodeCache>,
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
    /// Serializes mutations, scrub and delta-base pins per bounded stripe.
    /// A stripe collision only delays an unrelated generation; the table
    /// cannot grow with ingest history.
    generation_mutations: [Mutex<()>; GENERATION_MUTATION_LOCK_STRIPES],
    /// Every build and scrub step holds a read guard; quarantine and sealed
    /// directory removal take the write guard, so a removed namespace cannot
    /// be reused before its fence is settled. Mutation stripes exclude scrub
    /// from a target and from a delta's pinned base without stopping unrelated
    /// builds for the whole hash step.
    directory_lifecycle: RwLock<()>,
    scrub_progress: Mutex<quanta_index_core::domains::integrity::IntegrityScrubProgressV1>,
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

/// The open's visitor: keeps every proved and decoded file to become a
/// searcher.
#[derive(Default)]
struct LoadedGeneration {
    shards: Vec<(u64, ShardBody)>,
    file_authority: Option<file_authority::FileAuthority>,
    repo_metadata: Option<LexicalRepoMetadataPayload>,
    repo_commit_recency: Option<RepoCommitRecencyShard>,
    repo_meta: Option<RepoMetaShard>,
    repo_topic: Option<RepoTopicShard>,
    repo_description: Option<RepoDescriptionShard>,
    file_ownership: Option<FileOwnershipShard>,
    file_contributor: Option<FileContributorShard>,
}

struct TantivySearcher {
    /// Optional only for explicitly unbound generations; absence is not
    /// evidence of a completely indexed empty universe.
    source_coverage: Option<quanta_index_contract::FileCoverageSnapshot>,
    source_publication_event: Option<quanta_index_contract::SourcePublicationEvent>,
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    fields: SchemaFields,
    reader: IndexReader,
    ranked_keys: Arc<ranked_keys::RankedKeyTables>,
    live_bm25: Arc<sealed_generation::live_bm25::LiveBm25Statistics>,
    repo_metadata: Option<LexicalRepoMetadataPayload>,
    regex_match_cache: Arc<Mutex<RegexMatchCache>>,
    /// Deployment-scoped regex policy threaded from the adapter at open time.
    /// Read at the regex-leaf compile site rather than fabricated there, so
    /// the dialect/literal/trigram-cap knobs are a single source of truth.
    regex_policy: RegexPolicy,
    /// Examined-candidate budget every exact-set execution runs under.
    execution_budget: LexicalExecutionBudgetV1,
    text_authority: Option<ShardedTextAuthority>,
    file_authority: Option<file_authority::FileAuthority>,
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
    allowed_files: Option<BTreeSet<quanta_index_contract::SourceFileKey>>,
    allowed_repo_ids: Option<BTreeSet<String>>,
    allowed_candidate_ids: Option<BTreeSet<String>>,
    force_empty: bool,
}

/// What an `index:no` text scan returns: at most `limit` rows after the
/// boundary, projected when asked.
struct ManualPage<'a> {
    limit: usize,
    after: Option<&'a LexicalCursor>,
    group: Option<ProjectionGroup>,
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

#[cfg(test)]
mod adapter_tests {
    use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
    use quanta_index_core::{
        CoreError, IncompleteGenerationDiscardOutcomeV1, IncompleteGenerationDiscardPort,
    };
    use tantivy::schema::{STORED, STRING};

    use super::*;
    use crate::index_store::{
        open_or_create_index, open_sealed_index, persist_lexical_sealed_identity,
    };

    fn sample_identity(generation: u64, digest: &str) -> GenerationSnapshot {
        GenerationSnapshot {
            repo_id: RepoId::new("../../repo-alpha")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("/rev-alpha")
                .expect("static fixture ID satisfies canonical policy"),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        }
    }

    /// The door refuses an index committed under a schema this build does
    /// not write — here the schema before the text-authority doc id was
    /// indexed and a fast column — typed, before any query runs over it.
    #[test]
    fn the_door_refuses_an_index_under_another_schema() {
        let temp = crate::test_support::generation_fixture().expect("generation fixture");
        let mut builder = Schema::builder();
        let _candidate_id = builder.add_text_field("candidate_id", STRING | STORED);
        let _doc_id = builder.add_u64_field("text_authority_doc_id", STORED);
        let index = Index::create_in_dir(temp.path(), builder.build()).expect("create");
        let mut writer: IndexWriter = index.writer(15_000_000).expect("writer");
        let _opstamp = writer.commit().expect("commit");
        drop(writer);
        let refused = open_sealed_index(temp.path()).expect_err("another schema is refused");
        assert!(
            matches!(
                refused,
                CoreError::Typed { ref code, .. }
                    if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported
            ),
            "{refused:?}"
        );

        let current = crate::test_support::generation_fixture().expect("generation fixture");
        let index =
            Index::create_in_dir(current.path(), SchemaFields::build().schema).expect("create");
        let mut writer: IndexWriter = index.writer(15_000_000).expect("writer");
        let _opstamp = writer.commit().expect("commit");
        drop(writer);
        assert!(
            open_sealed_index(current.path()).is_ok(),
            "this build's schema opens"
        );
    }

    #[test]
    fn unproved_preupgrade_unsealed_index_cannot_be_reopened_as_current() {
        let old = tempfile::tempdir().expect("legacy unsealed generation");
        let old_path = old.path().canonicalize().expect("canonical old path");
        let fields = SchemaFields::build();
        let index = Index::create_in_dir(&old_path, fields.schema.clone())
            .expect("create old same-schema index without current producer marker");
        let mut writer: IndexWriter = index.writer(15_000_000).expect("writer");
        let _opstamp = writer.commit().expect("commit old index");
        drop(writer);
        drop(index);
        let refused = open_or_create_index(&fields, &old_path)
            .expect_err("unproved old index cannot acquire a current writer");
        assert!(
            matches!(
                refused,
                CoreError::Typed { ref code, ref message }
                    if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported
                        && message.contains("format marker is missing")
            ),
            "{refused:?}"
        );
        assert!(
            !old_path.join("search-corpus-index-format.cbor").exists(),
            "refusal must not launder old index content with a current marker"
        );

        let current = tempfile::tempdir().expect("current unsealed generation");
        let current_path = current
            .path()
            .canonicalize()
            .expect("canonical current path");
        let index = open_or_create_index(&fields, &current_path).expect("new index");
        drop(index);
        assert!(
            current_path
                .join("search-corpus-index-format.cbor")
                .is_file(),
            "producer marker must precede the first index commit"
        );
        let _reopened =
            open_or_create_index(&fields, &current_path).expect("marked current index may resume");
    }

    #[test]
    fn unsealed_index_rejects_wrong_malformed_and_symlinked_format_markers() {
        let fields = SchemaFields::build();
        let temp = tempfile::tempdir().expect("current index");
        let canonical = temp.path().canonicalize().expect("canonical index path");
        let index = open_or_create_index(&fields, &canonical).expect("create current index");
        drop(index);
        let marker = canonical.join("search-corpus-index-format.cbor");
        assert_eq!(std::fs::read(&marker).expect("read current marker"), [0x02]);
        let _reopened = open_or_create_index(&fields, &canonical)
            .expect("current format marker permits reopening");

        for (bytes, expected_detail) in [
            (&[0x01][..], "format 1, expected 2"),
            (&[0x03][..], "format 3, expected 2"),
            (&[0x01, 0x00][..], "invalid format marker"),
            (&[0xff][..], "invalid format marker"),
            (&[0; 10][..], "cannot read format marker"),
        ] {
            std::fs::write(&marker, bytes).expect("replace marker");
            let refused = open_or_create_index(&fields, &canonical)
                .expect_err("unproved format cannot reopen a materialized index");
            assert!(
                matches!(
                    refused,
                    CoreError::Typed { ref code, ref message }
                        if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported
                            && message.contains(expected_detail)
                ),
                "{refused:?}"
            );
        }

        let target = canonical.join("other-format.cbor");
        std::fs::write(&target, [0x02]).expect("valid bytes outside marker path");
        std::fs::remove_file(&marker).expect("remove marker before symlink");
        std::os::unix::fs::symlink(&target, &marker).expect("symlinked marker");
        let refused = open_or_create_index(&fields, &canonical)
            .expect_err("a symlink cannot prove format provenance");
        assert!(
            matches!(
                refused,
                CoreError::Typed { ref code, ref message }
                    if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported
                        && message.contains("cannot open format marker")
            ),
            "{refused:?}"
        );
    }

    #[test]
    fn empty_seal_cannot_promote_unproved_preupgrade_index() {
        let temp = tempfile::tempdir().expect("state root");
        let canonical = temp.path().canonicalize().expect("canonical state root");
        let adapter = LexicalAdapter::with_state_root(canonical);
        let candidate = sample_identity(8, "digest-empty-seal");
        let key = GenKey {
            repo_id: candidate.repo_id,
            revision_id: candidate.revision_id,
            generation: candidate.manifest_generation,
        };
        let generation_dir = adapter.index_path(&key);
        std::fs::create_dir_all(&generation_dir).expect("generation directory");
        let index = Index::create_in_dir(&generation_dir, SchemaFields::build().schema)
            .expect("old same-schema index without current marker");
        let mut writer: IndexWriter = index.writer(15_000_000).expect("writer");
        let _opstamp = writer.commit().expect("commit empty index");
        drop(writer);
        drop(index);

        let refused = adapter
            .finalize_index_for_seal(&key)
            .expect_err("empty seal must acquire the same proved writer");
        assert!(
            matches!(
                refused,
                CoreError::Typed { ref code, ref message }
                    if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported
                        && message.contains("format marker is missing")
            ),
            "{refused:?}"
        );
        assert!(
            !generation_dir
                .join("search-corpus-index-format.cbor")
                .exists()
        );
        assert!(
            !generation_dir
                .join("search-corpus-generation-manifest.cbor")
                .exists()
        );
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
            CoreError::Typed { ref code, .. }
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable
        ));
        let mut conflict = sealed;
        conflict.manifest_digest = "digest-b".to_string();
        let conflict_error = adapter
            .discard_incomplete_generation(&conflict)
            .expect_err("sealed conflict must fail closed");
        assert!(matches!(
            conflict_error,
            CoreError::Typed { ref code, .. }
                if *code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch
        ));
        assert!(generation_dir.exists());
    }
}
