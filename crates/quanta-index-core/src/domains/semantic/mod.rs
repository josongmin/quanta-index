//! Semantic domain — owns vector index build, open, and query.

mod admission;
mod inbound;
mod outbound;
mod service;
mod stream;

pub use admission::{
    AdmittedSemanticInputV1, EmbeddingOutcomeV1, PROVIDER_BUDGET_EXHAUSTED_CODE,
    PROVIDER_EGRESS_DENIED_CODE, PROVIDER_WORK_CANCELLED_CODE, ProviderBudgetLedger,
    ProviderBudgetSnapshotV1, ProviderReservationTicketV1, ProviderSettlementKindV1,
    ProviderSettlementReceiptV1, ProviderSettlementUsageV1, ProviderSupervisorEnrollmentV1,
    ProviderWorkBudgetV1, ProviderWorkEstimateV1, SemanticAdmissionEngine, SemanticEgressGrantV1,
    SemanticEgressPolicyV1, SemanticInputClass,
};
pub use inbound::SemanticQueryPort;
pub use outbound::{
    DenseIndexBuildV1, DenseIndexEffortV1, DenseIndexSegmentBuildV1, DenseIndexTrainingV1,
    DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1, EMBED_CHECKPOINT,
    SEMANTIC_ROW_ROOT_MISMATCH_CODE, SemanticContentRootsPort, SemanticIndexOpenPort,
    SemanticIngestPort, SemanticReadiness, SemanticSearchHitV1, SemanticSearcher,
    TextEmbeddingProvider,
};
pub use service::{
    L2_UNIT_NORM_TOLERANCE, L2UnitEmbeddingProvider, RawNormTallies, SemanticPolicy,
};
pub use stream::{
    ResidentScopeSource, SEMANTIC_STREAM_OWNER_SCOPE_OVER_WINDOW_CODE,
    SEMANTIC_STREAM_WINDOW_EXCEEDED_CODE, SEMANTIC_STREAM_WINDOW_SCOPES,
    SEMANTIC_STREAM_WINDOW_STILL_RESIDENT_CODE, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES,
    SemanticBatchIdentityV1, SemanticBatchMutationsV1, SemanticGenerationContractV1,
    SemanticIngestHeaderV1, SemanticScopeSource, SemanticScopeStreamBuildPort,
    SemanticScopeWindowV1, SemanticStreamTallyV1, SemanticStreamWindowPolicy, SemanticWindowFillV1,
    SemanticWindowIssuerV1, SemanticWindowLeaseV1, SemanticWindowPlacementV1,
    SemanticWindowResidencyV1, build_resident_semantic_batch_v1, owner_key_v1,
};
