//! Immutable run storage: stage, verify, atomically promote, retain.
//!
//! A run directory is `<root>/runs/<run-id>/` and is never modified after
//! promotion. `latest` is an advisory pointer with no comparison authority.
//! An admitted baseline names an immutable run id and digest, and a run that
//! backs an admitted baseline (with its raw inputs) can never be collected.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::codec::Wire as _;
use crate::envelope::{BenchmarkEvidenceV1, validate_relative_path, validate_run_id};
use crate::error::ProtocolError;
use crate::wire;

/// Evidence file name inside a run directory.
pub const EVIDENCE_FILE: &str = "evidence.json";
/// Raw artifact directory inside a run directory.
pub const RAW_DIR: &str = "raw";

/// Advisory `latest` pointer; never a baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestPointer {
    /// Run id the pointer names.
    pub run_id: String,
    /// Sealed digest of that run's evidence.
    pub digest: String,
    /// Family of that run.
    pub family: String,
    /// Profile of that run.
    pub profile: String,
    /// Seconds since the Unix epoch when the pointer was written.
    pub updated_epoch_seconds: u64,
}
crate::impl_wire!(LatestPointer {
    run_id,
    digest,
    family,
    profile,
    updated_epoch_seconds,
});

/// An admitted, immutable baseline reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineRecord {
    /// Family the baseline belongs to.
    pub family: String,
    /// Immutable run id.
    pub run_id: String,
    /// Sealed evidence digest.
    pub digest: String,
    /// Host policy the baseline is valid for.
    pub host_policy: String,
    /// Source closure digest the baseline was captured under.
    pub closure_digest: String,
    /// Input digests the baseline is comparable against.
    pub input_digests: Vec<String>,
    /// Predeclared regression margin in parts-per-million.
    pub margin_ppm: u64,
    /// Predeclared uncertainty method.
    pub uncertainty: String,
}
crate::impl_wire!(BaselineRecord {
    family,
    run_id,
    digest,
    host_policy,
    closure_digest,
    input_digests,
    margin_ppm,
    uncertainty,
});

/// Result of a successful promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    /// Immutable run id.
    pub run_id: String,
    /// Promoted run directory.
    pub run_dir: PathBuf,
    /// Sealed evidence digest.
    pub digest: String,
}

/// A staged, not-yet-admissible run.
#[derive(Debug)]
pub struct StagingRun {
    run_id: String,
    path: PathBuf,
}

impl StagingRun {
    /// Staging directory the caller writes into.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run id being staged.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Write one raw artifact and return its reference.
    pub fn write_raw(
        &self,
        relative: &str,
        bytes: &[u8],
    ) -> Result<crate::envelope::RawReference, ProtocolError> {
        validate_relative_path(relative)?;
        let target = self.path.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| ProtocolError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&target, bytes).map_err(|source| ProtocolError::Io {
            path: target.clone(),
            source,
        })?;
        Ok(crate::envelope::RawReference {
            path: relative.to_owned(),
            sha256: wire::digest_bytes(bytes),
            bytes: crate::wire::byte_len(bytes.len())?,
        })
    }

    /// Write the sealed evidence document.
    pub fn write_evidence(&self, evidence: &BenchmarkEvidenceV1) -> Result<(), ProtocolError> {
        if evidence.run_id != self.run_id {
            return Err(ProtocolError::metadata_mismatch(
                &self.run_id,
                &evidence.run_id,
            ));
        }
        let text = evidence.to_canonical_json()?;
        write_atomic(&self.path.join(EVIDENCE_FILE), text.as_bytes())
    }

    /// Abandon the staged run, leaving no admissible artifact.
    pub fn abort(self) -> Result<(), ProtocolError> {
        remove_dir(&self.path)
    }
}

/// Immutable run store rooted at an external benchmark root.
#[derive(Debug, Clone)]
pub struct RunStore {
    root: PathBuf,
}

