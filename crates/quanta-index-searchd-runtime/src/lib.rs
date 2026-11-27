#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Concrete runtime wiring for the search-plane daemon.

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticHealthCodeV1, DiagnosticSpanGuardV1, DiagnosticTraceErrorV1,
};
#[cfg(unix)]
pub mod canonical_publication_admission;
#[cfg(unix)]
mod catalog_lock_files;
#[cfg(unix)]
mod deployment_selection;
#[cfg(unix)]
pub mod durable_control_admission;
pub mod native;
mod native_authorization;
mod native_auto_signals;
mod native_catalog_cache_diagnostics;
mod native_catalog_retirement;
mod native_code_search_excerpt;
mod native_dispatch;
pub mod native_effort;
mod native_effort_ports;
mod native_effort_transport;
mod native_graph_transport;
mod native_interaction;
mod native_interaction_transport;
mod native_lexical_scope;
#[cfg(unix)]
pub mod native_object_read_io;
#[cfg(unix)]
pub mod native_object_reclaim_io;
#[cfg(unix)]
pub mod native_object_write_io;
mod native_overlay;
mod native_overlay_query;
mod native_overlay_transport;
mod native_producer_source_lookup;
pub mod native_product;
mod native_progress_observation;
mod native_projection_owner_lookup;
mod native_projection_source_diff;
mod native_provider_custody;
mod native_provider_signals;
mod native_pump;
mod native_query_lowering;
mod native_read_session_transport;
mod native_read_sessions;
mod native_reclaim_driver;
mod native_reclaim_observation;
#[cfg(unix)]
pub mod native_recovery;
mod native_release_deadline;
mod native_reply;
mod native_repository_path_transport;
mod native_resources;
mod native_result;
mod native_retention;
mod native_retention_observation;
#[cfg(unix)]
pub mod native_scratch_io;
mod native_semantic_result;
mod native_semantic_scope;
mod native_source_owner_lookup;
mod native_stub_certificate_projection_v1;
mod native_stub_transport;
mod native_text_match_transport;
mod native_text_result;
mod native_wal_reclaim;
mod native_work_diagnostic;
mod native_worker;
#[cfg(unix)]
mod owned_fd_close;
#[cfg(unix)]
pub mod publication_control_files;
#[cfg(unix)]
pub mod publication_file_capacity;
#[cfg(unix)]
pub mod publication_file_deletions;
/// Bounded boot inventory for the composition-owned publication authority.
///
/// This does not install whole-publication admission in the default runtime.
#[cfg(unix)]
pub mod publication_inventory;
#[cfg(all(test, unix, feature = "legacy-semantic-adapter"))]
mod publication_lance_io;
pub mod publication_lexical_lifetime;
#[cfg(unix)]
pub mod publication_sidecar;
#[cfg(all(unix, feature = "legacy-semantic-adapter"))]
pub mod semantic_file_factory;
#[cfg(unix)]
pub mod source_upload_file_admission;
#[cfg(all(unix, feature = "sqlite-owner-vfs-grant"))]
pub mod sqlite_owner_memory_grant;

fn runtime_diagnostic_span(
    context: &DiagnosticContextV1,
    result: std::result::Result<DiagnosticSpanGuardV1, DiagnosticTraceErrorV1>,
) -> Option<DiagnosticSpanGuardV1> {
    result.map_or_else(
        |error| {
            context.health(match error {
                DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    DiagnosticHealthCodeV1::CounterOverflow
                }
                DiagnosticTraceErrorV1::ClockRegressed => DiagnosticHealthCodeV1::ClockRegressed,
                DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                DiagnosticTraceErrorV1::MissingParentSpan => {
                    DiagnosticHealthCodeV1::MissingParentSpan
                }
            });
            None
        },
        Some,
    )
}

// Share the real-service fixture with crate-private fault tests.
#[cfg(test)]
extern crate self as quanta_index_searchd_runtime;

#[cfg(all(test, unix))]
mod native_owned_pump_fault_tests;
