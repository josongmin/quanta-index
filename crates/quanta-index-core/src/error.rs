use thiserror::Error;

use quanta_index_contract::{SearchPlaneErrorCodeV2, lex::LexicalErrorCode};

/// The one error every port speaks.
///
/// `Clone` is deliberate: a failure observed once can be owed to several
/// callers (a single-flight open shared by coalesced waiters), and each
/// must receive the typed outcome itself, not a rendering of it.
#[derive(Clone, Debug, Error)]
pub enum CoreError {
    #[error("invalid contract: {0}")]
    InvalidContract(String),
    #[error("typed failure {code}: {message}")]
    Typed {
        code: SearchPlaneErrorCodeV2,
        message: String,
    },
    #[error("not ready: {0}")]
    NotReady(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("storage failure: {0}")]
    Storage(String),
}

impl CoreError {
    /// Consume this domain error into the one closed search-plane wire taxonomy.
    ///
    /// This is the sole generic mapping owner. Query, ingest and control
    /// dispatchers may attach route-specific metadata, but must not remap codes.
    #[must_use]
    pub fn into_search_plane_wire(self) -> (SearchPlaneErrorCodeV2, String) {
        match self {
            Self::InvalidContract(message) => (SearchPlaneErrorCodeV2::InvalidRequest, message),
            Self::Typed { code, message } => (code, message),
            Self::NotReady(message) => (SearchPlaneErrorCodeV2::NotReady, message),
            Self::NotImplemented(message) => (SearchPlaneErrorCodeV2::NotImplemented, message),
            Self::NotFound(message) => (SearchPlaneErrorCodeV2::NotFound, message),
            Self::Storage(message) => (SearchPlaneErrorCodeV2::Internal, message),
        }
    }

    /// The storage's own failure message, or the error itself when it is
    /// anything else.
    ///
    /// Only the storage failing is worth waiting out: every other variant
    /// is a refusal about the request or about the state it found, and a
    /// retry would meet it again. Callers that may leave a step for a later
    /// pass (QI-BB-020, QI-BB-003) classify through this one match, which
    /// names every variant so a new one must be classified here.
    pub fn into_storage_failure(self) -> Result<String, Self> {
        match self {
            Self::Storage(message) => Ok(message),
            refusal @ (Self::InvalidContract(_)
            | Self::Typed { .. }
            | Self::NotReady(_)
            | Self::NotImplemented(_)
            | Self::NotFound(_)) => Err(refusal),
        }
    }
}

impl From<quanta_index_contract::TopKOutOfRangeV1> for CoreError {
    /// A refused `top_k` is the same typed failure on every query route.
    fn from(refused: quanta_index_contract::TopKOutOfRangeV1) -> Self {
        Self::Typed {
            code: SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode::QueryTopKOutOfRange),
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
