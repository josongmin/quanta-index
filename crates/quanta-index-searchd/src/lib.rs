#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Composition root for the search-plane daemon.

pub mod app;
pub mod cli;

pub use app::trace_recorder::{
    DiagnosticCaptureCloseErrorV1, DiagnosticCaptureClosedV1, DiagnosticCaptureLeaseV1,
    DiagnosticCaptureLimitsV1, DiagnosticCaptureStatusV1, DiagnosticCaptureV1,
};
pub use app::{
    CancelRoot, ChildContext, ChildExit, ChildExitKind, DEFAULT_COOPERATIVE_DRAIN_DEADLINE,
    HARD_DRAIN_DEADLINE, QueryServer, RuntimeGuards, RuntimeServers, SearchdConfig, SearchdRuntime,
    SearchdSupervisor, SupervisionError, SupervisionOutcome, drive, supervise_runtime,
};
pub use cli::SearchdCommand;
