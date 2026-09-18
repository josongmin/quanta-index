//! Search-plane ingest orchestration (QI-RT-01).
//!
//! Producer sends a typed [`SearchPlaneIngestIpcRequest`] over UDS
//! `ingest.sock`. This dispatcher routes the typed batch to owner materializer
//! ports. The concrete runtime may choose to mirror accepted batches into
//! legacy channel persistence, but channel row-op fanout is no longer the
//! public ingest truth.
//!
//! Composition root in `quanta-index-searchd` is the only place that names
//! concrete adapter types (materializers, channel mirrors, repo-map ingest);
//! this module holds only [`Arc<dyn ...Port>`] (CLAUDE.md DIP rule).
//!
//! Module map (dependencies point downward only):
//!
//! - `dispatcher` — `SearchPlaneIngestDispatcher`. Depends on `ports`,
//!   `errors`.
//! - `search_corpus` — the search-corpus materializer. Depends on
//!   `generation_plan`, `auxiliary` (the mutation coordinator it holds),
//!   `ports`, `errors`.
//! - `auxiliary` — the auxiliary-authority materializers and their mutation
//!   coordinator. Depends on `ports`.
//! - `semantic` — the semantic-only materializer. Leaf.
//! - `generation_plan`, `ports`, `errors` — leaves.
//!
//! [`SearchPlaneIngestIpcRequest`]: quanta_index_contract::SearchPlaneIngestIpcRequest

mod auxiliary;
mod dispatcher;
mod errors;
mod generation_plan;
mod ports;
mod search_corpus;
mod semantic;

pub use auxiliary::{
    AuxiliaryMaterializerParts, AuxiliaryMutationCoordinator, DirectHistoryMaterializer,
    DirectRuntimeMetadataMaterializer, DirectStructuralMaterializer,
};
pub use dispatcher::SearchPlaneIngestDispatcher;
pub use ports::{
    HistoryIngestPort, RuntimeMetadataIngestPort, SearchCorpusAuthorityInspectPort,
    SearchCorpusAuthorityWritePort, StructuralIngestPort,
};
pub use search_corpus::{
    DirectSearchCorpusMaterializer, IngestResourceStats, SearchCorpusGcStats,
    SearchCorpusMaterializerParts,
};
pub use semantic::{DirectSemanticMaterializer, SemanticIngestStreamStats};

#[cfg(test)]
mod tests;
