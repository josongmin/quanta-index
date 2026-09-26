//! The common `BenchmarkEvidenceV1` envelope.
//!
//! The envelope carries identity and provenance; the payload carries the
//! measurement. A payload may add fields but may not redefine the identities
//! below, so a document can always answer: which source, which build, which
//! inputs, which host, which command, which raw bytes, and which verdict scope.

use std::path::{Component, Path};

use serde_json::Value;

use crate::codec::Wire as _;
use crate::error::ProtocolError;
use crate::payloads::{MetricValue, Payload};
use crate::wire;
use crate::{PROTOCOL, PROTOCOL_VERSION};

/// Host policy vocabulary; identical to the registry vocabulary.
pub const HOST_ANY: &str = "any";
/// Diagnostic-only local host.
pub const HOST_LOCAL: &str = "local-diagnostic";
/// Quiet canonical Linux bench host.
pub const HOST_CANONICAL: &str = "canonical-linux";

/// Verdict scope vocabulary.
pub const SCOPE_DIAGNOSTIC: &str = "diagnostic";
/// Contract/fixture proof scope.
pub const SCOPE_CONTRACT: &str = "contract";
/// Quality qualification scope.
pub const SCOPE_QUALITY: &str = "quality";
/// Performance qualification scope.
pub const SCOPE_PERFORMANCE: &str = "performance";

/// Source identity of the measured revision.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceIdentity {
    /// Full 40-character Git revision.
    pub revision: String,
    /// Whether the measured checkout was dirty.
    pub dirty: bool,
    /// Digest of the relevant dirty paths, required when `dirty` is true.
    pub dirty_paths_digest: Option<String>,
    /// Source-closure profile that was verified.
    pub closure_profile: String,
    /// Digest of the captured source closure.
    pub closure_digest: String,
}
crate::impl_wire!(SourceIdentity {
    revision,
    dirty,
    dirty_paths_digest,
    closure_profile,
    closure_digest,
});

/// One measured binary.
#[derive(Debug, Clone, PartialEq)]
pub struct BinaryIdentity {
    /// Binary name.
    pub name: String,
    /// SHA-256 of the binary bytes.
    pub sha256: String,
}
crate::impl_wire!(BinaryIdentity { name, sha256 });

/// Build identity of the measured artifact set.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildIdentity {
    /// Toolchain identity, e.g. `rustc 1.92.0`.
    pub toolchain: String,
    /// Cargo target triple.
    pub target_triple: String,
    /// Digest of `Cargo.lock`.
    pub lockfile_digest: String,
    /// Build profile, e.g. `bench`.
    pub profile: String,
    /// Build flags, in order.
    pub flags: Vec<String>,
    /// Measured binaries; may be empty only for recorded-only payloads.
    pub binaries: Vec<BinaryIdentity>,
}
crate::impl_wire!(BuildIdentity {
    toolchain,
    target_triple,
    lockfile_digest,
    profile,
    flags,
    binaries,
});

/// One declared input.
#[derive(Debug, Clone, PartialEq)]
pub struct InputReference {
    /// Input id, e.g. `corpus`.
    pub id: String,
    /// `present` or `unavailable`.
    pub availability: String,
    /// Digest, required when the input is present.
    pub digest: Option<String>,
    /// Why the input is unavailable; required when it is.
    pub reason: Option<String>,
}
crate::impl_wire!(InputReference {
    id,
    availability,
    digest,
    reason,
});

/// Host lease observation for a timing capture.
#[derive(Debug, Clone, PartialEq)]
pub struct LeaseIdentity {
    /// `exclusive`, `shared` or `none`.
    pub mode: String,
    /// Capture-time host observations; a preflight snapshot alone is not a lease.
    pub observed_samples: u64,
}
crate::impl_wire!(LeaseIdentity {
    mode,
    observed_samples,
});

/// Host identity of the measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct HostIdentity {
    /// Host policy the family declares.
    pub policy: String,
    /// `linux`, `macos` or `windows`.
    pub os: String,
    /// Architecture.
    pub arch: String,
    /// Logical CPU count.
    pub cpu_count: u64,
    /// Digest of the hashed hostname; never the raw hostname.
    pub hostname_hash: String,
    /// Digest over the identity fields above plus measurable power state.
    pub identity_digest: String,
    /// Lease observation.
    pub lease: LeaseIdentity,
}
crate::impl_wire!(HostIdentity {
    policy,
    os,
    arch,
    cpu_count,
    hostname_hash,
    identity_digest,
    lease,
});

