//! Lexical domain — owns lexical index build, open, and query.

mod inbound;
mod outbound;
mod service;

pub use inbound::LexicalQueryPort;
pub use outbound::{
    FileContributorIngestPort, FileOwnershipIngestPort, LexicalBatchBuildPort,
    LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalIngestPort, LexicalReadiness,
    LexicalSearcher, RepoCommitRecencyIngestPort, RepoDescriptionIngestPort, RepoMetaIngestPort,
    RepoTopicIngestPort,
};
pub use service::LexicalPolicy;
