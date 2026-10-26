//! Native incremental publication coordinator. Durable authority lives in the
//! injected catalog; immutable objects and persistent maps are capabilities.
#![forbid(unsafe_code)]

mod attempt_reads;
mod binding_metadata;
mod boot;
mod build_staging;
mod diagnostic;
mod frontier;
mod identity;
mod member_staging;
mod mutation_staging;
mod path_staging;
mod profile;
mod projection;
mod query;
mod registry;
mod request_decode;
mod request_preparation;
mod source;
mod spool;
mod staging_payload;
mod ticket;
mod update;

pub use boot::verify_native_boot_inputs;
pub use quanta_index_core::index::{SourceChange, SourceDelta};
pub use query::QueryAdapterRegistry;
pub use registry::{AdapterDescriptor, AdapterRegistry};
pub use ticket::ReadTicket;
pub use update::{
    ControlCatalog, Kernel, KernelServices, OperationExecutionClass, PinnedQuerySession,
    PreparedQuery, PreparedQueryStage, ResumeDispatch,
};
