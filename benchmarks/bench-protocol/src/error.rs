//! Typed refusals for the evidence contract.
//!
//! Every refusal is explicit: there is no `Option`-swallowing, no empty
//! default and no "assume valid" fallback anywhere in this crate.

use std::path::PathBuf;

/// A benchmark evidence operation refused to proceed.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// The bytes are not valid JSON for the contract.
    #[error("malformed evidence JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The document names a different protocol.
    #[error("unsupported protocol {found:?}; expected {expected:?}")]
    UnsupportedProtocol {
        /// Protocol name found in the document.
        found: String,
        /// Protocol name this crate implements.
        expected: &'static str,
    },
    /// The document names an unknown protocol version.
    #[error("unsupported protocol_version {0}")]
    UnsupportedVersion(u32),
    /// A digest field is not `sha256:<64 lowercase hex>`.
    #[error("invalid digest for {field}: {value:?}")]
    InvalidDigest {
        /// Field that carried the malformed digest.
        field: String,
        /// The malformed value.
        value: String,
    },
    /// The declared document digest does not match the canonical bytes.
    #[error("evidence digest mismatch: declared {declared}, computed {computed}")]
    DigestMismatch {
        /// Digest carried by the document.
        declared: String,
        /// Digest recomputed from the canonical body.
        computed: String,
    },
    /// A filesystem operation failed.
    #[error("io error at {path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A raw reference escapes the run root.
    #[error("raw reference {0:?} escapes the run root")]
    PathEscape(String),
    /// A raw reference is a symlink.
    #[error("raw reference {0:?} is a symlink")]
    SymlinkRefused(String),
    /// A referenced raw file does not exist.
    #[error("referenced raw file {0:?} is missing")]
    MissingRaw(String),
    /// A raw file digest does not match the declaration.
    #[error("raw file {path:?} digest mismatch: declared {declared}, computed {computed}")]
    RawDigestMismatch {
        /// Raw reference path.
        path: String,
        /// Declared digest.
        declared: String,
        /// Recomputed digest.
        computed: String,
    },
    /// A raw file byte length does not match the declaration.
    #[error("raw file {path:?} length mismatch: declared {declared}, actual {actual}")]
    RawLengthMismatch {
        /// Raw reference path.
        path: String,
        /// Declared length.
        declared: u64,
        /// Actual length.
        actual: u64,
    },
    /// A file exists under `raw/` that no reference declares.
    #[error("undeclared raw file {0:?}")]
    ExtraRaw(String),
    /// The producer did not complete successfully.
    #[error("command status {0:?} is not admissible evidence")]
    InadmissibleCommand(String),
    /// The verdict status is not admissible for the requested scope.
    #[error("evidence is {0}")]
    Refused(String),
    /// A run id already exists; run ids are immutable.
    #[error("run {0:?} already exists; run ids are immutable")]
    RunExists(String),
    /// A run id is not a valid immutable identifier.
    #[error("invalid run id {0:?}")]
    InvalidRunId(String),
    /// The run id inside a document does not match its storage location.
    #[error("run id mismatch: expected {expected:?}, found {found:?}")]
    MetadataMismatch {
        /// Expected run id.
        expected: String,
        /// Run id found in the document.
        found: String,
    },
    /// Two different raw references share one path.
    #[error("duplicate raw reference {0:?}")]
    DuplicateRaw(String),
    /// A field is absent but the payload requires it.
    #[error("{0}")]
    Semantic(String),
}

impl ProtocolError {
    /// Construct a semantic refusal with a formatted message.
    pub fn semantic(message: impl Into<String>) -> Self {
        Self::Semantic(message.into())
    }

    /// Construct a run-id mismatch refusal.
    #[must_use]
    pub fn metadata_mismatch(expected: &str, found: &str) -> Self {
        Self::MetadataMismatch {
            expected: expected.to_owned(),
            found: found.to_owned(),
        }
    }
}
