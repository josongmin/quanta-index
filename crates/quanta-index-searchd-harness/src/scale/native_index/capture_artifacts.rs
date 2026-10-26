//! External custody for the two independent Native child capture streams.

use std::{
    collections::BTreeMap,
    fmt, fs,
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use serde_json::{Value, json};

use super::{child_trace, daemon_diagnostic::VerifiedDiagnostic};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Role {
    First,
    Reopen,
}

impl Role {
    fn parse(raw: &'static str) -> Result<Self> {
        match raw {
            "first" => Ok(Self::First),
            "reopen" => Ok(Self::Reopen),
            _ => anyhow::bail!("unknown Native child capture role: {raw}"),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Reopen => "reopen",
        }
    }

    const fn file(self) -> &'static str {
        match self {
            Self::First => "first.jsonl",
            Self::Reopen => "reopen.jsonl",
        }
    }

    const fn stderr_file(self) -> &'static str {
        match self {
            Self::First => "first.stderr",
            Self::Reopen => "reopen.stderr",
        }
    }
}

#[derive(Clone, Debug)]
struct VerifiedRecord {
    capture_id: String,
    sha256: String,
    bytes: u64,
    publication: bool,
    diagnostic: VerifiedDiagnostic,
}

impl VerifiedRecord {
    fn from_capture(
        publication: bool,
        capture: &Value,
        diagnostic: &VerifiedDiagnostic,
        max_bytes: u64,
    ) -> Result<Self> {
        let capture_id = capture
            .get("capture_id")
            .and_then(Value::as_str)
            .context("verified child capture ID absent")?;
        ensure!(
            capture_id.len() == 32 && capture_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "verified child capture ID malformed"
        );
        let sha256 = capture
            .get("sha256")
            .and_then(Value::as_str)
            .context("verified child capture SHA-256 absent")?;
        ensure!(
            sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "verified child capture SHA-256 malformed"
        );
        let bytes = capture
            .get("bytes")
            .and_then(Value::as_u64)
            .context("verified child capture byte count absent")?;
        ensure!(
            bytes > 0 && bytes <= max_bytes,
            "verified child capture byte count exceeds bound"
        );
        ensure!(
            diagnostic
                .summary()
                .get("capture_id")
                .and_then(Value::as_str)
                == Some(capture_id),
            "native stderr summary and child capture identities differ"
        );
        Ok(Self {
            capture_id: capture_id.to_owned(),
            sha256: sha256.to_owned(),
            bytes,
            publication,
            diagnostic: diagnostic.clone(),
        })
    }

    fn matches(&self, observed: &Value) -> bool {
        observed.get("capture_id").and_then(Value::as_str) == Some(self.capture_id.as_str())
            && observed.get("sha256").and_then(Value::as_str) == Some(self.sha256.as_str())
            && observed.get("bytes").and_then(Value::as_u64) == Some(self.bytes)
    }
}

/// Child trace paths live outside the state `TempDir`, including on failure.
#[derive(Clone, Debug)]
pub(super) struct ChildTraceArtifacts {
    directory: PathBuf,
    max_bytes: u64,
    verified: BTreeMap<Role, VerifiedRecord>,
}

impl ChildTraceArtifacts {
    pub(super) fn create(directory: &Path, max_bytes: u64) -> Result<Self> {
        let _validated_limits = quanta_index_searchd::DiagnosticCaptureLimitsV1::default()
            .with_max_output_bytes(max_bytes)?;
        ensure!(
            directory.is_absolute(),
            "Native child capture directory must be absolute"
        );
        ensure!(
            directory.to_str().is_some(),
            "Native child capture directory must be UTF-8 for artifact paths"
        );
        let parent = directory
            .parent()
            .context("Native child capture directory has no parent")?;
        ensure!(
            parent.is_dir(),
            "Native child capture parent directory does not exist"
        );
        let mut builder = fs::DirBuilder::new();
        let _private_mode = builder.mode(0o700);
        builder
            .create(directory)
            .context("create exclusive Native child capture directory")?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .context("set private Native child capture directory permissions")?;
        Ok(Self {
            directory: directory.to_path_buf(),
            max_bytes,
            verified: BTreeMap::new(),
        })
    }