impl RunStore {
    /// Create a store handle; directories are created on first write.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `runs/` directory.
    #[must_use]
    pub fn runs_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    /// `.staging/` directory.
    #[must_use]
    pub fn staging_dir(&self) -> PathBuf {
        self.root.join(".staging")
    }

    /// `baselines/` directory.
    #[must_use]
    pub fn baselines_dir(&self) -> PathBuf {
        self.root.join("baselines")
    }

    /// `latest` pointer path.
    #[must_use]
    pub fn latest_path(&self) -> PathBuf {
        self.root.join("latest")
    }

    /// Promoted run directory for `run_id`.
    pub fn run_dir(&self, run_id: &str) -> Result<PathBuf, ProtocolError> {
        validate_run_id(run_id)?;
        Ok(self.runs_dir().join(run_id))
    }

    /// Begin a fresh staging directory for `run_id`.
    pub fn stage(&self, run_id: &str) -> Result<StagingRun, ProtocolError> {
        validate_run_id(run_id)?;
        if self.run_dir(run_id)?.exists() {
            return Err(ProtocolError::RunExists(run_id.to_owned()));
        }
        let path = self.staging_dir().join(run_id);
        if path.exists() {
            remove_dir(&path)?;
        }
        fs::create_dir_all(&path).map_err(|source| ProtocolError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(StagingRun {
            run_id: run_id.to_owned(),
            path,
        })
    }

    /// Verify every referenced raw file, then atomically promote the run.
    pub fn promote(&self, staged: StagingRun) -> Result<Promotion, ProtocolError> {
        let evidence = self.read_staged_evidence(&staged)?;
        self.verify_raw(&staged.path, &evidence)?;
        let target = self.run_dir(&staged.run_id)?;
        if target.exists() {
            return Err(ProtocolError::RunExists(staged.run_id));
        }
        fs::create_dir_all(self.runs_dir()).map_err(|source| ProtocolError::Io {
            path: self.runs_dir(),
            source,
        })?;
        sync_dir(&staged.path)?;
        fs::rename(&staged.path, &target).map_err(|source| ProtocolError::Io {
            path: target.clone(),
            source,
        })?;
        sync_dir(&self.runs_dir())?;
        let digest = evidence
            .digest
            .clone()
            .ok_or_else(|| ProtocolError::semantic("promoted evidence is unsealed".to_owned()))?;
        let pointer = LatestPointer {
            run_id: staged.run_id.clone(),
            digest: digest.clone(),
            family: evidence.family.clone(),
            profile: evidence.profile,
            updated_epoch_seconds: now_epoch_seconds(),
        };
        let pointer_bytes = wire::canonical_json(&pointer.encode()?);
        write_atomic(&self.latest_path(), pointer_bytes.as_bytes())?;
        Ok(Promotion {
            run_id: staged.run_id,
            run_dir: target,
            digest,
        })
    }

    /// Read and verify a promoted run.
    pub fn load(&self, run_id: &str) -> Result<BenchmarkEvidenceV1, ProtocolError> {
        let run_dir = self.run_dir(run_id)?;
        if !run_dir.is_dir() {
            return Err(ProtocolError::MissingRaw(format!("runs/{run_id}")));
        }
        let text = read_regular_file(&run_dir.join(EVIDENCE_FILE))?;
        let text = String::from_utf8(text)
            .map_err(|error| ProtocolError::semantic(format!("evidence is not UTF-8: {error}")))?;
        let evidence = BenchmarkEvidenceV1::open(&text)?;
        if evidence.run_id != run_id {
            return Err(ProtocolError::metadata_mismatch(run_id, &evidence.run_id));
        }
        self.verify_raw(&run_dir, &evidence)?;
        Ok(evidence)
    }

    /// Read the advisory pointer, if any.
    pub fn read_latest(&self) -> Result<Option<LatestPointer>, ProtocolError> {
        let path = self.latest_path();
        if !path.exists() {
            return Ok(None);
        }
        let bytes = read_regular_file(&path)?;
        let text = decode_text(&bytes, &path)?;
        Ok(Some(LatestPointer::decode(&crate::codec::parse_strict(
            &text,
        )?)?))
    }

    /// Admit an immutable promoted run as a family baseline.
    pub fn admit_baseline(
        &self,
        family: &str,
        run_id: &str,
        margin_ppm: u64,
        uncertainty: &str,
    ) -> Result<BaselineRecord, ProtocolError> {
        if uncertainty.trim().is_empty() {
            return Err(ProtocolError::semantic(
                "baseline uncertainty method must be declared".to_owned(),
            ));
        }
        let evidence = self.load(run_id)?;
        if evidence.family != family {
            return Err(ProtocolError::semantic(format!(
                "run {run_id:?} belongs to family {:?}, not {family:?}",
                evidence.family
            )));
        }
        if evidence.verdict.status != "pass" {
            return Err(ProtocolError::Refused(format!(
                "run {run_id:?} has verdict {:?}; only a passing run can be admitted",
                evidence.verdict.status
            )));
        }
        let digest = evidence
            .digest
            .clone()
            .ok_or_else(|| ProtocolError::semantic("admitted run is unsealed".to_owned()))?;
        let record = BaselineRecord {
            family: family.to_owned(),
            run_id: run_id.to_owned(),
            digest,
            host_policy: evidence.host.policy.clone(),
            closure_digest: evidence.source.closure_digest.clone(),
            input_digests: evidence
                .inputs
                .iter()
                .filter_map(|input| input.digest.clone())
                .collect(),
            margin_ppm,
            uncertainty: uncertainty.to_owned(),
        };
        let bytes = wire::canonical_json(&record.encode()?);
        write_atomic(
            &self.baselines_dir().join(format!("{family}.json")),
            bytes.as_bytes(),
        )?;
        Ok(record)
    }

    /// Read an admitted baseline record.
    pub fn read_baseline(&self, family: &str) -> Result<Option<BaselineRecord>, ProtocolError> {
        let path = self.baselines_dir().join(format!("{family}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let bytes = read_regular_file(&path)?;
        let text = decode_text(&bytes, &path)?;
        Ok(Some(BaselineRecord::decode(&crate::codec::parse_strict(
            &text,
        )?)?))
    }

    /// Remove every run that is neither explicitly kept nor baseline-referenced.
    pub fn collect(&self, keep: &[String]) -> Result<Vec<String>, ProtocolError> {
        // Profile captures are orchestration-owned custody roots. Refuse GC
        // rather than deleting runs pinned by an unknown profile record.
        if fs::symlink_metadata(self.root.join("captures")).is_ok() {
            return Err(ProtocolError::semantic(
                "profile-capture custody requires orchestrator garbage collection",
            ));
        }
        let mut retained: Vec<String> = keep.to_vec();
        let baselines = self.baselines_dir();
        if baselines.is_dir() {
            for entry in fs::read_dir(&baselines).map_err(|source| ProtocolError::Io {
                path: baselines.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| ProtocolError::Io {
                    path: baselines.clone(),
                    source,
                })?;
                let bytes = read_regular_file(&entry.path())?;
                let text = decode_text(&bytes, &entry.path())?;
                let record = BaselineRecord::decode(&crate::codec::parse_strict(&text)?)?;
                retained.push(record.run_id);
            }
        }
        let mut removed: Vec<String> = Vec::new();
        let runs = self.runs_dir();
        if !runs.is_dir() {
            return Ok(removed);
        }
        for entry in fs::read_dir(&runs).map_err(|source| ProtocolError::Io {
            path: runs.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| ProtocolError::Io {
                path: runs.clone(),
                source,
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if retained.contains(&name) {
                continue;
            }
            remove_dir(&entry.path())?;
            removed.push(name);
        }
        removed.sort();
        Ok(removed)
    }

    fn read_staged_evidence(
        &self,
        staged: &StagingRun,
    ) -> Result<BenchmarkEvidenceV1, ProtocolError> {
        let text = read_regular_file(&staged.path.join(EVIDENCE_FILE))?;
        let text = String::from_utf8(text)
            .map_err(|error| ProtocolError::semantic(format!("evidence is not UTF-8: {error}")))?;
        BenchmarkEvidenceV1::open(&text)
    }

    fn verify_raw(
        &self,
        run_dir: &Path,
        evidence: &BenchmarkEvidenceV1,
    ) -> Result<(), ProtocolError> {
        for reference in &evidence.raw {
            validate_relative_path(&reference.path)?;
            let path = run_dir.join(&reference.path);
            let metadata = fs::symlink_metadata(&path).map_err(|source| {
                if source.kind() == std::io::ErrorKind::NotFound {
                    ProtocolError::MissingRaw(reference.path.clone())
                } else {
                    ProtocolError::Io {
                        path: path.clone(),
                        source,
                    }
                }
            })?;
            if metadata.file_type().is_symlink() {
                return Err(ProtocolError::SymlinkRefused(reference.path.clone()));
            }
            if !metadata.is_file() {
                return Err(ProtocolError::MissingRaw(reference.path.clone()));
            }
            let bytes = fs::read(&path).map_err(|source| ProtocolError::Io {
                path: path.clone(),
                source,
            })?;
            let actual_len = crate::wire::byte_len(bytes.len())?;
            if actual_len != reference.bytes {
                return Err(ProtocolError::RawLengthMismatch {
                    path: reference.path.clone(),
                    declared: reference.bytes,
                    actual: actual_len,
                });
            }
            let computed = wire::digest_bytes(&bytes);
            if computed != reference.sha256 {
                return Err(ProtocolError::RawDigestMismatch {
                    path: reference.path.clone(),
                    declared: reference.sha256.clone(),
                    computed,
                });
            }
        }
        let raw_root = run_dir.join(RAW_DIR);
        if raw_root.is_dir() {
            for entry in fs::read_dir(&raw_root).map_err(|source| ProtocolError::Io {
                path: raw_root.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| ProtocolError::Io {
                    path: raw_root.clone(),
                    source,
                })?;
                let relative = Path::new(RAW_DIR)
                    .join(entry.file_name())
                    .to_string_lossy()
                    .into_owned();
                if !evidence.raw.iter().any(|item| item.path == relative) {
                    return Err(ProtocolError::ExtraRaw(relative));
                }
            }
        }
        Ok(())
    }
}

fn decode_text(bytes: &[u8], path: &Path) -> Result<String, ProtocolError> {
    String::from_utf8(bytes.to_vec()).map_err(|error| ProtocolError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
    })
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, ProtocolError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| ProtocolError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ProtocolError::SymlinkRefused(
            path.to_string_lossy().into_owned(),
        ));
    }
    if !metadata.is_file() {
        return Err(ProtocolError::MissingRaw(
            path.to_string_lossy().into_owned(),
        ));
    }
    fs::read(path).map_err(|source| ProtocolError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ProtocolError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ProtocolError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let temporary = path.with_extension("tmp-write");
    {
        let mut handle = File::create(&temporary).map_err(|source| ProtocolError::Io {
            path: temporary.clone(),
            source,
        })?;
        handle
            .write_all(bytes)
            .map_err(|source| ProtocolError::Io {
                path: temporary.clone(),
                source,
            })?;
        handle.sync_all().map_err(|source| ProtocolError::Io {
            path: temporary.clone(),
            source,
        })?;
    }
    fs::rename(&temporary, path).map_err(|source| ProtocolError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if let Some(parent) = path.parent() {
        sync_dir(parent)?;
    }
    Ok(())
}

fn sync_dir(path: &Path) -> Result<(), ProtocolError> {
    let handle = File::open(path).map_err(|source| ProtocolError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    handle.sync_all().map_err(|source| ProtocolError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn remove_dir(path: &Path) -> Result<(), ProtocolError> {
    fs::remove_dir_all(path).map_err(|source| ProtocolError::Io {
        path: path.to_path_buf(),
        source,
    })
}
