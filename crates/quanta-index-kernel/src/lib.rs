//! Native incremental publication coordinator. Durable authority lives in the
//! injected catalog; immutable objects and persistent maps are capabilities.
#![forbid(unsafe_code)]

mod identity;
mod projection;
mod registry;
mod source;
mod spool;
mod ticket;
mod update;

pub use quanta_index_core::index::{SourceChange, SourceDelta};
pub use registry::{AdapterDescriptor, AdapterRegistry};
pub use ticket::ReadTicket;
pub use update::{ControlCatalog, Kernel, KernelServices, OperationExecutionClass, ResumeDispatch};
