//! Hybrid domain — RRF fusion + Explain. The only domain allowed to consume
//! lexical and semantic public ports.

mod dense_admission;
mod inbound;
mod service;
mod trace_file_rank;

pub use dense_admission::{
    DenseAdmissionOutcomeV1, DenseLaneFilterClassV1, HYBRID_FILTER_UNSUPPORTED_CODE,
    HybridFilterPlanV1, classify_hybrid_filter_v1, dense_admission_round_outcome_v1,
    hybrid_filter_name_v1,
};
pub use inbound::{ExplainQueryPort, HybridQueryPort};
pub use service::{FusedKeyV1, FusedLaneRankV1, HybridFetchFloorPolicy, HybridOrchestratorPolicy};
pub use trace_file_rank::{
    TraceFileEvidenceV1, TraceFileHitV1, TraceFileKeyV1, TraceFileLaneWitnessV1,
    TraceFileRankPolicyV1, TraceSourceFrameClassV1,
};
