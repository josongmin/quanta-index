//! Closed set of LQ-family error codes.
//!
//! Consolidates the per-crate `*ErrorCode` placeholders that currently live in
//! 16 `lq_*` crates. Downstream migration is a separate ticket; this scaffold
//! only lands the canonical surface so subsequent tickets can pivot to it.
//!
//! Wire form: `SCREAMING_SNAKE_CASE` string. Deserialization is an exact-match
//! against the closed table; an unknown code fails closed per `CLAUDE.md`
//! Safety rules (no silent fallback to a generic / Unknown variant).
//!
//! Spec source: the per-crate code tables enumerated in
//! [`docs/plans/may-24-lexical-indexing-sorucegraph/tickets/PRE-CONTRACT-EXT.md`](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/PRE-CONTRACT-EXT.md).
//! Final reconciliation against `usecase.md` §0 is the consuming ticket's job
//! (PRE-NORM, LEX-01..05, LEX-07, STR-01, BRIDGE-01); this scaffold lists every
//! variant claimed by the per-crate placeholders so they can route through one
//! canonical enum.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

/// Closed enum of LQ-family error codes.
///
/// Variants are grouped by originating crate but the wire form is a flat
/// `SCREAMING_SNAKE_CASE` namespace; the grouping is editorial only.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LexicalErrorCode {
    // --- PRE-NORM (parser / planner limits + lexical syntax) ---
    LimitExceededBytes,
    LimitExceededDepth,
    LimitExceededFanout,
    LimitExceededNfa,
    LimitExceededStructural,
    TokenInvalid,
    ForbiddenSyntax,
    UnknownFilter,
    InvalidFilterValue,
    EmptyQuery,
    UnclosedQuote,
    RegexParse,
    InvalidPatternType,
    UnsupportedCombo,
    SyntaxError,

    // --- LEX-00 text-norm ---
    InvalidUtf8,
    OversizedChunk,
    NormalizerUnknownLang,
    UnknownPatternType,

    // --- LEX-01 scorer ---
    InvalidGeneration,
    EmptyCorpus,
    InvalidBm25Param,
    IdfTableDeserialize,
    ScoreNormalizationFailed,
    NanSignal,

    // --- LEX-02 trigram ---
    PlanLimitExceeded,
    IndexDeserialize,
    RegexPrefilterUnusable,
    IndexCorrupted,

    // --- LEX-03 positions ---
    StateGenerationRegression,
    NormalizerVersionMismatch,
    WindowOutOfRange,

    // --- LEX-04 regex ---
    ParseFail,
    QueryTimeout,
    ExecutionInternal,

    // --- LEX-05 symbol ---
    SymbolPayloadDecodeFail,
    SymbolRecordInvalid,
    StateNotReady,

    // --- LEX-06 ranker ---
    InvalidWeights,
    RankInvalidSignal,
    WeightsDeserialize,
    WeightsHashMismatch,
    WeightsEncodeFailed,

    // --- LEX-07 history ---
    HistoryRefNotFound,
    HistoryRangeOverrun,
    HistoryMergeCycle,
    HistoryTraceIncomplete,
    HistoryUnindexed,
    HistoryCommitDecodeFail,
    HistoryRefDecodeFail,
    HistoryCommitParentUnknown,

    // --- STR-01 structural ---
    StrParseFail,
    StrInvalidMetavar,
    StrLangNotSupported,
    StrParseTreeDecodeFail,
    StrProducerParseTreeUnavailable,

    // --- RT-01 runtime / dirty buffer ---
    DirtyStaleGen,
    DirtyBufferFull,
    DirtyTtlExpired,
    DirtyBadIdentity,
    DirtyPayloadDecodeFail,
    InvalidBufferConfig,

    // --- SEM-01 semantic ---
    SemDimMismatch,
    SemNotReady,
    SemInvalidVector,
    SemMetricUnsupported,
    SemAnnNondeterministic,

    // --- SEM-02 hybrid ---
    HybInvalidWeights,
    HybGenMismatch,
    HybPushdownIncomplete,
    HybTopKInvalid,
    HybStrategyUnsupported,
    HybSubqueryInvalid,

    // --- BRIDGE-01 ---
    BridgeUnsupportedFilter,
    BridgeUnsupportedDirective,
    BridgeAmbiguousFilter,
    BridgeVersionPin,
    BridgeTranslateFail,

    // --- OBS-01 ---
    ObsCardinalityGuard,
    ObsInvalidSpan,
    ObsInvalidMetric,
    ObsAuditMissingField,
}

