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

/// Preserve both native admission/I/O causes and the canonical Legacy cause.
#[derive(Debug)]
pub enum ExistingFileReadFailureV1 {
    AdmissionOrIo(IndexError),
    Canonical(crate::CoreError),
}
impl From<IndexError> for ExistingFileReadFailureV1 {
    fn from(error: IndexError) -> Self {
        Self::AdmissionOrIo(error)
    }
}
impl From<crate::CoreError> for ExistingFileReadFailureV1 {
    fn from(error: crate::CoreError) -> Self {
        Self::Canonical(error)
    }
}
impl From<std::io::Error> for ExistingFileReadFailureV1 {
    fn from(error: std::io::Error) -> Self {
        Self::Canonical(crate::CoreError::storage_io(
            "existing read metadata",
            &error,
        ))
    }
}
impl std::fmt::Display for ExistingFileReadFailureV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AdmissionOrIo(error) => std::fmt::Display::fmt(error, formatter),
            Self::Canonical(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}
impl std::error::Error for ExistingFileReadFailureV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AdmissionOrIo(error) => Some(error),
            Self::Canonical(error) => Some(error),
        }
    }
}
