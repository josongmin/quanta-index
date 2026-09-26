//! Deterministic sample evidence document.
//!
//! This fixture is the shared cross-language oracle: the Python writer in
//! `tools/benchmark/evidence.py` builds the same logical document from the
//! same constants, and both sides must produce byte-identical canonical JSON
//! and the same digest. It is deliberately not a real measurement — it is a
//! contract fixture.

use crate::envelope::{
    BenchmarkEvidenceV1, BinaryIdentity, Boundary, BuildIdentity, CommandRecord, HostIdentity,
    InputReference, LeaseIdentity, RawReference, SourceIdentity, Verdict,
};
use crate::error::ProtocolError;
use crate::payloads::{LatencyPayload, LatencyRow, MetricValue, Payload};
use crate::wire;
use crate::{PROTOCOL, PROTOCOL_VERSION};

/// Run id used by the sample fixture.
pub const SAMPLE_RUN_ID: &str = "run-20260926T120000Z-a1b2c3d4";
/// Source revision used by the sample fixture.
pub const SAMPLE_REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
/// Creation timestamp used by the sample fixture.
pub const SAMPLE_CREATED_UTC: &str = "2026-09-26T12:00:00Z";
/// Exact raw bytes the sample fixture references.
pub const SAMPLE_RAW: &[u8] = b"{\"sample\":\"warm-matrix\",\"p50_ms\":0.42}\n";

/// Build the deterministic, structurally valid sample document (unsealed).
pub fn sample_evidence() -> Result<BenchmarkEvidenceV1, ProtocolError> {
    Ok(BenchmarkEvidenceV1 {
        protocol: PROTOCOL.to_owned(),
        protocol_version: PROTOCOL_VERSION,
        run_id: SAMPLE_RUN_ID.to_owned(),
        family: "dsl-warm".to_owned(),
        profile: "dsl-authority".to_owned(),
        case_id: None,
        created_utc: SAMPLE_CREATED_UTC.to_owned(),
        source: SourceIdentity {
            revision: SAMPLE_REVISION.to_owned(),
            dirty: false,
            dirty_paths_digest: None,
            closure_profile: "benchmark-control-plane".to_owned(),
            closure_digest: wire::digest_bytes(b"closure-sample"),
        },
        build: BuildIdentity {
            toolchain: "rustc 1.92.0".to_owned(),
            target_triple: "aarch64-apple-darwin".to_owned(),
            lockfile_digest: wire::digest_bytes(b"lockfile-sample"),
            profile: "bench".to_owned(),
            flags: vec!["--locked".to_owned()],
            binaries: vec![BinaryIdentity {
                name: "dsl_warm_matrix".to_owned(),
                sha256: wire::digest_bytes(b"binary-sample"),
            }],
        },
        inputs: vec![InputReference {
            id: "workspace-fixture".to_owned(),
            availability: "present".to_owned(),
            digest: Some(wire::digest_bytes(b"corpus-sample")),
            reason: None,
        }],
        host: HostIdentity {
            policy: "local-diagnostic".to_owned(),
            os: "macos".to_owned(),
            arch: "aarch64".to_owned(),
            cpu_count: 10,
            hostname_hash: wire::digest_bytes(b"hostname-sample"),
            identity_digest: wire::digest_bytes(b"host-sample"),
            lease: LeaseIdentity {
                mode: "shared".to_owned(),
                observed_samples: 1,
            },
        },
        command: CommandRecord {
            argv: vec!["just".to_owned(), "rust-bench-dsl-warm".to_owned()],
            cwd: ".".to_owned(),
            status: "completed".to_owned(),
            exit_code: Some(0),
            timeout_seconds: 1800,
            wall_ms: 12345,
        },
        boundary: Boundary {
            clock: "monotonic".to_owned(),
            instrumentation: "none".to_owned(),
            start_event: "producer_exec".to_owned(),
            end_event: "artifact_written".to_owned(),
        },
        payload: Payload::Latency(LatencyPayload {
            rows: vec![LatencyRow {
                case_id: "lexical.keyword.native".to_owned(),
                metric: "p50".to_owned(),
                unit: "ms".to_owned(),
                samples: 200,
                p50: Some(0.42),
                p95: Some(0.55),
                p99: Some(0.61),
                error_count: 0,
                timeout_count: 0,
                early_stop_reason: None,
            }],
            errors: 0,
            timeouts: 0,
            drops: 0,
        }),
        raw: vec![RawReference {
            path: "raw/warm-matrix.json".to_owned(),
            sha256: wire::digest_bytes(SAMPLE_RAW),
            bytes: wire::byte_len(SAMPLE_RAW.len())?,
        }],
        output_digest: wire::digest_bytes(b"normalized-sample"),
        verdict: Verdict {
            scope: "diagnostic".to_owned(),
            status: "pass".to_owned(),
            reason: None,
            metrics: vec![MetricValue {
                name: "p50_ms".to_owned(),
                unit: "ms".to_owned(),
                value: 0.42,
                numerator: None,
                denominator: None,
            }],
        },
        digest: None,
    })
}

/// Build and seal the sample document.
pub fn sample_sealed() -> Result<BenchmarkEvidenceV1, ProtocolError> {
    sample_evidence()?.seal()
}

/// Raw bytes the sample fixture references.
#[must_use]
pub fn sample_raw_bytes() -> Vec<u8> {
    SAMPLE_RAW.to_vec()
}
