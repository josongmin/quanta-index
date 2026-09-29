use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::LexicalErrorCode;

macro_rules! define_search_plane_error_codes {
    (
        lexical [$($lexical:ident),+ $(,)?];
        native [$($variant:ident => $wire:literal),+ $(,)?];
    ) => {
        /// Closed authority for every error code that can cross a search-plane boundary.
        ///
        /// Lexical codes retain their lower-domain type while sharing the same flat wire
        /// namespace. There is deliberately no unknown/free-form representation.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum SearchPlaneErrorCodeV2 {
            Lexical(LexicalErrorCode),
            $($variant),+
        }

        impl SearchPlaneErrorCodeV2 {
            /// Every accepted code. Declaration order is stable; the committed table
            /// sorts the corresponding wire strings before hashing.
            pub const ALL: &'static [Self] = &[
                $(Self::Lexical(LexicalErrorCode::$lexical),)+
                $(Self::$variant),+
            ];

            #[must_use]
            pub const fn as_wire_str(self) -> &'static str {
                match self {
                    Self::Lexical(code) => code.as_code_str(),
                    $(Self::$variant => $wire),+
                }
            }

            /// Exact inverse of [`Self::as_wire_str`]. Unknown and retired codes fail.
            #[must_use]
            pub fn from_wire_str(value: &str) -> Option<Self> {
                if let Some(code) = LexicalErrorCode::from_code_str(value) {
                    return Some(Self::Lexical(code));
                }
                match value {
                    $($wire => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

define_search_plane_error_codes! {
    lexical [
        LimitExceededBytes, LimitExceededDepth, LimitExceededFanout, LimitExceededNfa,
        LimitExceededStructural, TokenInvalid, ForbiddenSyntax, UnknownFilter,
        InvalidFilterValue, EmptyQuery, UnclosedQuote, RegexParse, InvalidPatternType,
        UnsupportedCombo, SyntaxError, InvalidUtf8, OversizedChunk,
        NormalizerUnknownLang, UnknownPatternType, InvalidGeneration, EmptyCorpus,
        InvalidBm25Param, IdfTableDeserialize, ScoreNormalizationFailed, NanSignal,
        PlanLimitExceeded, IndexDeserialize, RegexPrefilterUnusable, IndexCorrupted,
        StateGenerationRegression, NormalizerVersionMismatch, WindowOutOfRange,
        ParseFail, QueryTimeout, ExecutionInternal, SymbolPayloadDecodeFail,
        SymbolRecordInvalid, StateNotReady, InvalidWeights, RankInvalidSignal,
        WeightsDeserialize, WeightsHashMismatch, WeightsEncodeFailed, HistoryRefNotFound,
        HistoryRangeOverrun, HistoryMergeCycle, HistoryTraceIncomplete, HistoryUnindexed,
        HistoryCommitDecodeFail, HistoryRefDecodeFail, HistoryCommitParentUnknown,
        StrParseFail, StrInvalidMetavar, StrHoleKindUnsupported, StrLangNotSupported,
        StrParseTreeDecodeFail, StrProducerParseTreeUnavailable, DirtyStaleGen,
        DirtyBufferFull, DirtyTtlExpired, DirtyBadIdentity, DirtyPayloadDecodeFail,
        InvalidBufferConfig, SemDimMismatch, SemModelMismatch, SemNotReady,
        SemInvalidVector, SemProviderUnavailable, SemProviderAuth, SemProviderTransport,
        SemMetricUnsupported, SemAnnNondeterministic, HybInvalidWeights, HybGenMismatch,
        HybPushdownIncomplete, HybStrategyUnsupported, HybSubqueryInvalid,
        QueryTopKOutOfRange, QueryInternalFetchOutOfRange, BridgeUnsupportedFilter,
        BridgeUnsupportedDirective, BridgeAmbiguousFilter, BridgeVersionPin,
        BridgeTranslateFail, ObsCardinalityGuard, ObsInvalidSpan, ObsInvalidMetric,
        ObsAuditMissingField,
    ];
    native [
        ActivationCasConflict => "ACTIVATION_CAS_CONFLICT",
        ActivationTargetNotSealed => "ACTIVATION_TARGET_NOT_SEALED",
        ActivationTargetUnopenable => "ACTIVATION_TARGET_UNOPENABLE",
        AnnIndexIncompatible => "ANN_INDEX_INCOMPATIBLE",
        AnnIndexMissing => "ANN_INDEX_MISSING",
        AuxEpochExpired => "AUX_EPOCH_EXPIRED",
        AuxEpochUnknown => "AUX_EPOCH_UNKNOWN",
        BatchDigestConflict => "BATCH_DIGEST_CONFLICT",
        BatchDigestMismatch => "BATCH_DIGEST_MISMATCH",
        CandidateCommitmentConflict => "CANDIDATE_COMMITMENT_CONFLICT",
        CandidateGenerationMustAdvanceExpectedActive => "CANDIDATE_GENERATION_MUST_ADVANCE_EXPECTED_ACTIVE",
        CandidateIdentityInvalid => "CANDIDATE_IDENTITY_INVALID",
        CatalogBusy => "CATALOG_BUSY",
        CatalogRowCorrupt => "CATALOG_ROW_CORRUPT",
        ControlAuthorizationDenied => "CONTROL_AUTHORIZATION_DENIED",
        CompositeActivationCasConflict => "COMPOSITE_ACTIVATION_CAS_CONFLICT",
        CursorContextMismatch => "CURSOR_CONTEXT_MISMATCH",
        CursorExpired => "CURSOR_EXPIRED",
        CursorInvalid => "CURSOR_INVALID",
        DeltaBaseConflict => "DELTA_BASE_CONFLICT",
        DeltaBaseUnresolved => "DELTA_BASE_UNRESOLVED",
        FileContributorUnavailable => "FILE_CONTRIBUTOR_UNAVAILABLE",
        FileOwnershipUnavailable => "FILE_OWNERSHIP_UNAVAILABLE",
        FocusSubjectNotFound => "FOCUS_SUBJECT_NOT_FOUND",
        GenerationIdentityDigestMismatch => "GENERATION_IDENTITY_DIGEST_MISMATCH",
        GenerationIdentityIncomplete => "GENERATION_IDENTITY_INCOMPLETE",
        GenerationIdentityScopeMismatch => "GENERATION_IDENTITY_SCOPE_MISMATCH",
        GenerationImmutable => "GENERATION_IMMUTABLE",
        GenerationManifestFormatUnsupported => "GENERATION_MANIFEST_FORMAT_UNSUPPORTED",
        GenerationManifestMissing => "GENERATION_MANIFEST_MISSING",
        GenerationMismatch => "GENERATION_MISMATCH",
        GenerationNormalizerUnsupported => "GENERATION_NORMALIZER_UNSUPPORTED",
        GenerationNotSealed => "GENERATION_NOT_SEALED",
        GenerationQuarantined => "GENERATION_QUARANTINED",
        GenerationQuarantineContentCorrupt => "GENERATION_QUARANTINE_CONTENT_CORRUPT",
        GenerationQuarantineFormatUnsupported => "GENERATION_QUARANTINE_FORMAT_UNSUPPORTED",
        GenerationQuarantineIdentityDigestMismatch => "GENERATION_QUARANTINE_IDENTITY_DIGEST_MISMATCH",
        GenerationQuarantineIdentityUnreadable => "GENERATION_QUARANTINE_IDENTITY_UNREADABLE",
        GenerationQuarantineNonCanonicalLayout => "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT",
        GenerationQuarantineOrphaned => "GENERATION_QUARANTINE_ORPHANED",
        GenerationQuarantineScopeMismatch => "GENERATION_QUARANTINE_SCOPE_MISMATCH",
        GenerationScrubReceiptInvalid => "GENERATION_SCRUB_RECEIPT_INVALID",
        GenerationSidecarCorrupt => "GENERATION_SIDECAR_CORRUPT",
        GenerationTextAuthorityFormatUnsupported => "GENERATION_TEXT_AUTHORITY_FORMAT_UNSUPPORTED",
        HistoryCursorOrderMismatch => "HISTORY_CURSOR_ORDER_MISMATCH",
        HistoryGenerationNotReady => "HISTORY_GENERATION_NOT_READY",
        HistoryInvalidTimeref => "HISTORY_INVALID_TIMEREF",
        HistoryProducerUnavailable => "HISTORY_PRODUCER_UNAVAILABLE",
        HistoryRelevanceUnavailable => "HISTORY_RELEVANCE_UNAVAILABLE",
        HistoryRepoCommitRecencyUnavailable => "HISTORY_REPO_COMMIT_RECENCY_UNAVAILABLE",
        HistoryShardUnavailable => "HISTORY_SHARD_UNAVAILABLE",
        HistoryTextIndexCorrupt => "HISTORY_TEXT_INDEX_CORRUPT",
        HistoryTextIndexForeignEntry => "HISTORY_TEXT_INDEX_FOREIGN_ENTRY",
        HistoryTextIndexNormalizerUnsupported => "HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED",
        HistoryTextIndexNotReady => "HISTORY_TEXT_INDEX_NOT_READY",
        HistoryTextQueryUnscorable => "HISTORY_TEXT_QUERY_UNSCORABLE",
        HybridFilterUnsupported => "HYBRID_FILTER_UNSUPPORTED",
        IdentityControlCharacter => "IDENTITY_CONTROL_CHARACTER",
        IdentityEmpty => "IDENTITY_EMPTY",
        IdentityNonCanonical => "IDENTITY_NON_CANONICAL",
        IdentityTooLong => "IDENTITY_TOO_LONG",
        IngestResourceBudgetExceeded => "INGEST_RESOURCE_BUDGET_EXCEEDED",
        Internal => "INTERNAL",
        InvalidRequest => "INVALID_REQUEST",
        LexFilterArchivedUnavailable => "LEX_FILTER_ARCHIVED_UNAVAILABLE",
        LexFilterAuthorUnavailable => "LEX_FILTER_AUTHOR_UNAVAILABLE",
        LexFilterCommitterUnavailable => "LEX_FILTER_COMMITTER_UNAVAILABLE",
        LexFilterConflictingSurface => "LEX_FILTER_CONFLICTING_SURFACE",
        LexFilterContextUnavailable => "LEX_FILTER_CONTEXT_UNAVAILABLE",
        LexFilterDirtyUnavailable => "LEX_FILTER_DIRTY_UNAVAILABLE",
        LexFilterForkUnavailable => "LEX_FILTER_FORK_UNAVAILABLE",
        LexFilterInvalidCount => "LEX_FILTER_INVALID_COUNT",
        LexFilterMessageUnavailable => "LEX_FILTER_MESSAGE_UNAVAILABLE",
        LexFilterRevUnavailable => "LEX_FILTER_REV_UNAVAILABLE",
        LexFilterRuntimeCatalogUnavailable => "LEX_FILTER_RUNTIME_CATALOG_UNAVAILABLE",
        LexFilterUnrouted => "LEX_FILTER_UNROUTED",
        LexFilterUnsupportedCombo => "LEX_FILTER_UNSUPPORTED_COMBO",
        LexFilterVisibilityUnavailable => "LEX_FILTER_VISIBILITY_UNAVAILABLE",
        LexicalExaminedBudgetExceeded => "LEXICAL_EXAMINED_BUDGET_EXCEEDED",
        LexicalCollectionBudgetExceeded => "LEXICAL_COLLECTION_BUDGET_EXCEEDED",
        SemanticWorkBudgetExceeded => "SEMANTIC_WORK_BUDGET_EXCEEDED",
        SearchPreviewIntegrity => "SEARCH_PREVIEW_INTEGRITY",
        SymbolCoverageIncomplete => "SYMBOL_COVERAGE_INCOMPLETE",
        SymbolCoverageUnavailable => "SYMBOL_COVERAGE_UNAVAILABLE",
        LexPhrasePlanLimitExceeded => "LEX_PHRASE_PLAN_LIMIT_EXCEEDED",
        LexPhrasePositionsIndexMissing => "LEX_PHRASE_POSITIONS_INDEX_MISSING",
        LexPlannerUnsupportedFilterCombo => "LEX_PLANNER_UNSUPPORTED_FILTER_COMBO",
        LexPlannerUnsupportedNotScope => "LEX_PLANNER_UNSUPPORTED_NOT_SCOPE",
        LexPlannerUnsupportedOrScope => "LEX_PLANNER_UNSUPPORTED_OR_SCOPE",
        LexPredicateUnimplemented => "LEX_PREDICATE_UNIMPLEMENTED",
        LexRawSubstringTrigramIndexMissing => "LEX_RAW_SUBSTRING_TRIGRAM_INDEX_MISSING",
        LexRawSubstringIndexMissing => "LEX_RAW_SUBSTRING_INDEX_MISSING",
        LexRegexBudgetExceeded => "LEX_REGEX_BUDGET_EXCEEDED",
        LexRegexDialectParseError => "LEX_REGEX_DIALECT_PARSE_ERROR",
        LexRegexDialectUnsupported => "LEX_REGEX_DIALECT_UNSUPPORTED",
        LexRegexExecutionInternal => "LEX_REGEX_EXECUTION_INTERNAL",
        LexRegexForbiddenSyntax => "LEX_REGEX_FORBIDDEN_SYNTAX",
        LexRegexInterrupted => "LEX_REGEX_INTERRUPTED",
        LexRegexParseFail => "LEX_REGEX_PARSE_FAIL",
        LexRegexPlanLimitExceeded => "LEX_REGEX_PLAN_LIMIT_EXCEEDED",
        LexRegexQueryTimeout => "LEX_REGEX_QUERY_TIMEOUT",
        LexRegexRegexPrefilterUnusable => "LEX_REGEX_REGEX_PREFILTER_UNUSABLE",
        LexRegexTrigramIndexMissing => "LEX_REGEX_TRIGRAM_INDEX_MISSING",
        LexTextQueryNoTokens => "LEX_TEXT_QUERY_NO_TOKENS",
        LexTextQueryTokenTooLong => "LEX_TEXT_QUERY_TOKEN_TOO_LONG",
        LexTrigramPlanLimitExceeded => "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED",
        LexTrigramPrefilterUnusable => "LEX_TRIGRAM_PREFILTER_UNUSABLE",
        MetricsSourceDefect => "METRICS_SOURCE_DEFECT",
        NotFound => "NOT_FOUND",
        NotImplemented => "NOT_IMPLEMENTED",
        NotReady => "NOT_READY",
        OperationFenceLost => "OPERATION_FENCE_LOST",
        OperationReplayFloor => "OPERATION_REPLAY_FLOOR",
        ProcessMemoryEnvelopeExceeded => "PROCESS_MEMORY_ENVELOPE_EXCEEDED",
        ProcessNotReady => "PROCESS_NOT_READY",
        ProcessRssCeilingExceeded => "PROCESS_RSS_CEILING_EXCEEDED",
        ProtocolVersionUnsupported => "PROTOCOL_VERSION_UNSUPPORTED",
        ProviderEgressDenied => "PROVIDER_EGRESS_DENIED",
        QuarantineTargetNotQuarantined => "QUARANTINE_TARGET_NOT_QUARANTINED",
        QuarantineTargetStillReferenced => "QUARANTINE_TARGET_STILL_REFERENCED",
        QueryCursorGenerationMismatch => "QUERY_CURSOR_GENERATION_MISMATCH",
        QueryCursorUnsupported => "QUERY_CURSOR_UNSUPPORTED",
        ReadViewDomainUndeclared => "READ_VIEW_DOMAIN_UNDECLARED",
        ReadViewGenerationMix => "READ_VIEW_GENERATION_MIX",
        RepoDescriptionUnavailable => "REPO_DESCRIPTION_UNAVAILABLE",
        RepoMetaUnavailable => "REPO_META_UNAVAILABLE",
        RepoTopicUnavailable => "REPO_TOPIC_UNAVAILABLE",
        RequestCancelled => "REQUEST_CANCELLED",
        RequestDeadlineExceeded => "REQUEST_DEADLINE_EXCEEDED",
        ResultTooLarge => "RESULT_TOO_LARGE",
        RollbackCasConflict => "ROLLBACK_CAS_CONFLICT",
        RollbackTargetUnopenable => "ROLLBACK_TARGET_UNOPENABLE",
        RuntimeCatalogChunkUniverseUnavailable => "RUNTIME_CATALOG_CHUNK_UNIVERSE_UNAVAILABLE",
        RuntimeCatalogConflictingBatch => "RUNTIME_CATALOG_CONFLICTING_BATCH",
        RuntimeCatalogHeadMissing => "RUNTIME_CATALOG_HEAD_MISSING",
        RuntimeCatalogNotReady => "RUNTIME_CATALOG_NOT_READY",
        RuntimeCatalogStaleBatch => "RUNTIME_CATALOG_STALE_BATCH",
        RuntimeCatalogUnknownDocId => "RUNTIME_CATALOG_UNKNOWN_DOC_ID",
        RuntimeDirtyOnlyUnsupported => "RUNTIME_DIRTY_ONLY_UNSUPPORTED",
        RuntimeInvalidScope => "RUNTIME_INVALID_SCOPE",
        RuntimeNotReady => "RUNTIME_NOT_READY",
        SearchCorpusAuthorityConflict => "SEARCH_CORPUS_AUTHORITY_CONFLICT",
        SearchCorpusBatchShapeInvalid => "SEARCH_CORPUS_BATCH_SHAPE_INVALID",
        SearchCorpusDeltaBaseNotSealed => "SEARCH_CORPUS_DELTA_BASE_NOT_SEALED",
        SearchCorpusGenerationConflict => "SEARCH_CORPUS_GENERATION_CONFLICT",
        SearchCorpusGenerationRepairRequired => "SEARCH_CORPUS_GENERATION_REPAIR_REQUIRED",
        SearchCorpusHistoryRetentionExhausted => "SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED",
        SearchCorpusHistoryRetentionPolicyInvalid => "SEARCH_CORPUS_HISTORY_RETENTION_POLICY_INVALID",
        SearchTrackGenerationNotSealed => "SEARCH_TRACK_GENERATION_NOT_SEALED",
        SearchTrackManifestDigestMismatch => "SEARCH_TRACK_MANIFEST_DIGEST_MISMATCH",
        SemanticGenerationNotMaterialized => "SEMANTIC_GENERATION_NOT_MATERIALIZED",
        SemanticGenerationNotSealed => "SEMANTIC_GENERATION_NOT_SEALED",
        SemanticManifestDigestMismatch => "SEMANTIC_MANIFEST_DIGEST_MISMATCH",
        SemanticRowRootMismatch => "SEMANTIC_ROW_ROOT_MISMATCH",
        SemanticStreamOwnerScopeOverWindow => "SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW",
        SemanticStreamWindowExceeded => "SEMANTIC_STREAM_WINDOW_EXCEEDED",
        SemanticStreamWindowStillResident => "SEMANTIC_STREAM_WINDOW_STILL_RESIDENT",
        SequenceExhausted => "SEQUENCE_EXHAUSTED",
        ServerOverloaded => "SERVER_OVERLOADED",
        SnapshotUnknown => "SNAPSHOT_UNKNOWN",
        StateRootFormatUnsupported => "STATE_ROOT_FORMAT_UNSUPPORTED",
        StateRootInsecure => "STATE_ROOT_INSECURE",
        StateRootInUse => "STATE_ROOT_IN_USE",
        StateRootSecurityPolicyUnsupported => "STATE_ROOT_SECURITY_POLICY_UNSUPPORTED",
        StrGenerationNotReady => "STR_GENERATION_NOT_READY",
        StrInvalidRequest => "STR_INVALID_REQUEST",
        StrProducerExecutionFailed => "STR_PRODUCER_EXECUTION_FAILED",
        StrShardUnavailable => "STR_SHARD_UNAVAILABLE",
        UnknownGeneration => "UNKNOWN_GENERATION",
    ];
}

impl From<LexicalErrorCode> for SearchPlaneErrorCodeV2 {
    fn from(value: LexicalErrorCode) -> Self {
        Self::Lexical(value)
    }
}

impl fmt::Display for SearchPlaneErrorCodeV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_wire_str())
    }
}

