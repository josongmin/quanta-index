#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Application-layer search-plane orchestration.
//!
//! This crate owns readiness state, direct authority materialization,
//! lexical lowering, and the cross-domain query orchestration that sits
//! between transport and the core domain ports.

mod control_dispatcher;
mod ingest_dispatcher;
mod lowering;
mod query_dispatcher;
mod query_embedder;
pub mod readiness;

pub use control_dispatcher::SearchPlaneControlDispatcher;
pub use ingest_dispatcher::{
    DirectHistoryMaterializer, DirectLexicalMaterializer, DirectRuntimeMetadataMaterializer,
    DirectSemanticMaterializer, DirectStructuralMaterializer, HistoryIngestPort,
    LegacySemanticJournalStore, RuntimeMetadataIngestPort, SEARCH_OWNED_SEMANTIC_DIMENSION,
    SearchPlaneIngestDispatcher, StructuralIngestPort,
};
pub use lowering::{lower_lexical_text_query, lower_sourcegraph_query_text};
pub use quanta_index_lq_obs::{MetricSample, ObsError};
pub use query_dispatcher::{
    BoundedQueryObsStore, QueryObsSink, SearchPlaneDispatcher, SearchPlaneQueryDispatcher,
    SearchPlaneQueryService, make_pin, repair_for_code,
};
pub use query_embedder::{HashingQueryTextEmbedder, QueryTextEmbedderPort};
pub use readiness::{
    ActivationCatalog, ActiveGenerationRecord, AuxiliaryAuthorityStore, Ledger, TrackLedger,
};
