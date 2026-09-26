//! Typed errors for corpus loading and conformance verdicts.
//!
//! `ConformanceError` is the crate-local closed error vocabulary. Its exact
//! variants are defined below; `docs/adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md`
//! owns the product-level DSL contract, not this enum's variant inventory.

use core::fmt;

/// Closed-set corpus-conformance error codes.
///
/// Variants originate from the historical use-case error table. This local
/// type is not itself the public `LexicalErrorCode` contract.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum ConformanceError {
    ParseError,
    InvalidFilter,
    ForbiddenSyntax,
    PlanError,
    UnsupportedCombo,
    TimeoutExceeded,
    OversizedRequest,
    GenerationMismatch,
    AclDenied,
    TenantIsolation,
    Cancelled,
    BridgeRejected,
    NotImplemented,
}

impl ConformanceError {
    /// Canonical `SCREAMING_SNAKE_CASE` name per `usecase.md` §0.
    #[must_use]
    pub const fn as_code_str(&self) -> &'static str {
        match self {
            Self::ParseError => "PARSE_ERROR",
            Self::InvalidFilter => "INVALID_FILTER",
            Self::ForbiddenSyntax => "FORBIDDEN_SYNTAX",
            Self::PlanError => "PLAN_ERROR",
            Self::UnsupportedCombo => "UNSUPPORTED_COMBO",
            Self::TimeoutExceeded => "TIMEOUT_EXCEEDED",
            Self::OversizedRequest => "OVERSIZED_REQUEST",
            Self::GenerationMismatch => "GENERATION_MISMATCH",
            Self::AclDenied => "ACL_DENIED",
            Self::TenantIsolation => "TENANT_ISOLATION",
            Self::Cancelled => "CANCELLED",
            Self::BridgeRejected => "BRIDGE_REJECTED",
            Self::NotImplemented => "NOT_IMPLEMENTED",
        }
    }

    /// Parse from the `SCREAMING_SNAKE_CASE` form. Closed-set; unknown
    /// values return `None` so callers can surface a typed
    /// [`CorpusLoadError::UnknownErrorCode`].
    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "PARSE_ERROR" => Some(Self::ParseError),
            "INVALID_FILTER" => Some(Self::InvalidFilter),
            "FORBIDDEN_SYNTAX" => Some(Self::ForbiddenSyntax),
            "PLAN_ERROR" => Some(Self::PlanError),
            "UNSUPPORTED_COMBO" => Some(Self::UnsupportedCombo),
            "TIMEOUT_EXCEEDED" => Some(Self::TimeoutExceeded),
            "OVERSIZED_REQUEST" => Some(Self::OversizedRequest),
            "GENERATION_MISMATCH" => Some(Self::GenerationMismatch),
            "ACL_DENIED" => Some(Self::AclDenied),
            "TENANT_ISOLATION" => Some(Self::TenantIsolation),
            "CANCELLED" => Some(Self::Cancelled),
            "BRIDGE_REJECTED" => Some(Self::BridgeRejected),
            "NOT_IMPLEMENTED" => Some(Self::NotImplemented),
            _ => None,
        }
    }
}

impl fmt::Display for ConformanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl core::error::Error for ConformanceError {}

/// Failures emitted while loading the corpus from disk. These are
/// CI-level failures, not LQ pipeline failures: a corpus that does not
/// parse aborts the run before any row is dispatched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CorpusLoadError {
    /// Filesystem read failed (path, message).
    Io { path: String, message: String },
    /// TOML lexer/parser refused the file.
    TomlParse {
        path: String,
        line: u32,
        column: u32,
        message: String,
    },
    /// Required field missing in a row.
    MissingField { path: String, field: &'static str },
    /// Duplicate field key inside one row.
    DuplicateField { path: String, field: String },
    /// Unknown row-level field — fail-closed, never silently dropped.
    UnknownField { path: String, field: String },
    /// Field has a type the schema does not accept.
    TypeMismatch {
        path: String,
        field: &'static str,
        expected: &'static str,
        observed: &'static str,
    },
    /// Closed-set value did not match any known variant
    /// (e.g. `kind = "Pending"` instead of `pending`).
    UnknownEnumValue {
        path: String,
        field: &'static str,
        value: String,
        allowed: &'static [&'static str],
    },
    /// `expected.code = "...LITERAL..."` did not resolve to a known
    /// `ConformanceError` variant.
    UnknownErrorCode { path: String, value: String },
    /// Two rows share the same `id` field across the corpus.
    DuplicateRowId { id: String },
    /// A `gate = "pending" | "blocked"` row is missing `gating_ticket`.
    MissingGatingTicket { row_id: String },
    /// A `gate = "active"` row carries a `gating_ticket` — disallowed
    /// because the field is the marker of a non-active gate.
    UnexpectedGatingTicket { row_id: String },
    /// A runtime-row contract invariant was violated.
    InvalidRuntimeRow { row_id: String, message: String },
}

impl fmt::Display for CorpusLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "io error reading {path}: {message}"),
            Self::TomlParse {
                path,
                line,
                column,
                message,
            } => write!(f, "{path}:{line}:{column}: toml parse: {message}"),
            Self::MissingField { path, field } => {
                write!(f, "{path}: missing required field `{field}`")
            }
            Self::DuplicateField { path, field } => {
                write!(f, "{path}: duplicate field `{field}`")
            }
            Self::UnknownField { path, field } => {
                write!(f, "{path}: unknown field `{field}` (fail-closed)")
            }
            Self::TypeMismatch {
                path,
                field,
                expected,
                observed,
            } => write!(
                f,
                "{path}: field `{field}` expected {expected}, got {observed}",
            ),
            Self::UnknownEnumValue {
                path,
                field,
                value,
                allowed,
            } => {
                write!(
                    f,
                    "{path}: field `{field}` value `{value}` not in allowed set {allowed:?}",
                )
            }
            Self::UnknownErrorCode { path, value } => {
                write!(f, "{path}: unknown error code `{value}`")
            }
            Self::DuplicateRowId { id } => write!(f, "duplicate corpus row id `{id}`"),
            Self::MissingGatingTicket { row_id } => {
                write!(f, "row `{row_id}` has gate ≠ active but no `gating_ticket`",)
            }
            Self::UnexpectedGatingTicket { row_id } => {
                write!(
                    f,
                    "row `{row_id}` has gate = active but carries a `gating_ticket`",
                )
            }
            Self::InvalidRuntimeRow { row_id, message } => {
                write!(f, "row `{row_id}` violates runtime row contract: {message}")
            }
        }
    }
}

impl core::error::Error for CorpusLoadError {}