impl Serialize for SearchPlaneErrorCodeV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_wire_str())
    }
}

impl<'de> Deserialize<'de> for SearchPlaneErrorCodeV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ErrorCodeVisitor;
        impl Visitor<'_> for ErrorCodeVisitor {
            type Value = SearchPlaneErrorCodeV2;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a closed SearchPlaneErrorCodeV2 wire string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                SearchPlaneErrorCodeV2::from_wire_str(value).ok_or_else(|| {
                    de::Error::unknown_variant(value, &["<closed SearchPlaneErrorCodeV2 set>"])
                })
            }
        }
        deserializer.deserialize_str(ErrorCodeVisitor)
    }
}

/// Repair class for a typed query failure (J7Q-06).
///
/// Keeps the distinct failure families the bridge / lexical layers already
/// separate from collapsing into one generic "error" at the wire: a UI or
/// operator can branch on the class without string-matching the code. The class
/// is advisory repair metadata only — it never changes the fail-closed outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RepairClass {
    /// The query matched more than one supported target and was refused
    /// fail-closed; the caller must disambiguate.
    Ambiguous,
    /// The construct is not supported on this surface; the caller should switch
    /// to a supported shape.
    Unsupported,
    /// The query was sent to the wrong route family for its intent.
    WrongRoute,
    /// The query shape itself is malformed (e.g. a bad version pin).
    Malformed,
    /// The request continued a read the plane no longer retains (e.g. a
    /// keyset cursor whose auxiliary epoch was pruned); the caller must
    /// start the walk over rather than resume it.
    Expired,
}

