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

/// Inspect one original-funded durable request without releasing its decode
/// backing before the caller has applied an installed policy gate.
pub fn with_decoded_update_request_v1<R>(
    bytes: &[u8],
    budget: &mut quanta_index_core::index::WorkBudget,
    inspect: impl FnOnce(
        &quanta_index_core::index::UpdateRequest,
        &mut quanta_index_core::index::WorkBudget,
    ) -> R,
) -> Result<R, quanta_index_core::index::IndexError> {
    let loaded = request_decode::decode(bytes, budget)?;
    Ok(inspect(&loaded, budget))
}

/// Inspect an existing immutable profile under the original request budget.
pub fn with_loaded_profile_v1<R>(
    reference: quanta_index_core::index::ObjectRef,
    objects: &dyn quanta_index_core::index::ports::ImmutableObjectPort,
    budget: &mut quanta_index_core::index::WorkBudget,
    inspect: impl FnOnce(
        &quanta_index_core::index::IndexProfile,
        &mut quanta_index_core::index::WorkBudget,
    ) -> R,
) -> Result<R, quanta_index_core::index::IndexError> {
    let loaded = profile::load(reference, objects, budget)?;
    Ok(inspect(&loaded, budget))
}
mod request_preparation;
mod source;
mod spool;
mod staging_payload;
mod ticket;
mod update;

pub use boot::{
    RetainedSourceTextPolicyV20, native_boot_source_genesis_v1,
    verify_native_boot_candidate_inputs, verify_native_boot_captured_request_v1,
    verify_native_boot_inputs, verify_native_boot_retained_source_inputs_v20,
    verify_native_boot_source_target_v1, verify_native_published_full_target_v1,
    verify_native_published_source_history_target_v1, verify_native_published_source_target_v1,
    with_native_boot_history_full_v1, with_native_boot_history_request_v1,
    with_native_boot_source_history_initial_v1,
};
pub use projection::{ProjectionCommitBasis, ProjectionCommitValidator};
pub use quanta_index_core::index::{SourceChange, SourceDelta};
pub use query::QueryAdapterRegistry;
pub use registry::{AdapterDescriptor, AdapterRegistry};
pub use ticket::{ReadTicket, TicketReadAttempt};
pub use update::{
    ControlCatalog, Kernel, KernelServices, OperationExecutionClass, PinnedQuerySession,
    PreparedQuery, PreparedQueryStage, QueryExecutionTimings, ReadSessionProfile, ResumeDispatch,
    identify_update_request_with,
};
