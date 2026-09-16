use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid contract: {0}")]
    InvalidContract(String),
    #[error("typed failure {code}: {message}")]
    Typed { code: String, message: String },
    #[error("not ready: {0}")]
    NotReady(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("storage failure: {0}")]
    Storage(String),
}

impl From<quanta_index_contract::TopKOutOfRangeV1> for CoreError {
    /// A refused `top_k` is the same typed failure on every query route.
    fn from(refused: quanta_index_contract::TopKOutOfRangeV1) -> Self {
        Self::Typed {
            code: refused.code().to_string(),
            message: refused.to_string(),
        }
    }
}

/// Accept a caller's `top_k` or return the shared typed refusal.
///
/// Every query route validates through this one call so the public range and
/// the error code cannot drift between routes.
pub fn validate_query_top_k(top_k: u32) -> Result<u32, CoreError> {
    Ok(quanta_index_contract::validate_public_top_k(top_k)?)
}

impl From<quanta_index_contract::InternalFetchOutOfRangeV1> for CoreError {
    /// A fetch size past the internal ceiling is a search-plane defect, not a
    /// caller error: the dispatcher derives it from an already-validated
    /// `top_k`.
    fn from(refused: quanta_index_contract::InternalFetchOutOfRangeV1) -> Self {
        Self::InvalidContract(format!("{}: {refused}", refused.code()))
    }
}

/// Accept an adapter-boundary fetch size or return the contract refusal.
pub fn validate_internal_fetch_size(fetch: u32) -> Result<u32, CoreError> {
    Ok(quanta_index_contract::validate_internal_fetch_size(fetch)?)
}