impl RepairClass {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Ambiguous => "AMBIGUOUS",
            Self::Unsupported => "UNSUPPORTED",
            Self::WrongRoute => "WRONG_ROUTE",
            Self::Malformed => "MALFORMED",
            Self::Expired => "EXPIRED",
        }
    }

    /// Inverse of [`RepairClass::as_code_str`]; `None` on unknown input.
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "AMBIGUOUS" => Self::Ambiguous,
            "UNSUPPORTED" => Self::Unsupported,
            "WRONG_ROUTE" => Self::WrongRoute,
            "MALFORMED" => Self::Malformed,
            "EXPIRED" => Self::Expired,
            _ => return None,
        };
        Some(v)
    }
}

impl Serialize for RepairClass {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for RepairClass {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RepairClassVisitor;
        impl Visitor<'_> for RepairClassVisitor {
            type Value = RepairClass;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("RepairClass SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E>(self, value: &str) -> Result<RepairClass, E>
            where
                E: de::Error,
            {
                RepairClass::from_code_str(value).ok_or_else(|| {
                    de::Error::unknown_variant(
                        value,
                        &[
                            "AMBIGUOUS",
                            "UNSUPPORTED",
                            "WRONG_ROUTE",
                            "MALFORMED",
                            "EXPIRED",
                        ],
                    )
                })
            }
        }
        deserializer.deserialize_str(RepairClassVisitor)
    }
}