    pub(super) fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub(super) fn path(&self, role: &'static str) -> Result<PathBuf> {
        Ok(self.directory.join(Role::parse(role)?.file()))
    }

    pub(super) fn stderr_path(&self, role: &'static str) -> Result<PathBuf> {
        Ok(self.directory.join(Role::parse(role)?.stderr_file()))
    }

    /// Remember an already verified drain. A failed first operation may have
    /// a complete trace with `publication=false`; only `success_detail` requires
    /// the publication witness.
    pub(super) fn remember(
        &mut self,
        role: &'static str,
        publication: bool,
        capture: &Value,
        diagnostic: &VerifiedDiagnostic,
    ) -> Result<()> {
        let role = Role::parse(role)?;
        ensure!(
            !self.verified.contains_key(&role),
            "Native child capture role was verified twice"
        );
        let stderr_path = self.directory.join(role.stderr_file());
        ensure!(
            diagnostic.path() == stderr_path.as_path(),
            "Native stderr diagnostic belongs to a different child role"
        );
        let record =
            VerifiedRecord::from_capture(publication, capture, diagnostic, self.max_bytes)?;
        let _previous = self.verified.insert(role, record);
        Ok(())
    }

    /// Reopen and validate both raw streams before a successful artifact may
    /// reference their digest, byte count and independent capture identity.
    pub(super) fn success_detail(&self) -> Result<Value> {
        ensure!(
            self.verified.len() == 2,
            "both Native child captures are required for a successful artifact"
        );
        let first_id = self
            .verified
            .get(&Role::First)
            .context("first Native child capture verification absent")?
            .capture_id
            .as_str();
        let reopen_id = self
            .verified
            .get(&Role::Reopen)
            .context("reopen Native child capture verification absent")?
            .capture_id
            .as_str();
        ensure!(
            first_id != reopen_id,
            "reopened child reused the first process capture identity"
        );
        let mut captures = Vec::with_capacity(2);
        for (role, required_publication) in [(Role::First, true), (Role::Reopen, false)] {
            let remembered = self
                .verified
                .get(&role)
                .context("Native child capture verification absent")?;
            ensure!(
                remembered.publication == required_publication,
                "Native child capture publication witness did not match successful run"
            );
            let path = self.directory.join(role.file());
            let observed = child_trace::verify(&path, required_publication, self.max_bytes)
                .with_context(|| format!("revalidate {} Native child capture", role.name()))?;
            ensure!(
                remembered.matches(&observed),
                "Native child capture changed after its drain verification"
            );
            let stderr = remembered
                .diagnostic
                .verify(&self.directory.join(role.stderr_file()))
                .with_context(|| format!("revalidate {} Native child stderr", role.name()))?;
            captures.push(json!({
                "role": role.name(),
                "file": role.file(),
                "path": path,
                "capture_id": remembered.capture_id,
                "sha256": remembered.sha256,
                "bytes": remembered.bytes,
                "publication_witness_required": required_publication,
                "stderr_tail": stderr,
            }));
        }
        Ok(json!({
            "scope": "independent child process streams; no parent/child clock alignment",
            "directory": self.directory,
            "captures": captures,
        }))
    }

