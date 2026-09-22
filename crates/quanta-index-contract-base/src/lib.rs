#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Stable core surface shared by the producer (`semantica-codegraph-v2`) and the
//! search-plane runtime.
//!
//! `quanta-index-contract` composes envelope/request/response shapes on top of
//! this crate. Downstream code that only needs the *core* identifiers,
//! generation pinning, and text-query syntax — i.e. does not need to encode or
//! decode envelopes — should depend on `quanta-index-contract-base` directly to
//! avoid pulling the full envelope compile surface.

#[macro_use]
pub mod macros;

pub mod ids;
pub mod query;
pub mod results;

pub use ids::{
    FileId, GenerationId, IdentityValidationErrorV1, LogicalGenerationIdentityV1, ManifestDigest,
    ManifestGeneration, RepoId, RepoRelativePath, RepositoryRevisionIdentityV1, RevisionId,
};
pub use query::{
    ExactRepoRelativePathV1, GenerationPin, GenerationSelector, INTERNAL_FETCH_CEILING,
    INTERNAL_FETCH_OUT_OF_RANGE_CODE, InternalFetchOutOfRangeV1, LanguageCode, LexicalCursor,
    LexicalRowOrderKey, PUBLIC_TOP_K_MAX, PUBLIC_TOP_K_MIN, QUERY_CURSOR_GENERATION_MISMATCH_CODE,
    QUERY_CURSOR_UNSUPPORTED_CODE, QueryConstraintIntersectionV1, QueryConstraintSetV1,
    TOP_K_OUT_OF_RANGE_CODE, TextQueryRequest, TextQuerySyntax, TopKOutOfRangeV1,
    continuation_fetch_size, validate_internal_fetch_size, validate_lexical_page_v1,
    validate_public_top_k,
};
pub use results::{
    ApproximateMethodV2, ApproximateQualityContractV2, CURSOR_ENVELOPE_V2_VERSION,
    CandidateCountV1, ContinuationTokenError, ContinuationTokenV2, CoverageV1,
    CursorAuxEpochKindV2, CursorAuxEpochV2, CursorBindingV2, CursorEnvelopeError, CursorEnvelopeV2,
    CursorKeyV2, CursorRouteV2, CursorTtlPolicyV2, DiffCandidate, DiffHunkSide, EmptyProvenanceV2,
    ExaminedUniverseV1, ExecutionOutcomeV2, ExhaustionProofV1, HighlightSpan, HistoryScoreError,
    HistoryScoreV1, InterruptedReasonV2, LaneTraceV1, LexicalCandidate, QueryResultWindowV1,
    QueryResultWindowV2, StructuralBinding, StructuralCandidate,
};