/// Typed repair metadata attached to a query failure (J7Q-06).
///
/// Carries the failure [`RepairClass`], the supported alternative shapes /
/// example queries the caller can switch to, and an optional docs anchor — all
/// as typed fields so CLI and SDK consumers render the same guidance from one
/// payload instead of re-deriving hints from prose. This is additive, advisory
/// metadata: it never rewrites the query and never softens the fail-closed code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryErrorRepair {
    pub class: RepairClass,
    pub supported_alternatives: Vec<String>,
    pub docs_anchor: Option<String>,
}

const QUERY_ERROR_REPAIR_FIELDS: &[&str] = &["class", "supported_alternatives", "docs_anchor"];

impl Serialize for QueryErrorRepair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 2usize;
        if self.docs_anchor.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("QueryErrorRepair", field_count)?;
        state.serialize_field("class", &self.class)?;
        state.serialize_field("supported_alternatives", &self.supported_alternatives)?;
        if let Some(anchor) = &self.docs_anchor {
            state.serialize_field("docs_anchor", anchor)?;
        }
        state.end()
    }
}

struct QueryErrorRepairVisitor;

impl<'de> Visitor<'de> for QueryErrorRepairVisitor {
    type Value = QueryErrorRepair;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a QueryErrorRepair map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut class: Option<RepairClass> = None;
        let mut supported_alternatives: Option<Vec<String>> = None;
        let mut docs_anchor: Option<String> = None;
        let mut docs_anchor_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "class" => {
                    if class.is_some() {
                        return Err(de::Error::duplicate_field("class"));
                    }
                    class = Some(map.next_value()?);
                }
                "supported_alternatives" => {
                    if supported_alternatives.is_some() {
                        return Err(de::Error::duplicate_field("supported_alternatives"));
                    }
                    supported_alternatives = Some(map.next_value()?);
                }
                "docs_anchor" => {
                    if docs_anchor_seen {
                        return Err(de::Error::duplicate_field("docs_anchor"));
                    }
                    docs_anchor_seen = true;
                    docs_anchor = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, QUERY_ERROR_REPAIR_FIELDS));
                }
            }
        }
        Ok(QueryErrorRepair {
            class: class.ok_or_else(|| de::Error::missing_field("class"))?,
            supported_alternatives: supported_alternatives
                .ok_or_else(|| de::Error::missing_field("supported_alternatives"))?,
            docs_anchor,
        })
    }
}

