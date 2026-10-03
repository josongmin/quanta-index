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

mod activation_token;
pub mod ids;
pub mod query;
pub mod results;
mod source_file;

pub use source_file::{SourceFileKey, SourceFileRevision};

pub use activation_token::{
    ACTIVATION_ROOT_INCARNATION_BYTES_V1, ActivationTokenValidationErrorV1,
    SearchCorpusActivationTokenV1,
};
pub use ids::{
    FileId, GenerationId, IdentityValidationErrorV1, LogicalGenerationIdentityV1, ManifestDigest,
    ManifestGeneration, RepoId, RepoRelativePath, RepositoryRevisionIdentityV1, RevisionId,
};
pub use query::{
    CODE_SEARCH_IDENTIFIER_TYPO_PREDICATE, CODE_SEARCH_SYMBOL_COMPONENTS_PREDICATE,
    ExactRepoRelativePathV1, GenerationPin,
    GenerationSelector, INTERNAL_FETCH_CEILING, INTERNAL_FETCH_OUT_OF_RANGE_CODE,
    InternalFetchOutOfRangeV1, LanguageCode, LexicalCursor, LexicalRowOrderKey,
    MAX_CODE_SEARCH_TERM_BYTES, MAX_CODE_SEARCH_TERMS, MAX_CODE_SEARCH_TYPO_BYTES,
    MIN_CODE_SEARCH_TYPO_BYTES, PUBLIC_TOP_K_MAX, PUBLIC_TOP_K_MIN,
    QUERY_CURSOR_GENERATION_MISMATCH_CODE, QUERY_CURSOR_UNSUPPORTED_CODE,
    QueryConstraintIntersectionV1, QueryConstraintSetV1, TOP_K_OUT_OF_RANGE_CODE, TextQueryRequest,
    TextQuerySyntax, TopKOutOfRangeV1, continuation_fetch_size, valid_code_search_typo_identifier,
    valid_code_search_component_query,
    validate_internal_fetch_size, validate_lexical_page_v1, validate_public_top_k,
};
pub use results::{
    ApproximateMethodV2, ApproximateQualityContractV2, CURSOR_ENVELOPE_V2_VERSION,
    CandidateCountV1, ContinuationTokenError, ContinuationTokenV2, CoverageV1,
    CursorAuxEpochKindV2, CursorAuxEpochV2, CursorBindingV2, CursorEnvelopeError, CursorEnvelopeV2,
    CursorKeyV2, CursorRouteV2, CursorTtlPolicyV2, DiffCandidate, DiffHunkSide, EmptyProvenanceV2,
    ExaminedUniverseV1, ExecutionOutcomeV2, ExhaustionProofV1, HighlightSpan, HistoryScoreError,
    HistoryScoreV1, InterruptedReasonV2, LaneTraceV1, LexicalCandidate, PreviewByteRange,
    PreviewKind, PreviewMetadata, PreviewUnavailableReason, QueryResultWindowV1,
    QueryResultWindowV2, StructuralBinding, StructuralCandidate,
};