/// Exact producer command.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandRecord {
    /// Allowlisted argv, in order.
    pub argv: Vec<String>,
    /// Repository-relative working directory.
    pub cwd: String,
    /// `completed`, `failed`, `timeout` or `interrupted`.
    pub status: String,
    /// Process exit code; absent when the process never exited.
    pub exit_code: Option<i32>,
    /// Declared timeout.
    pub timeout_seconds: u64,
    /// Wall duration of the producer run.
    pub wall_ms: u64,
}
crate::impl_wire!(CommandRecord {
    argv,
    cwd,
    status,
    exit_code,
    timeout_seconds,
    wall_ms,
});

/// Measurement boundary description.
#[derive(Debug, Clone, PartialEq)]
pub struct Boundary {
    /// Clock source, e.g. `monotonic`.
    pub clock: String,
    /// `none` or the instrumentation mode.
    pub instrumentation: String,
    /// Event that opens the measured interval.
    pub start_event: String,
    /// Event that closes the measured interval.
    pub end_event: String,
}
crate::impl_wire!(Boundary {
    clock,
    instrumentation,
    start_event,
    end_event,
});

/// One referenced raw artifact inside the run directory.
#[derive(Debug, Clone, PartialEq)]
pub struct RawReference {
    /// Run-relative path, e.g. `raw/warm-matrix.json`.
    pub path: String,
    /// SHA-256 of the raw bytes.
    pub sha256: String,
    /// Exact byte length.
    pub bytes: u64,
}
crate::impl_wire!(RawReference {
    path,
    sha256,
    bytes,
});

/// Verdict scope and status for this document.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// `diagnostic`, `contract`, `quality` or `performance`.
    pub scope: String,
    /// `pass`, `fail`, `unsupported` or `not_run`.
    pub status: String,
    /// Why the verdict is not `pass`.
    pub reason: Option<String>,
    /// Metrics behind the verdict; may be empty for `not_run`.
    pub metrics: Vec<MetricValue>,
}
crate::impl_wire!(Verdict {
    scope,
    status,
    reason,
    metrics,
});

/// The common benchmark evidence envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkEvidenceV1 {
    /// Protocol name; always [`PROTOCOL`].
    pub protocol: String,
    /// Protocol version; always [`PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// Immutable run identifier.
    pub run_id: String,
    /// Registered family id.
    pub family: String,
    /// Profile that produced the run.
    pub profile: String,
    /// Optional case id within the family.
    pub case_id: Option<String>,
    /// RFC 3339 UTC creation time.
    pub created_utc: String,
    /// Source identity.
    pub source: SourceIdentity,
    /// Build identity.
    pub build: BuildIdentity,
    /// Declared inputs.
    pub inputs: Vec<InputReference>,
    /// Host identity.
    pub host: HostIdentity,
    /// Producer command.
    pub command: CommandRecord,
    /// Measurement boundary.
    pub boundary: Boundary,
    /// Typed measurement payload.
    pub payload: Payload,
    /// Raw artifact references.
    pub raw: Vec<RawReference>,
    /// Digest of the normalized payload as written.
    pub output_digest: String,
    /// Verdict scope and status.
    pub verdict: Verdict,
    /// Sealed document digest; `None` before sealing. The canonical body omits
    /// this key entirely; the sealed wire form always carries it.
    pub digest: Option<String>,
}
crate::impl_wire!(BenchmarkEvidenceV1 {
    protocol,
    protocol_version,
    run_id,
    family,
    profile,
    case_id,
    created_utc,
    source,
    build,
    inputs,
    host,
    command,
    boundary,
    payload,
    raw,
    output_digest,
    verdict,
    digest,
});

impl BenchmarkEvidenceV1 {
    /// Canonical body value: the document without its own `digest` field.
    pub fn body_value(&self) -> Result<Value, ProtocolError> {
        let mut value = self.encode()?;
        if let Some(object) = value.as_object_mut() {
            let _sealed_digest: Option<Value> = object.remove("digest");
        }
        Ok(value)
    }

