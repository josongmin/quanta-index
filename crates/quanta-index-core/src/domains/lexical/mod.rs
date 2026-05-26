//! Lexical domain — owns lexical index build, open, and query.

mod inbound;
mod outbound;
mod service;

pub use inbound::LexicalQueryPort;
pub use outbound::{
    LexicalBatchBuildPort, LexicalIndexBuildPort, LexicalIndexOpenPort, LexicalIngestPort,
    LexicalReadiness, LexicalSearcher,
};
pub use service::LexicalPolicy;
