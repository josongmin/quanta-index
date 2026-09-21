use quanta_index_contract::{QueryErrorRepair, SearchPlaneErrorCodeV2};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("usage: {0}")]
    Usage(String),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error("serialization: {0}")]
    Serialization(String),

    #[error("transport: {0}")]
    Transport(#[source] quanta_index_ipc::IpcError),

    /// Contextual response binding refused a response whose variant was
    /// right but whose identity, order, window or commitment did not
    /// match the request that produced it (S21-07). Carries only the
    /// route, the failed axis and kind labels — never a payload field.
    #[error("binding mismatch on {route} axis {axis}: expected {expected}, got {actual}")]
    Binding {
        route: &'static str,
        axis: crate::binding::ResponseBindingAxis,
        expected: String,
        actual: String,
    },

    /// The call needs a transport this client profile does not configure
    /// (S21-07): a query-only client has no control or ingest transport,
    /// and never fabricates a dummy one.
    #[error("plane unavailable: {plane} transport is not configured for this client profile")]
    PlaneUnavailable { plane: &'static str },

    // QI-SDK-01: SDK no longer opens channel publishers directly, so the
    // `Channel(ChannelError)` variant was removed alongside the dependency
    // drop. Channel-level failures now surface through the ingest
    // dispatcher as typed `SearchPlaneIpcError` (code/message) and arrive
    // here as `Remote { code, message, repair }`. `repair` is optional typed
    // query-failure repair metadata (J7Q-06); it is None for control/ingest
    // failures and for query failures with no repairable class.
    #[error("remote {code}: {message}")]
    Remote {
        code: SearchPlaneErrorCodeV2,
        message: String,
        repair: Option<QueryErrorRepair>,
    },
}

impl SdkError {
    /// Build a [`SdkError::Protocol`] for the namespace-receipt /
    /// query-response mismatch pattern that appears once per namespace
    /// per direction. `expected` describes the wanted shape (e.g.
    /// `"semantic receipt"`); `kind` is the human-readable label of
    /// what actually arrived (typically produced by
    /// [`crate::client::QuantaIndex::ingest_response_kind`] or
    /// equivalents). Centralizing the format here keeps message wording
    /// identical across namespaces.
    #[must_use]
    pub(crate) fn unexpected_response(expected: &str, kind: &str) -> Self {
        SdkError::Protocol(format!("expected {expected}, got {kind}"))
    }
}
