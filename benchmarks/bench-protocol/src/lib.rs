//! Typed benchmark evidence contract (`BenchmarkEvidenceV1`).
//!
//! This crate is the normative definition of the common benchmark evidence
//! envelope and the reference validator for it. It has **no dependency on any
//! product crate or cloud service**: producers and the Python orchestrator may
//! depend on this contract, shipping binaries must not.
//!
//! The crate exists so that the wire format has exactly one meaning. The
//! Python orchestrator (`tools/benchmark/evidence.py`) writes the same
//! canonical bytes; the cross-language conformance vectors in
//! `tests/conformance_vectors.rs` pin the agreement. A disagreement is a
//! refusal, never a silent upcast.
//!
//! Key rules enforced here:
//!
//! - one payload per purpose; a retrieval payload can never become a latency
//!   row, an instruction count can never become wall latency;
//! - immutable run directories: stage, validate every referenced raw file,
//!   then atomically promote; never overwrite an existing run id;
//! - `latest` is an advisory pointer and never a baseline;
//! - missing, duplicated, stale, wrong-source, wrong-host, partial, timed-out
//!   or tampered evidence fails closed.

#![forbid(unsafe_code)]

pub mod codec;
pub mod envelope;
pub mod error;
pub mod payloads;
pub mod run_store;
pub mod sample;
pub mod wire;

pub use envelope::{
    BenchmarkEvidenceV1, BinaryIdentity, Boundary, BuildIdentity, CommandRecord, HostIdentity,
    InputReference, LeaseIdentity, RawReference, SourceIdentity, Verdict,
};
pub use error::ProtocolError;
pub use payloads::{
    AgentOutcomePayload, ExperimentPoint, FreshnessPayload, FreshnessPhase, LatencyPayload,
    LatencyRow, LoadPayload, LoadPoint, MetricValue, MicroPayload, Payload,
    RecordedExperimentPayload, RetrievalPayload, RetrievalRow,
};
pub use run_store::{BaselineRecord, LatestPointer, Promotion, RunStore, StagingRun};
pub use wire::{canonical_json, digest_bytes, digest_of, is_digest, sha256_hex};

/// Wire protocol name; independent of `BenchArtifactV1`, retrieval suite v3 and
/// runner v5 version numbers.
pub const PROTOCOL: &str = "BenchmarkEvidenceV1";

/// Wire protocol version.
pub const PROTOCOL_VERSION: u32 = 1;
