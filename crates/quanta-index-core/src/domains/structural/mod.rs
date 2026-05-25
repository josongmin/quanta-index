//! LXE-09 structural domain — defines the parse-tree-backed structural query
//! surface and Option B fail-closed semantics. Re-export facade only; no
//! inline items per module-discipline lint.

mod inbound;
mod outbound;
mod policy;
mod service;

pub use inbound::{StructuralQueryRequest, StructuralQueryResponse};
pub use outbound::{StructuralError, StructuralProducerPort, StructuralReadiness};
pub use policy::StructuralPolicy;
pub use service::StructuralService;
