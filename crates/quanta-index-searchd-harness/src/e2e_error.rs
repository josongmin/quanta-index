//! Typed harness and daemon failures shared by benchmark runners.

use std::fmt;

use quanta_index_contract::SearchPlaneErrorCodeV2;

/// Harness failures are distinct from refusals decoded from the daemon wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum E2eErrorCode {
    Remote(SearchPlaneErrorCodeV2),
    HarnessStart,
    HarnessRouteMismatch,
    IpcTransport,
    UnexpectedResponse,
}

impl E2eErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Remote(code) => code.as_wire_str(),
            Self::HarnessStart => "HARNESS_START",
            Self::HarnessRouteMismatch => "HARNESS_ROUTE_MISMATCH",
            Self::IpcTransport => "IPC_TRANSPORT",
            Self::UnexpectedResponse => "UNEXPECTED_RESPONSE",
        }
    }

    /// Exact decoder for the child-process benchmark artifact boundary.
    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "HARNESS_START" => Some(Self::HarnessStart),
            "HARNESS_ROUTE_MISMATCH" => Some(Self::HarnessRouteMismatch),
            "IPC_TRANSPORT" => Some(Self::IpcTransport),
            "UNEXPECTED_RESPONSE" => Some(Self::UnexpectedResponse),
            _ => SearchPlaneErrorCodeV2::from_wire_str(value).map(Self::Remote),
        }
    }
}

impl fmt::Display for E2eErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
