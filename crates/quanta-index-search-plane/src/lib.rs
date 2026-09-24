#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Application-layer search-plane orchestration.
//!
//! This crate owns readiness state, direct authority materialization,
//! lexical lowering, and the cross-domain query orchestration that sits
//! between transport and the core domain ports.

mod auxiliary_authority;
#[cfg(test)]
mod content_roots_test_support;
mod control_dispatcher;
pub mod crash_point;
#[cfg(test)]
mod door_findings_test_support;
mod history_text;
mod ingest_dispatcher;
mod lowering;
mod observability;
mod post_durable;
mod quarantine;
mod query_dispatcher;
mod query_embedder;
pub mod readiness;
mod search_corpus_lifecycle;
mod search_corpus_retention;
mod semantic_derive;
mod single_flight;
mod snapshot_registry;

pub use control_dispatcher::{
    ControlAccessV1, ControlCapabilityV1, ProcessReadinessPort, SearchPlaneControlDispatcher,
    SearchPlaneControlDispatcherParts,
};
pub use history_text::{HistoryTextHandles, HistoryTextIndexParts};
pub use ingest_dispatcher::{
    AuxiliaryMaterializerParts, AuxiliaryMutationCoordinator, DirectHistoryMaterializer,
    DirectRuntimeMetadataMaterializer, DirectSearchCorpusMaterializer, DirectSemanticMaterializer,
    DirectStructuralMaterializer, HistoryIngestPort, IngestResourceStats,
    RuntimeMetadataIngestPort, SearchCorpusAuthorityInspectPort, SearchCorpusAuthorityWritePort,
    SearchCorpusGcStats, SearchCorpusMaterializerParts, SearchPlaneIngestDispatcher,
    SemanticIngestStreamStats, StructuralIngestPort,
};
pub use lowering::{lower_lexical_text_query, lower_sourcegraph_query_text};
pub use observability::{
    BoundedQueryObsStore, HISTOGRAM_BUCKET_BOUNDS, MAX_OBS_ERRORS, MAX_OBS_SAMPLES,
    ObservabilityScrape, QueryObsSink,
};
pub use quanta_index_core::RequestBudgetV1;
pub use quanta_index_lq_obs::{MetricSample, ObsError};
pub use quarantine::{
    OrphanedSealedGenerationV1, QUARANTINE_TARGET_STILL_REFERENCED_CODE, QuarantineService,
    QuarantineServiceParts, partition_sealed_inventory_v1,
};
pub use query_dispatcher::{
    CursorKeyStore, RESPONSE_ENVELOPE_RESERVE_BYTES, ResponsePayloadBudget, SearchPlaneDispatcher,
    SearchPlaneQueryDispatcher, SearchPlaneQueryService, make_pin, repair_for_code,
};
pub use query_embedder::{
    HashingQueryTextEmbedder, ProviderBoundaryQueryEmbedder, QueryTextEmbedderPort,
    SEARCH_OWNED_SEMANTIC_DIMENSION, SEARCH_OWNED_SEMANTIC_MODEL_ID,
    SEARCH_OWNED_SEMANTIC_MODEL_REVISION, admit_source_derive_content,
};
pub use readiness::{
    ActivationCatalog, ActiveGenerationRecord, AuxiliaryAuthorityStore, Ledger,
    PairIndexBytesMeasurer, PreparedSearchCorpusGenerationV1, SealedSearchCorpusAuthorityStateV1,
    SearchCorpusGenerationActivationV1, SearchCorpusGenerationV1, SearchCorpusIndexBytesPort,
    TrackLedger,
};
pub use search_corpus_lifecycle::{
    ActivationPromotionParts, SearchCorpusLifecycleOwner, SearchCorpusLifecycleParts,
};
pub use snapshot_registry::{
    OpenedSnapshot, SnapshotAcquireOutcome, SnapshotAcquired, SnapshotKey, SnapshotPromoteOutcome,
    SnapshotRegistries, SnapshotRegistry, SnapshotRegistryPolicy, SnapshotRegistryStats,
    SnapshotRetireOutcome,
};