impl LexicalErrorCode {
    /// Every variant, in declaration order. The order is load-bearing for
    /// tests (cardinality + uniqueness) and stable iteration in downstream
    /// observability / docs surfaces.
    pub const ALL: &'static [Self] = &[
        Self::LimitExceededBytes,
        Self::LimitExceededDepth,
        Self::LimitExceededFanout,
        Self::LimitExceededNfa,
        Self::LimitExceededStructural,
        Self::TokenInvalid,
        Self::ForbiddenSyntax,
        Self::UnknownFilter,
        Self::InvalidFilterValue,
        Self::EmptyQuery,
        Self::UnclosedQuote,
        Self::RegexParse,
        Self::InvalidPatternType,
        Self::UnsupportedCombo,
        Self::SyntaxError,
        Self::InvalidUtf8,
        Self::OversizedChunk,
        Self::NormalizerUnknownLang,
        Self::UnknownPatternType,
        Self::InvalidGeneration,
        Self::EmptyCorpus,
        Self::InvalidBm25Param,
        Self::IdfTableDeserialize,
        Self::ScoreNormalizationFailed,
        Self::NanSignal,
        Self::PlanLimitExceeded,
        Self::IndexDeserialize,
        Self::RegexPrefilterUnusable,
        Self::IndexCorrupted,
        Self::StateGenerationRegression,
        Self::NormalizerVersionMismatch,
        Self::WindowOutOfRange,
        Self::ParseFail,
        Self::QueryTimeout,
        Self::ExecutionInternal,
        Self::SymbolPayloadDecodeFail,
        Self::SymbolRecordInvalid,
        Self::StateNotReady,
        Self::InvalidWeights,
        Self::RankInvalidSignal,
        Self::WeightsDeserialize,
        Self::WeightsHashMismatch,
        Self::WeightsEncodeFailed,
        Self::HistoryRefNotFound,
        Self::HistoryRangeOverrun,
        Self::HistoryMergeCycle,
        Self::HistoryTraceIncomplete,
        Self::HistoryUnindexed,
        Self::HistoryCommitDecodeFail,
        Self::HistoryRefDecodeFail,
        Self::HistoryCommitParentUnknown,
        Self::StrParseFail,
        Self::StrInvalidMetavar,
        Self::StrLangNotSupported,
        Self::StrParseTreeDecodeFail,
        Self::StrProducerParseTreeUnavailable,
        Self::DirtyStaleGen,
        Self::DirtyBufferFull,
        Self::DirtyTtlExpired,
        Self::DirtyBadIdentity,
        Self::DirtyPayloadDecodeFail,
        Self::InvalidBufferConfig,
        Self::SemDimMismatch,
        Self::SemNotReady,
        Self::SemInvalidVector,
        Self::SemMetricUnsupported,
        Self::SemAnnNondeterministic,
        Self::HybInvalidWeights,
        Self::HybGenMismatch,
        Self::HybPushdownIncomplete,
        Self::HybTopKInvalid,
        Self::HybStrategyUnsupported,
        Self::HybSubqueryInvalid,
        Self::BridgeUnsupportedFilter,
        Self::BridgeUnsupportedDirective,
        Self::BridgeAmbiguousFilter,
        Self::BridgeVersionPin,
        Self::BridgeTranslateFail,
        Self::ObsCardinalityGuard,
        Self::ObsInvalidSpan,
        Self::ObsInvalidMetric,
        Self::ObsAuditMissingField,
    ];

    /// Canonical `SCREAMING_SNAKE_CASE` wire form.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::LimitExceededBytes => "LIMIT_EXCEEDED_BYTES",
            Self::LimitExceededDepth => "LIMIT_EXCEEDED_DEPTH",
            Self::LimitExceededFanout => "LIMIT_EXCEEDED_FANOUT",
            Self::LimitExceededNfa => "LIMIT_EXCEEDED_NFA",
            Self::LimitExceededStructural => "LIMIT_EXCEEDED_STRUCTURAL",
            Self::TokenInvalid => "TOKEN_INVALID",
            Self::ForbiddenSyntax => "FORBIDDEN_SYNTAX",
            Self::UnknownFilter => "UNKNOWN_FILTER",
            Self::InvalidFilterValue => "INVALID_FILTER_VALUE",
            Self::EmptyQuery => "EMPTY_QUERY",
            Self::UnclosedQuote => "UNCLOSED_QUOTE",
            Self::RegexParse => "REGEX_PARSE",
            Self::InvalidPatternType => "INVALID_PATTERN_TYPE",
            Self::UnsupportedCombo => "UNSUPPORTED_COMBO",
            Self::SyntaxError => "SYNTAX_ERROR",
            Self::InvalidUtf8 => "INVALID_UTF8",
            Self::OversizedChunk => "OVERSIZED_CHUNK",
            Self::NormalizerUnknownLang => "NORMALIZER_UNKNOWN_LANG",
            Self::UnknownPatternType => "UNKNOWN_PATTERN_TYPE",
            Self::InvalidGeneration => "INVALID_GENERATION",
            Self::EmptyCorpus => "EMPTY_CORPUS",
            Self::InvalidBm25Param => "INVALID_BM25_PARAM",
            Self::IdfTableDeserialize => "IDF_TABLE_DESERIALIZE",
            Self::ScoreNormalizationFailed => "SCORE_NORMALIZATION_FAILED",
            Self::NanSignal => "NAN_SIGNAL",
            Self::PlanLimitExceeded => "PLAN_LIMIT_EXCEEDED",
            Self::IndexDeserialize => "INDEX_DESERIALIZE",
            Self::RegexPrefilterUnusable => "REGEX_PREFILTER_UNUSABLE",
            Self::IndexCorrupted => "INDEX_CORRUPTED",
            Self::StateGenerationRegression => "STATE_GENERATION_REGRESSION",
            Self::NormalizerVersionMismatch => "NORMALIZER_VERSION_MISMATCH",
            Self::WindowOutOfRange => "WINDOW_OUT_OF_RANGE",
            Self::ParseFail => "PARSE_FAIL",
            Self::QueryTimeout => "QUERY_TIMEOUT",
            Self::ExecutionInternal => "EXECUTION_INTERNAL",
            Self::SymbolPayloadDecodeFail => "SYMBOL_PAYLOAD_DECODE_FAIL",
            Self::SymbolRecordInvalid => "SYMBOL_RECORD_INVALID",
            Self::StateNotReady => "STATE_NOT_READY",
            Self::InvalidWeights => "INVALID_WEIGHTS",
            Self::RankInvalidSignal => "RANK_INVALID_SIGNAL",
            Self::WeightsDeserialize => "WEIGHTS_DESERIALIZE",
            Self::WeightsHashMismatch => "WEIGHTS_HASH_MISMATCH",
            Self::WeightsEncodeFailed => "WEIGHTS_ENCODE_FAILED",
            Self::HistoryRefNotFound => "HISTORY_REF_NOT_FOUND",
            Self::HistoryRangeOverrun => "HISTORY_RANGE_OVERRUN",
            Self::HistoryMergeCycle => "HISTORY_MERGE_CYCLE",
            Self::HistoryTraceIncomplete => "HISTORY_TRACE_INCOMPLETE",
            Self::HistoryUnindexed => "HISTORY_UNINDEXED",
            Self::HistoryCommitDecodeFail => "HISTORY_COMMIT_DECODE_FAIL",
            Self::HistoryRefDecodeFail => "HISTORY_REF_DECODE_FAIL",
            Self::HistoryCommitParentUnknown => "HISTORY_COMMIT_PARENT_UNKNOWN",
            Self::StrParseFail => "STR_PARSE_FAIL",
            Self::StrInvalidMetavar => "STR_INVALID_METAVAR",
            Self::StrLangNotSupported => "STR_LANG_NOT_SUPPORTED",
            Self::StrParseTreeDecodeFail => "STR_PARSE_TREE_DECODE_FAIL",
            Self::StrProducerParseTreeUnavailable => "STR_PRODUCER_PARSE_TREE_UNAVAILABLE",
            Self::DirtyStaleGen => "DIRTY_STALE_GEN",
            Self::DirtyBufferFull => "DIRTY_BUFFER_FULL",
            Self::DirtyTtlExpired => "DIRTY_TTL_EXPIRED",
            Self::DirtyBadIdentity => "DIRTY_BAD_IDENTITY",
            Self::DirtyPayloadDecodeFail => "DIRTY_PAYLOAD_DECODE_FAIL",
            Self::InvalidBufferConfig => "INVALID_BUFFER_CONFIG",
            Self::SemDimMismatch => "SEM_DIM_MISMATCH",
            Self::SemNotReady => "SEM_NOT_READY",
            Self::SemInvalidVector => "SEM_INVALID_VECTOR",
            Self::SemMetricUnsupported => "SEM_METRIC_UNSUPPORTED",
            Self::SemAnnNondeterministic => "SEM_ANN_NONDETERMINISTIC",
            Self::HybInvalidWeights => "HYB_INVALID_WEIGHTS",
            Self::HybGenMismatch => "HYB_GEN_MISMATCH",
            Self::HybPushdownIncomplete => "HYB_PUSHDOWN_INCOMPLETE",
            Self::HybTopKInvalid => "HYB_TOP_K_INVALID",
            Self::HybStrategyUnsupported => "HYB_STRATEGY_UNSUPPORTED",
            Self::HybSubqueryInvalid => "HYB_SUBQUERY_INVALID",
            Self::BridgeUnsupportedFilter => "BRIDGE_UNSUPPORTED_FILTER",
            Self::BridgeUnsupportedDirective => "BRIDGE_UNSUPPORTED_DIRECTIVE",
            Self::BridgeAmbiguousFilter => "BRIDGE_AMBIGUOUS_FILTER",
            Self::BridgeVersionPin => "BRIDGE_VERSION_PIN",
            Self::BridgeTranslateFail => "BRIDGE_TRANSLATE_FAIL",
            Self::ObsCardinalityGuard => "OBS_CARDINALITY_GUARD",
            Self::ObsInvalidSpan => "OBS_INVALID_SPAN",
            Self::ObsInvalidMetric => "OBS_INVALID_METRIC",
            Self::ObsAuditMissingField => "OBS_AUDIT_MISSING_FIELD",
        }
    }

    /// Parse the canonical wire form. Returns `None` for any out-of-set
    /// value; callers must never substitute a generic / Unknown variant.
    ///
    /// Linear scan over `ALL` is O(N) but N <= ~100 and this is not a
    /// hot-path (parser error reporting + IPC decode). A hash table would
    /// pull in either `std::collections::HashMap` (banned by `clippy.toml`)
    /// or a `BTreeMap` with one-off insertion at module init; neither is
    /// worth the complexity at this scale.
    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|variant| variant.as_code_str() == value)
    }
}

impl fmt::Display for LexicalErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

impl Serialize for LexicalErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct LexicalErrorCodeVisitor;

impl Visitor<'_> for LexicalErrorCodeVisitor {
    type Value = LexicalErrorCode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a `SCREAMING_SNAKE_CASE` LexicalErrorCode string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        LexicalErrorCode::from_code_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, &["<closed LexicalErrorCode set>"]))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for LexicalErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(LexicalErrorCodeVisitor)
    }
}