    /// Preserve discoverable raw paths after an unsuccessful run without
    /// claiming a current checksum or a completed Product publication.
    pub(super) fn failure_detail(&self) -> Value {
        let captures = [Role::First, Role::Reopen].map(|role| {
            let path = self.directory.join(role.file());
            let remembered = self.verified.get(&role);
            let regular_file_observed_now =
                fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file());
            let stderr_path = self.directory.join(role.stderr_file());
            let stderr_regular_file_observed_now =
                fs::symlink_metadata(&stderr_path).is_ok_and(|metadata| metadata.is_file());
            json!({
                "role": role.name(),
                "file": role.file(),
                "path": path,
                "regular_file_observed_now": regular_file_observed_now,
                "verified_at_drain": remembered.is_some(),
                "publication_checked_at_drain": remembered.map(|record| record.publication),
                "current_integrity": "unverified",
                "stderr_tail": {
                    "path": stderr_path,
                    "regular_file_observed_now": stderr_regular_file_observed_now,
                    "verified_at_drain": remembered.is_some(),
                    "current_integrity": "unverified",
                    "scope": "diagnostic stderr; no current bounded-tail or full-file integrity claim",
                },
            })
        });
        json!({
            "scope": "failure diagnostics only; no Product success or current capture integrity claim",
            "directory": self.directory,
            "captures": captures,
        })
    }
}

impl fmt::Display for ChildTraceArtifacts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Native child capture paths retained at {} (failure diagnostics only)",
            self.directory.display()
        )
    }
}

impl std::error::Error for ChildTraceArtifacts {}

#[cfg(test)]
mod tests {
    use super::super::daemon_diagnostic;
    use super::*;
    use std::os::unix::ffi::OsStringExt as _;

    #[test]
    fn verified_child_captures_remain_source_bound_at_artifact_time() -> Result<()> {
        let root = tempfile::tempdir()?;
        let non_utf8 = root
            .path()
            .join(std::ffi::OsString::from_vec(vec![b'n', 0xff]));
        ensure!(ChildTraceArtifacts::create(&non_utf8, child_trace::MAX_BYTES).is_err());
        ensure!(!non_utf8.exists(), "rejected capture path was created");
        let directory = root.path().join("native-child-traces");
        let mut artifacts = ChildTraceArtifacts::create(&directory, child_trace::MAX_BYTES)?;
        ensure!(ChildTraceArtifacts::create(&directory, child_trace::MAX_BYTES).is_err());
        ensure!(artifacts.path("foreign").is_err());
        ensure!(artifacts.stderr_path("foreign").is_err());
        for (role, publication) in [("first", true), ("reopen", false)] {
            let path = artifacts.path(role)?;
            child_trace::write_balanced_fixture(&path)?;
            if role == "reopen" {
                let original = fs::read_to_string(&path)?;
                let reopened = original.replace(&"11".repeat(16), &"22".repeat(16));
                ensure!(original.len() == reopened.len(), "golden ID width changed");
                fs::write(&path, reopened)?;
            }
            let capture = child_trace::verify(&path, publication, child_trace::MAX_BYTES)?;
            let stderr_path = artifacts.stderr_path(role)?;
            daemon_diagnostic::write_balanced_stderr_fixture(&stderr_path, &capture)?;
            let diagnostic = daemon_diagnostic::verified_object_io(&stderr_path, &capture)?;
            artifacts.remember(role, publication, &capture, &diagnostic)?;
        }
        let detail = artifacts.success_detail()?;
        ensure!(detail.pointer("/captures/0/role") == Some(&json!("first")));
        ensure!(detail.pointer("/captures/1/role") == Some(&json!("reopen")));
        ensure!(
            detail
                .pointer("/captures/0/sha256")
                .and_then(Value::as_str)
                .is_some(),
            "first raw child capture digest absent"
        );
        ensure!(
            detail.pointer("/captures/0/stderr_tail/path")
                == Some(&json!(artifacts.stderr_path("first")?)),
            "first original stderr path was not retained"
        );
        ensure!(
            detail.pointer("/captures/1/stderr_tail/path")
                == Some(&json!(artifacts.stderr_path("reopen")?)),
            "reopened original stderr path was not retained"
        );
        ensure!(
            artifacts.stderr_path("first")?.is_file()
                && artifacts.stderr_path("reopen")?.is_file()
                && artifacts.stderr_path("first")? != artifacts.stderr_path("reopen")?,
            "the two daemon stderr epochs were conflated"
        );

        let reopen = artifacts.path("reopen")?;
        let original = fs::read_to_string(&reopen)?;
        ensure!(original.contains("\"value\":27"), "fixed IO golden absent");
        let changed = original.replacen("\"value\":27", "\"value\":28", 1);
        ensure!(original.len() == changed.len(), "golden byte width changed");
        fs::write(&reopen, changed)?;
        let valid_rewritten_capture = child_trace::verify(&reopen, false, child_trace::MAX_BYTES)?;
        ensure!(
            valid_rewritten_capture.get("bytes") == detail.pointer("/captures/1/bytes"),
            "same-width rewritten capture changed byte count"
        );
        ensure!(
            artifacts.success_detail().is_err(),
            "valid same-width rewritten capture retained a successful digest binding"
        );
        child_trace::write_balanced_fixture(&reopen)?;
        let original = fs::read_to_string(&reopen)?;
        let reopened = original.replace(&"11".repeat(16), &"22".repeat(16));
        fs::write(&reopen, reopened)?;
        fs::remove_file(reopen)?;
        ensure!(
            artifacts.success_detail().is_err(),
            "missing raw child capture retained a successful binding"
        );
        Ok(())
    }

