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
    LEXICAL_EXAMINED_BUDGET_EXCEEDED_CODE, LexicalExecutionBudgetV1, LexicalPolicy,
    RegexMatchCachePolicy, RegexMatchCacheStats,
};