impl<'de> Deserialize<'de> for QueryErrorRepair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "QueryErrorRepair",
            QUERY_ERROR_REPAIR_FIELDS,
            QueryErrorRepairVisitor,
        )
    }
}

/// Wire code for a response the server computed but cannot put on the wire
/// because its encoded body exceeds the frame limit (QI-BB-005).
///
/// Before this code the connection was simply closed after the work was
/// done, so the caller could not tell an oversized answer from a crash. The
/// typed refusal names both sizes so the caller can narrow `top_k` or the
/// projection.
pub const ERR_RESULT_TOO_LARGE: SearchPlaneErrorCodeV2 = SearchPlaneErrorCodeV2::ResultTooLarge;

/// Wire code for a request the server could not admit to a dispatch slot
/// within its queue wait (QI-BB-002). Nothing was executed; the caller may
/// retry with backoff.
pub const ERR_SERVER_OVERLOADED: SearchPlaneErrorCodeV2 = SearchPlaneErrorCodeV2::ServerOverloaded;

/// Wire-level typed error carried in every search-plane IPC response.
///
/// `code` + `message` are the load-bearing fail-closed fields; `repair` is
/// optional typed guidance (J7Q-06). Every current-format error writes
/// `repair` explicitly, including `None`.
/// Missing or unknown fields are rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneIpcError {
    pub code: SearchPlaneErrorCodeV2,
    pub message: String,
    pub repair: Option<QueryErrorRepair>,
}

