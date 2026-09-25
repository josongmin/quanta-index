//! Real-repository retrieval benchmark runner (RB-02/RB-03).
//!
//! Benchmark-only crate: loads an admitted file manifest, chunks it with a
//! benchmark-owned strategy, publishes through the public
//! `quanta-index-sdk` to a real `searchd` process, queries SDK routes, and
//! emits a versioned runner record for the single Python evaluator. It never
//! dispatches IPC directly and never links the in-process fixture harness.

#![forbid(unsafe_code)]

pub mod batch;
pub mod canonical;
pub mod chunking;
pub mod corpus;
pub mod diagnostics;
pub mod profile;
pub mod query_plan;
pub mod record;
pub mod schedule;
pub mod sdk;

use thiserror::Error;

/// Fail-closed benchmark error: every variant carries the evidence that
/// failed, never a guess about what the caller meant.
#[derive(Debug, Error)]
pub enum BenchError {
    #[error("config: {0}")]
    Config(String),
    #[error("io {path}: {message}")]
    Io { path: String, message: String },
    #[error("json {path}: {message}")]
    Json { path: String, message: String },
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("corpus {path}: {message}")]
    Corpus { path: String, message: String },
    #[error("chunk {path}: {message}")]
    Chunk { path: String, message: String },
    #[error("sdk: {0}")]
    Sdk(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("daemon: {0}")]
    Daemon(String),
    #[error("timeout after {0:?}: {1}")]
    Timeout(std::time::Duration, String),
}

pub type BenchResult<T> = Result<T, BenchError>;

/// Lowercase hex SHA-256 over bytes.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}
