//! Lexical domain — owns lexical index build, open, and query.

mod collection_budget;
mod coverage;
mod history_text;
mod inbound;
mod outbound;
mod query_plan;
mod service;

pub use collection_budget::{LexicalCollectionBudget, LexicalMemoryReservation};
pub use coverage::require_complete_symbol_coverage;
pub use query_plan::{LexicalEndpoint, LexicalPlanKind, ValidatedLexicalPlan};

pub use history_text::{
    HISTORY_TEXT_INDEX_CORRUPT_CODE, HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE,
    HISTORY_TEXT_INDEX_NOT_READY_CODE, HISTORY_TEXT_QUERY_UNSCORABLE_CODE, HistoryTextAdmitFn,
    HistoryTextBuildV1, HistoryTextDiscardOutcomeV1, HistoryTextDocKeyV1, HistoryTextDocV1,
    HistoryTextEpochReceiptV1, HistoryTextEpochStatusV1, HistoryTextHitV1, HistoryTextIndexPort,
    HistoryTextKindV1, HistoryTextPageV1, HistoryTextQueryV1, HistoryTextSearcher,
};
pub use inbound::LexicalQueryPort;
pub use outbound::{
    CodeSearchRankStudyV1, CodeSearchScoreComponentsV1, FileContributorIngestPort,
    FileOwnershipIngestPort, LexicalCandidateExplanationV1, LexicalIndexBuildPort,
    LexicalIndexOpenPort, LexicalPageSpec, LexicalReadiness, LexicalScoreEngineV1,
    LexicalScoreTraceV1, LexicalSearchPageV1, LexicalSearcher, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMetaIngestPort, RepoTopicIngestPort, SearchCorpusBatchBuildPort,
    SearchCorpusIngestPort, SearchCorpusPreflightPhaseV1, SymbolSearchPageV1,
};
pub use service::{
    LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE, LEXICAL_WRITER_HEAP_BYTES_MAX,
    LEXICAL_WRITER_HEAP_BYTES_MIN, LexicalExecutionBudgetV1, LexicalPolicy,
    LexicalWriterCacheStats, LexicalWriterPolicy, RegexMatchCachePolicy, RegexMatchCacheStats,
    TextAuthorityUpdateStats,
};