impl SearchPlaneIpcError {
    /// The refusal a transport sends when every dispatch slot stayed busy
    /// for `waited`; `slots` is the server's concurrency so the caller can
    /// size its backoff.
    #[must_use]
    pub fn overloaded(waited: core::time::Duration, slots: usize) -> Self {
        Self {
            code: ERR_SERVER_OVERLOADED,
            message: format!(
                "no dispatch slot came free within {} ms ({slots} slots busy); retry with backoff",
                waited.as_millis()
            ),
            repair: None,
        }
    }

    /// The refusal a transport sends in place of a response whose encoded
    /// body of `encoded_bytes` exceeds `limit_bytes`.
    #[must_use]
    pub fn result_too_large(encoded_bytes: u64, limit_bytes: u64) -> Self {
        Self {
            code: ERR_RESULT_TOO_LARGE,
            message: format!(
                "response body of {encoded_bytes} bytes exceeds the {limit_bytes}-byte frame limit; narrow top_k or the projection"
            ),
            repair: None,
        }
    }
}

const SEARCH_PLANE_IPC_ERROR_FIELDS: &[&str] = &["code", "message", "repair"];

impl Serialize for SearchPlaneIpcError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneIpcError", 3)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("message", &self.message)?;
        state.serialize_field("repair", &self.repair)?;
        state.end()
    }
}

