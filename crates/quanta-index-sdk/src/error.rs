use core::fmt;
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
        axis: ResponseBindingAxis,
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

/// Which contextual axis a response failed to bind on. The error carries
/// only the route, this axis and kind labels — never a payload field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseBindingAxis {
    /// The response variant is not the one this call declared.
    Variant,
    /// A pinned response read identity differs from the requested pin.
    ReadIdentity,
    /// An active-selector response resolved outside the requested
    /// repo/revision domain.
    SelectorDomain,
    /// A candidate row belongs to another generation than the page's.
    CandidateIdentity,
    /// The page window disagrees with the rows it describes.
    Window,
    /// The response order differs from the requested order.
    Order,
    /// A text response ranks a different unit than the request requires.
    ResultUnit,
    /// The returned row count exceeds the request cap.
    Cardinality,
    /// The response reports invalid work or exceeds the admitted allowance.
    WorkSettlement,
    /// A receipt digest or generation differs from the published batch.
    BatchCommitment,
    /// An ACK's target identity differs from the requested target.
    TargetIdentity,
    /// A CAS ACK's prior-state commitment differs from the expectation
    /// the request carried.
    CasExpectation,
    /// A mutation ACK's durable sequence is not positive.
    Sequence,
    /// Ranked rows violate the response's intrinsic ranking policy:
    /// order, per-row validity or identity uniqueness.
    RankingOrder,
    /// A paired file-owner projection does not pair one to one with
    /// the ranked candidates in order.
    ProjectionPairing,
}

impl ResponseBindingAxis {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Variant => "variant",
            Self::ReadIdentity => "read_identity",
            Self::SelectorDomain => "selector_domain",
            Self::CandidateIdentity => "candidate_identity",
            Self::Window => "window",
            Self::Order => "order",
            Self::ResultUnit => "result_unit",
            Self::Cardinality => "cardinality",
            Self::WorkSettlement => "work_settlement",
            Self::BatchCommitment => "batch_commitment",
            Self::TargetIdentity => "target_identity",
            Self::CasExpectation => "cas_expectation",
            Self::Sequence => "sequence",
            Self::RankingOrder => "ranking_order",
            Self::ProjectionPairing => "projection_pairing",
        }
    }
}

impl fmt::Display for ResponseBindingAxis {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
