//! Lexical domain — owns lexical index build, open, and query.

mod inbound;
mod outbound;
mod service;

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
};