    #[test]
    fn failure_detail_keeps_partial_paths_without_promoting_publication() -> Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("native-child-traces");
        let mut artifacts = ChildTraceArtifacts::create(&directory, child_trace::MAX_BYTES)?;
        let first = artifacts.path("first")?;
        child_trace::write_balanced_fixture(&first)?;
        let capture = child_trace::verify(&first, false, child_trace::MAX_BYTES)?;
        let first_stderr = artifacts.stderr_path("first")?;
        daemon_diagnostic::write_balanced_stderr_fixture(&first_stderr, &capture)?;
        let diagnostic = daemon_diagnostic::verified_object_io(&first_stderr, &capture)?;
        artifacts.remember("first", false, &capture, &diagnostic)?;
        ensure!(
            artifacts.success_detail().is_err(),
            "failed first publication acquired a successful artifact"
        );
        let failure = artifacts.failure_detail();
        ensure!(failure.pointer("/captures/0/verified_at_drain") == Some(&json!(true)));
        ensure!(failure.pointer("/captures/0/publication_checked_at_drain") == Some(&json!(false)));
        ensure!(failure.pointer("/captures/1/verified_at_drain") == Some(&json!(false)));
        ensure!(failure.pointer("/captures/1/current_integrity") == Some(&json!("unverified")));
        ensure!(failure.pointer("/captures/0/stderr_tail/path") == Some(&json!(first_stderr)));
        ensure!(
            failure.pointer("/captures/0/stderr_tail/current_integrity")
                == Some(&json!("unverified"))
        );
        ensure!(
            failure.pointer("/captures/1/stderr_tail/verified_at_drain") == Some(&json!(false))
        );
        let reopen = artifacts.path("reopen")?;
        child_trace::write_balanced_fixture(&reopen)?;
        let original = fs::read_to_string(&reopen)?;
        let reopened = original.replace(&"11".repeat(16), &"22".repeat(16));
        fs::write(&reopen, reopened)?;
        let reopened_capture = child_trace::verify(&reopen, false, child_trace::MAX_BYTES)?;
        let reopen_stderr = artifacts.stderr_path("reopen")?;
        daemon_diagnostic::write_balanced_stderr_fixture(&reopen_stderr, &reopened_capture)?;
        let reopened_diagnostic =
            daemon_diagnostic::verified_object_io(&reopen_stderr, &reopened_capture)?;
        artifacts.remember("reopen", false, &reopened_capture, &reopened_diagnostic)?;
        ensure!(
            artifacts.success_detail().is_err(),
            "two verified streams promoted a failed first publication"
        );
        drop(artifacts);
        ensure!(
            first.is_file(),
            "external failure capture was removed on drop"
        );
        Ok(())
    }
}
