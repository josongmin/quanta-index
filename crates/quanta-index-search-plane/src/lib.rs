#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Application-layer search-plane orchestration.
//!
//! This crate owns readiness state, channel-driven materialization dispatch,
//! lexical lowering, and the cross-domain query orchestration that sits
//! between transport and the core domain ports.

mod channel_dispatcher;
mod control_dispatcher;
mod ingest_dispatcher;
mod lowering;
mod query_dispatcher;
mod readiness;

pub use channel_dispatcher::ChannelDispatcher;
pub use control_dispatcher::SearchPlaneControlDispatcher;
pub use ingest_dispatcher::{
    ChannelLexicalIngestAdapter, ChannelSemanticIngestAdapter, SearchPlaneIngestDispatcher,
};
pub use lowering::{lower_lexical_text_query, lower_sourcegraph_query_text};
pub use query_dispatcher::{
    SearchPlaneDispatcher, SearchPlaneQueryDispatcher, SearchPlaneQueryService, make_pin,
};
pub use readiness::{ActivationCatalog, ActiveGenerationRecord, Ledger, TrackLedger};
