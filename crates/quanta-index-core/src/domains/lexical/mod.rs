//! Lexical domain — owns lexical index build, open, and query.

mod history_text;
mod inbound;
mod outbound;
mod service;

pub use history_text::{
    HISTORY_TEXT_INDEX_CORRUPT_CODE, HISTORY_TEXT_INDEX_NORMALIZER_UNSUPPORTED_CODE,
    HISTORY_TEXT_INDEX_NOT_READY_CODE, HISTORY_TEXT_QUERY_UNSCORABLE_CODE, HistoryTextAdmitFn,
    HistoryTextBuildV1, HistoryTextDiscardOutcomeV1, HistoryTextDocKeyV1, HistoryTextDocV1,
    HistoryTextEpochReceiptV1, HistoryTextEpochStatusV1, HistoryTextHitV1, HistoryTextIndexPort,
    HistoryTextKindV1, HistoryTextPageV1, HistoryTextQueryV1, HistoryTextSearcher,
};
pub use inbound::LexicalQueryPort;
pub use outbound::{
    FileContributorIngestPort, FileOwnershipIngestPort, LexicalCandidateExplanationV1,
    LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalReadiness, LexicalScoreEngineV1,
    LexicalScoreTraceV1, LexicalSearchPageV1, LexicalSearcher, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMetaIngestPort, RepoTopicIngestPort, SearchCorpusBatchBuildPort,
    SearchCorpusIngestPort,
};
pub use service::{
    LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE, LEXICAL_WRITER_HEAP_BYTES_MAX,
    LEXICAL_WRITER_HEAP_BYTES_MIN, LexicalExecutionBudgetV1, LexicalPolicy,
    LexicalWriterCacheStats, LexicalWriterPolicy, RegexMatchCachePolicy, RegexMatchCacheStats,
    TextAuthorityUpdateStats,
};
