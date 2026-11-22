use quanta_index_core::{CoreError, index::IndexError};

/// Distinguishes an admission refusal from a failed integrity check.
pub(crate) enum IntegrityCheckError {
    Admission(IndexError),
    Integrity(CoreError),
}

impl From<CoreError> for IntegrityCheckError {
    fn from(error: CoreError) -> Self {
        Self::Integrity(error)
    }
}

impl From<IndexError> for IntegrityCheckError {
    fn from(error: IndexError) -> Self {
        Self::Admission(error)
    }
}
