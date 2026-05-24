//! Hybrid domain — RRF fusion + Explain. The only domain allowed to consume
//! lexical and semantic public ports.

mod inbound;
mod service;

pub use inbound::{ExplainQueryPort, HybridQueryPort};
pub use service::HybridOrchestratorPolicy;
