use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid contract: {0}")]
    InvalidContract(String),
    #[error("not ready: {0}")]
    NotReady(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("storage failure: {0}")]
    Storage(String),
}
