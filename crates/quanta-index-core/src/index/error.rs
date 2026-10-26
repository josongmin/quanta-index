//! Shared incremental errors; independent of execution and allocation budgets.

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum IndexError {
    #[error("incremental input is invalid: {0}")]
    InvalidInput(String),
    #[error(
        "FOCUS_SUBJECT_NOT_FOUND: a requested RepoMap focus subject is absent from the selected view"
    )]
    FocusSubjectNotFound,
    #[error("incremental precondition conflict")]
    Conflict,
    #[error("operation digest conflicts with retained admission")]
    DigestConflict,
    #[error("attempt fence is no longer current")]
    FenceLost,
    #[error("owner work claim expired or was superseded")]
    WorkFenceLost,
    #[error("incremental work budget exhausted")]
    ResourceLimited,
    #[error("original execution work budget exhausted")]
    WorkLimitExceeded,
    #[error("incremental execution cancelled; admitted operation custody is retained")]
    Cancelled,
    #[error("exact snapshot is unavailable")]
    SnapshotUnavailable,
    #[error("immutable object is corrupt: {0}")]
    CorruptObject(String),
    #[error("adapter violated its binding contract: {0}")]
    BackendViolation(String),
    #[error("storage failure: {0}")]
    Storage(String),
    #[error("explicit rebuild required")]
    RebuildRequired,
    #[error("bounded maintenance required")]
    MaintenanceRequired,
}