struct SearchPlaneIpcErrorVisitor;

impl<'de> Visitor<'de> for SearchPlaneIpcErrorVisitor {
    type Value = SearchPlaneIpcError;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIpcError map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut code: Option<SearchPlaneErrorCodeV2> = None;
        let mut message: Option<String> = None;
        let mut repair: Option<QueryErrorRepair> = None;
        let mut repair_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "code" => {
                    if code.is_some() {
                        return Err(de::Error::duplicate_field("code"));
                    }
                    code = Some(map.next_value()?);
                }
                "message" => {
                    if message.is_some() {
                        return Err(de::Error::duplicate_field("message"));
                    }
                    message = Some(map.next_value()?);
                }
                "repair" => {
                    if repair_seen {
                        return Err(de::Error::duplicate_field("repair"));
                    }
                    repair_seen = true;
                    repair = map.next_value()?;
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_IPC_ERROR_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneIpcError {
            code: code.ok_or_else(|| de::Error::missing_field("code"))?,
            message: message.ok_or_else(|| de::Error::missing_field("message"))?,
            repair: if repair_seen {
                repair
            } else {
                return Err(de::Error::missing_field("repair"));
            },
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIpcError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneIpcError",
            SEARCH_PLANE_IPC_ERROR_FIELDS,
            SearchPlaneIpcErrorVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{QueryErrorRepair, RepairClass, SearchPlaneErrorCodeV2, SearchPlaneIpcError};
    use crate::lex::LexicalErrorCode;

    fn cbor_roundtrip_error(value: &SearchPlaneIpcError) -> SearchPlaneIpcError {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(value, &mut buf).expect("serialize");
        ciborium::de::from_reader(buf.as_slice()).expect("deserialize")
    }

    #[test]
    fn repair_class_roundtrip_all_variants() {
        for class in [
            RepairClass::Ambiguous,
            RepairClass::Unsupported,
            RepairClass::WrongRoute,
            RepairClass::Malformed,
            RepairClass::Expired,
        ] {
            assert_eq!(RepairClass::from_code_str(class.as_code_str()), Some(class));
        }
        assert_eq!(RepairClass::from_code_str("NOPE"), None);
    }

    #[test]
    fn error_without_repair_roundtrips() {
        let err = SearchPlaneIpcError {
            code: SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode::BridgeTranslateFail),
            message: "boom".to_string(),
            repair: None,
        };
        assert_eq!(cbor_roundtrip_error(&err), err);
    }

    #[test]
    fn error_with_repair_roundtrips() {
        let err = SearchPlaneIpcError {
            code: SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode::BridgeAmbiguousFilter),
            message: "filter resolves to 2 targets".to_string(),
            repair: Some(QueryErrorRepair {
                class: RepairClass::Ambiguous,
                supported_alternatives: vec!["repo:".to_string(), "file:".to_string()],
                docs_anchor: Some("docs/query#ambiguous".to_string()),
            }),
        };
        assert_eq!(cbor_roundtrip_error(&err), err);
    }

    #[test]
    fn repair_without_docs_anchor_roundtrips() {
        let err = SearchPlaneIpcError {
            code: SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode::BridgeUnsupportedFilter),
            message: "no projection".to_string(),
            repair: Some(QueryErrorRepair {
                class: RepairClass::Unsupported,
                supported_alternatives: vec!["content:".to_string()],
                docs_anchor: None,
            }),
        };
        assert_eq!(cbor_roundtrip_error(&err), err);
    }

    #[test]
    fn legacy_two_field_wire_is_refused() {
        // A peer that predates J7Q-06 writes only {code, message}; this
        // current-format decoder requires an explicit repair field.
        let map: std::collections::BTreeMap<String, String> = [
            ("code".to_string(), "NOT_READY".to_string()),
            ("message".to_string(), "warming".to_string()),
        ]
        .into_iter()
        .collect();
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(&map, &mut buf).expect("serialize legacy");
        let error = ciborium::de::from_reader::<SearchPlaneIpcError, _>(buf.as_slice())
            .expect_err("old error wire must refuse");
        assert!(error.to_string().contains("repair"), "{error}");
    }
}