    /// Seal the document by computing its canonical digest.
    pub fn seal(mut self) -> Result<Self, ProtocolError> {
        self.validate()?;
        self.digest = Some(wire::digest_of(&self.body_value()?));
        Ok(self)
    }

    /// Recompute and compare the sealed digest.
    pub fn verify_digest(&self) -> Result<(), ProtocolError> {
        let declared = self
            .digest
            .clone()
            .ok_or_else(|| ProtocolError::semantic("evidence has no digest".to_owned()))?;
        wire::require_digest("digest", &declared)?;
        let computed = wire::digest_of(&self.body_value()?);
        if computed == declared {
            Ok(())
        } else {
            Err(ProtocolError::DigestMismatch { declared, computed })
        }
    }

    /// Full structural validation of the envelope and its typed payload.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol != PROTOCOL {
            return Err(ProtocolError::UnsupportedProtocol {
                found: self.protocol.clone(),
                expected: PROTOCOL,
            });
        }
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.protocol_version));
        }
        validate_run_id(&self.run_id)?;
        if self.family.trim().is_empty() || self.profile.trim().is_empty() {
            return Err(ProtocolError::semantic(
                "family and profile must be non-empty".to_owned(),
            ));
        }
        if self.created_utc.trim().is_empty() {
            return Err(ProtocolError::semantic(
                "created_utc must be non-empty".to_owned(),
            ));
        }

        let revision = &self.source.revision;
        if revision.len() != 40
            || !revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ProtocolError::semantic(format!(
                "source.revision {revision:?} is not a full lowercase Git SHA"
            )));
        }
        match (&self.source.dirty, &self.source.dirty_paths_digest) {
            (true, Some(digest)) => wire::require_digest("source.dirty_paths_digest", digest)?,
            (true, None) => {
                return Err(ProtocolError::semantic(
                    "a dirty source must carry source.dirty_paths_digest".to_owned(),
                ));
            }
            (false, Some(_)) => {
                return Err(ProtocolError::semantic(
                    "a clean source must not carry source.dirty_paths_digest".to_owned(),
                ));
            }
            (false, None) => {}
        }
        if self.source.closure_profile.trim().is_empty() {
            return Err(ProtocolError::semantic(
                "source.closure_profile must be non-empty".to_owned(),
            ));
        }
        wire::require_digest("source.closure_digest", &self.source.closure_digest)?;

        wire::require_digest("build.lockfile_digest", &self.build.lockfile_digest)?;
        if self.build.toolchain.trim().is_empty() || self.build.target_triple.trim().is_empty() {
            return Err(ProtocolError::semantic(
                "build.toolchain and build.target_triple must be non-empty".to_owned(),
            ));
        }
        for binary in &self.build.binaries {
            wire::require_digest("build.binaries[].sha256", &binary.sha256)?;
            if binary.name.trim().is_empty() {
                return Err(ProtocolError::semantic(
                    "build.binaries[].name must be non-empty".to_owned(),
                ));
            }
        }

        for input in &self.inputs {
            match input.availability.as_str() {
                "present" => {
                    let digest = input.digest.as_deref().unwrap_or("");
                    wire::require_digest("inputs[].digest", digest)?;
                }
                "unavailable" => {
                    if input.digest.is_some() {
                        return Err(ProtocolError::semantic(format!(
                            "input {:?} is unavailable but carries a digest",
                            input.id
                        )));
                    }
                    if input.reason.as_deref().unwrap_or("").trim().is_empty() {
                        return Err(ProtocolError::semantic(format!(
                            "input {:?} is unavailable without a reason",
                            input.id
                        )));
                    }
                }
                other => {
                    return Err(ProtocolError::semantic(format!(
                        "input {:?} has unknown availability {other:?}",
                        input.id
                    )));
                }
            }
        }

        match self.host.policy.as_str() {
            HOST_ANY | HOST_LOCAL => {}
            HOST_CANONICAL => {
                if self.host.os != "linux" {
                    return Err(ProtocolError::semantic(format!(
                        "host policy canonical-linux cannot be satisfied on os {:?}",
                        self.host.os
                    )));
                }
            }
            other => {
                return Err(ProtocolError::semantic(format!(
                    "host.policy {other:?} is not registered"
                )));
            }
        }
        if !matches!(
            self.host.lease.mode.as_str(),
            "exclusive" | "shared" | "none"
        ) {
            return Err(ProtocolError::semantic(format!(
                "host.lease.mode {:?} is not exclusive/shared/none",
                self.host.lease.mode
            )));
        }
        wire::require_digest("host.hostname_hash", &self.host.hostname_hash)?;
        wire::require_digest("host.identity_digest", &self.host.identity_digest)?;

        if self.command.argv.is_empty() {
            return Err(ProtocolError::semantic(
                "command.argv must not be empty".to_owned(),
            ));
        }
        match self.command.status.as_str() {
            "completed" => {
                if self.command.exit_code != Some(0) {
                    return Err(ProtocolError::InadmissibleCommand(format!(
                        "completed with exit_code {:?}",
                        self.command.exit_code
                    )));
                }
            }
            "failed" | "timeout" | "interrupted" => {
                if self.verdict.status != "not_run" {
                    return Err(ProtocolError::InadmissibleCommand(format!(
                        "producer {} cannot carry verdict {:?}",
                        self.command.status, self.verdict.status
                    )));
                }
            }
            other => {
                return Err(ProtocolError::InadmissibleCommand(other.to_owned()));
            }
        }
        if self.command.timeout_seconds == 0 {
            return Err(ProtocolError::semantic(
                "command.timeout_seconds must be positive".to_owned(),
            ));
        }

        if self.payload.kind() == "micro" && self.boundary.instrumentation == "none" {
            return Err(ProtocolError::semantic(
                "a micro payload must declare its instrumentation mode".to_owned(),
            ));
        }
        self.payload.validate()?;
        if let Payload::Proof(proof) = &self.payload
            && (self.verdict.scope != "contract"
                || proof.source_digest != self.source.closure_digest
                || (self.verdict.status == "pass" && proof.failed != 0))
        {
            return Err(ProtocolError::semantic(
                "proof requires contract scope, matching source, and no failed tests for pass"
                    .to_owned(),
            ));
        }

        if self.raw.is_empty() {
            return Err(ProtocolError::semantic(
                "evidence must reference at least one raw artifact".to_owned(),
            ));
        }
        let mut paths: Vec<&str> = Vec::with_capacity(self.raw.len());
        for reference in &self.raw {
            validate_relative_path(&reference.path)?;
            wire::require_digest("raw[].sha256", &reference.sha256)?;
            if paths.contains(&reference.path.as_str()) {
                return Err(ProtocolError::DuplicateRaw(reference.path.clone()));
            }
            paths.push(&reference.path);
        }

        wire::require_digest("output_digest", &self.output_digest)?;
        match self.verdict.scope.as_str() {
            SCOPE_DIAGNOSTIC | SCOPE_CONTRACT => {}
            SCOPE_QUALITY => {
                if self.host.identity_digest.is_empty() {
                    return Err(ProtocolError::semantic(
                        "quality scope requires a host identity digest".to_owned(),
                    ));
                }
            }
            SCOPE_PERFORMANCE => {
                if self.boundary.clock != "monotonic" {
                    return Err(ProtocolError::semantic(
                        "performance scope requires a monotonic clock".to_owned(),
                    ));
                }
                if self.host.lease.mode != "exclusive" || self.host.lease.observed_samples < 2 {
                    return Err(ProtocolError::semantic(
                        "performance scope requires an exclusive host lease with capture-time \
                         observations"
                            .to_owned(),
                    ));
                }
            }
            other => {
                return Err(ProtocolError::semantic(format!(
                    "verdict.scope {other:?} is not registered"
                )));
            }
        }
        match self.verdict.status.as_str() {
            "pass" | "fail" | "unsupported" | "not_run" => {}
            other => {
                return Err(ProtocolError::semantic(format!(
                    "verdict.status {other:?} is not registered"
                )));
            }
        }
        if self.verdict.status != "pass"
            && self
                .verdict
                .reason
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(ProtocolError::semantic(
                "a non-pass verdict must state a reason".to_owned(),
            ));
        }
        for metric in &self.verdict.metrics {
            metric.validate("verdict.metrics[]")?;
        }
        Ok(())
    }

    /// Refuse a comparison whose environment, inputs or metric structure differ.
    ///
    /// Source revision may differ (a baseline is captured at an earlier head),
    /// but the host, build profile, measurement boundary and input digests are
    /// part of the experiment's identity: a mismatch is a refusal, never a
    /// zero-regression pass.
    pub fn require_comparable(&self, baseline: &Self) -> Result<(), ProtocolError> {
        let mut differences: Vec<String> = Vec::new();
        if self.family != baseline.family {
            differences.push(format!("family {:?} != {:?}", self.family, baseline.family));
        }
        if self.payload.kind() != baseline.payload.kind() {
            differences.push(format!(
                "payload kind {:?} != {:?}",
                self.payload.kind(),
                baseline.payload.kind()
            ));
        }
        if self.host.policy != baseline.host.policy {
            differences.push(format!(
                "host policy {:?} != {:?}",
                self.host.policy, baseline.host.policy
            ));
        }
        if self.host.identity_digest != baseline.host.identity_digest {
            differences.push("host identity differs".to_owned());
        }
        if self.build.target_triple != baseline.build.target_triple {
            differences.push(format!(
                "target triple {:?} != {:?}",
                self.build.target_triple, baseline.build.target_triple
            ));
        }
        if self.build.profile != baseline.build.profile {
            differences.push(format!(
                "build profile {:?} != {:?}",
                self.build.profile, baseline.build.profile
            ));
        }
        if self.boundary.clock != baseline.boundary.clock {
            differences.push(format!(
                "clock {:?} != {:?}",
                self.boundary.clock, baseline.boundary.clock
            ));
        }
        if self.boundary.instrumentation != baseline.boundary.instrumentation {
            differences.push("instrumentation mode differs".to_owned());
        }
        let mut candidate_inputs: Vec<(&str, Option<&str>)> = self
            .inputs
            .iter()
            .map(|input| (input.id.as_str(), input.digest.as_deref()))
            .collect();
        let mut baseline_inputs: Vec<(&str, Option<&str>)> = baseline
            .inputs
            .iter()
            .map(|input| (input.id.as_str(), input.digest.as_deref()))
            .collect();
        candidate_inputs.sort();
        baseline_inputs.sort();
        if candidate_inputs != baseline_inputs {
            differences.push("input digests differ".to_owned());
        }
        if differences.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError::Refused(format!(
                "evidence is not comparable to the baseline: {}",
                differences.join("; ")
            )))
        }
    }

    /// Deserialize and verify a sealed document, refusing every malformed case.
    pub fn open(text: &str) -> Result<Self, ProtocolError> {
        // Strict parsing rejects duplicate keys at any depth; the manual
        // decoder rejects unknown and missing fields per struct.
        let value = crate::codec::parse_strict(text)?;
        let evidence = Self::decode(&value)?;
        evidence.verify_digest()?;
        evidence.validate()?;
        Ok(evidence)
    }

    /// Canonical sealed JSON bytes.
    pub fn to_canonical_json(&self) -> Result<String, ProtocolError> {
        self.verify_digest()?;
        Ok(wire::canonical_json(&self.encode()?))
    }
}

/// Refuse a run id that could escape a run root or collide with a pointer.
pub fn validate_run_id(run_id: &str) -> Result<(), ProtocolError> {
    if run_id.is_empty()
        || run_id.len() > 128
        || run_id == "latest"
        || run_id.starts_with('.')
        || run_id.contains('/')
        || run_id.contains('\\')
        || run_id.contains("..")
    {
        return Err(ProtocolError::InvalidRunId(run_id.to_owned()));
    }
    if !run_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ProtocolError::InvalidRunId(run_id.to_owned()));
    }
    Ok(())
}

/// Refuse a raw reference that escapes the run directory.
pub fn validate_relative_path(path: &str) -> Result<(), ProtocolError> {
    if path.is_empty() || path.contains('\\') {
        return Err(ProtocolError::PathEscape(path.to_owned()));
    }
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(ProtocolError::PathEscape(path.to_owned()));
            }
        }
    }
    Ok(())
}
