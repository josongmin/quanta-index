#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! DTO surface shared between producer (`semantica-codegraph-v2`) and search-plane.
//!
//! Two surfaces:
//!
//! * Query path (`query`, `results`, `ipc` envelopes/split) — UDS frame payloads sent by query clients.
//! * Channel path (`channel`) — legacy typed transport ops still consumed by
//!   `searchd` internals. New producer-facing publish authority lives under
//!   `ipc`.
//!
//! Core identifiers, generation pinning, and text-query syntax live in the
//! sibling `quanta-index-contract-base` crate. They are re-exported here so
//! existing imports keep working while downstream crates that only need the
//! core surface can migrate to depend on `quanta-index-contract-base` directly.

#[macro_use]
mod macros;
mod bounded_cluster_members;
pub mod canonical_order;
mod semantic_kinds;
mod source_coverage;

pub use quanta_index_contract_base::{
    PreviewByteRange, PreviewKind, PreviewMetadata, PreviewUnavailableReason, SourceFileKey,
    SourceFileRevision,
};
pub use source_coverage::{
    FileCoverageIter, FileCoverageSnapshot, SourceCoverageError, SourceFileCoverage,
    SourcePublicationEvent, SymbolCoverage, SymbolNameSourcePolicyV1, source_file_unit_set_sha256,
};

/// Internal legacy channel surface used by `searchd` composition-root,
/// replay, and restart recovery paths.
///
/// External producer/query callers should use typed DTOs under `ipc`, `query`,
/// `repomap`, and `results` instead of importing channel ops from this module.
pub mod channel;
pub mod ipc;
pub mod query;
pub mod repomap;
pub mod results;

pub use channel::{
    ChunkId, ChunkRecord, ChunkStructuralMetadata, ClearLexicalSurface, EmbeddingId,
    EmbeddingRecord, LexicalFullBundle, LexicalSeal, OwnerDocKind, ReplaceLexicalScope,
    ReplaceStructuralScope, SymbolId, TombstoneLexicalScope, UpsertChunk, UpsertCommit,
    UpsertParseTree, UpsertRef, UpsertSymbol,
};
pub use ipc::*;
pub use quanta_index_contract_base::{
    ACTIVATION_ROOT_INCARNATION_BYTES_V1, ActivationTokenValidationErrorV1, ApproximateMethodV2,
    ApproximateQualityContractV2, CURSOR_ENVELOPE_V2_VERSION, CandidateCountV1,
    ContinuationTokenError, ContinuationTokenV2, CoverageV1, CursorAuxEpochKindV2,
    CursorAuxEpochV2, CursorBindingV2, CursorEnvelopeError, CursorEnvelopeV2, CursorKeyV2,
    CursorRouteV2, CursorTtlPolicyV2, EmptyProvenanceV2, ExaminedUniverseV1, ExecutionOutcomeV2,
    ExhaustionProofV1, FileId, GenerationId, GenerationPin, INTERNAL_FETCH_CEILING,
    INTERNAL_FETCH_OUT_OF_RANGE_CODE, IdentityValidationErrorV1, InternalFetchOutOfRangeV1,
    InterruptedReasonV2, LaneTraceV1, LogicalGenerationIdentityV1, ManifestDigest,
    ManifestGeneration, PUBLIC_TOP_K_MAX, PUBLIC_TOP_K_MIN, QueryResultWindowV1,
    QueryResultWindowV2, RepoId, RepoRelativePath, RepositoryRevisionIdentityV1, RevisionId,
    SearchCorpusActivationTokenV1, TOP_K_OUT_OF_RANGE_CODE, TopKOutOfRangeV1,
    continuation_fetch_size, validate_internal_fetch_size, validate_public_top_k,
};
pub use query::*;
pub use repomap::*;
pub use results::*;

// PRE-CONTRACT-EXT canonical LQ-family wire types. `lex::*` is a thin
// re-export facade over `results::explanation::*` for the ranker-explanation
// surface (`ExplanationRow`, `SearchExplanation`, ...) plus the LQ-family
// records that live only under `lex` (`CommitRecord`, `SymbolRecord`, ...).
pub mod lex;
