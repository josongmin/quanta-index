//! Real-daemon SDK session (RB-02).
//!
//! Boots a separately pinned `searchd` binary over a runner-owned fresh
//! state root, waits readiness through socket acceptance plus a product
//! query, proves the index empty, publishes via
//! `SearchCorpusNamespace::publish_and_activate`, and queries the lexical,
//! semantic and hybrid SDK routes. No direct IPC, no fixture harness.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use quanta_index_contract::ipc::GenerationStatusReport;
use quanta_index_contract::{
    GenerationPin, HybridCandidateV1, LexicalCandidate, ManifestGeneration, QueryResultWindowV2,
    RepoId, RevisionId, SearchCorpusActiveHeadV1, SearchExplanation, SearchPlaneErrorCodeV2,
    SearchPlaneSearchCorpusActivationCasAck,
};
use quanta_index_sdk::{BatchReceipt, ConnectOptions, QuantaIndex, SdkError, SearchCorpusBatch};

use crate::batch::BatchIdentity;
use crate::{BenchError, BenchResult};

pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(30);
pub const SEARCHD_BIN_ENV: &str = "QUANTA_INDEX_SEARCHD_BIN";
pub const EMBEDDER_ENV: &str = "QUANTA_INDEX_EMBEDDER";

/// Resolve the daemon binary.
///
/// An explicit path or environment pin is authoritative: a bad pin must
/// fail instead of silently selecting another binary, and an unset pin
/// refuses outright. There is deliberately no next-to-runner discovery:
/// the benchmark never measures a daemon it did not explicitly pin.
pub fn resolve_searchd_binary(explicit: Option<&Path>) -> BenchResult<PathBuf> {
    resolve_searchd_binary_with_env(explicit, &|key| std::env::var_os(key))
}

/// [`resolve_searchd_binary`] with an injectable environment lookup, so
/// tests can prove the refusal without mutating process globals.
pub fn resolve_searchd_binary_with_env(
    explicit: Option<&Path>,
    get_env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> BenchResult<PathBuf> {
    if let Some(path) = explicit {
        return require_searchd_binary(path, "--searchd-bin");
    }
    if let Some(path) = get_env(SEARCHD_BIN_ENV) {
        return require_searchd_binary(&PathBuf::from(path), SEARCHD_BIN_ENV);
    }
    Err(BenchError::Daemon(format!(
        "searchd binary is not pinned (pass --searchd-bin or set {SEARCHD_BIN_ENV}); refusing undiscovered daemon binaries",
    )))
}

/// Verify the resolved daemon binary against the pinned digest.
///
/// Returns the observed digest for the capture record. The driver pins
/// the digest; the runner re-verifies it so a directly invoked binary
/// cannot measure an unpinned daemon.
pub fn verify_searchd_digest(binary: &Path, expected_sha256: &str) -> BenchResult<String> {
    if expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(BenchError::Config(
            "--searchd-expected-sha256 must be a lowercase sha256".to_string(),
        ));
    }
    let bytes = std::fs::read(binary).map_err(|err| BenchError::Io {
        path: binary.display().to_string(),
        message: format!("cannot hash the pinned searchd binary: {err}"),
    })?;
    let observed = crate::sha256_hex(&bytes);
    if observed != expected_sha256 {
        return Err(BenchError::Daemon(
            "searchd binary digest differs from the pinned digest; refusing to boot".to_string(),
        ));
    }
    Ok(observed)
}

fn require_searchd_binary(path: &Path, source: &str) -> BenchResult<PathBuf> {
    if !path.is_absolute() || !path.is_file() {
        return Err(BenchError::Daemon(format!(
            "{source} must name an existing absolute searchd binary: {}",
            path.display()
        )));
    }
    Ok(path.to_path_buf())
}

fn daemon_socket_paths(state_root: &Path) -> [PathBuf; 3] {
    [
        state_root.join("search-plane/query.sock"),
        state_root.join("search-plane/control.sock"),
        state_root.join("search-plane/ingest.sock"),
    ]
}

#[cfg(unix)]
fn socket_accepts_connection(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::UnixStream;
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
        && UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn socket_accepts_connection(_path: &Path) -> bool {
    false
}

fn remove_socket_files(state_root: &Path) -> BenchResult<()> {
    for socket in daemon_socket_paths(state_root) {
        match std::fs::remove_file(&socket) {
            Ok(()) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                return Err(BenchError::Io {
                    path: socket.display().to_string(),
                    message: format!("failed to remove daemon socket: {err}"),
                });
            }
        }
    }
    Ok(())
}

/// The daemon requires a 0700 state root; enforce it on every accepted
/// root — created or pre-existing-but-empty — so a umask-dependent boot
/// can never fail closed spuriously, and verify the result.
#[cfg(unix)]
fn secure_state_root(state_root: &Path) -> BenchResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(state_root, std::fs::Permissions::from_mode(0o700)).map_err(
        |err| BenchError::Io {
            path: state_root.display().to_string(),
            message: format!("failed to secure state root mode 0700: {err}"),
        },
    )?;
    let mode = std::fs::metadata(state_root)
        .map_err(|err| BenchError::Io {
            path: state_root.display().to_string(),
            message: format!("cannot stat state root mode: {err}"),
        })?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o700 {
        return Err(BenchError::Daemon(format!(
            "state root mode is {mode:o}, not 0700; refusing to boot"
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn secure_state_root(_state_root: &Path) -> BenchResult<()> {
    Ok(())
}

fn daemon_log_paths(state_root: &Path) -> BenchResult<(PathBuf, PathBuf)> {
    let parent = state_root.parent().ok_or_else(|| {
        BenchError::Config(format!(
            "state root has no parent for external daemon logs: {}",
            state_root.display()
        ))
    })?;
    let name = state_root.file_name().ok_or_else(|| {
        BenchError::Config(format!(
            "state root has no file name for external daemon logs: {}",
            state_root.display()
        ))
    })?;
    let prefix = name.to_string_lossy();
    Ok((
        parent.join(format!("{prefix}.searchd.stdout.log")),
        parent.join(format!("{prefix}.searchd.stderr.log")),
    ))
}

fn daemon_stderr_tail(stderr_log: &Path) -> String {
    let text = match std::fs::read_to_string(stderr_log) {
        Ok(text) => text,
        Err(err) => format!("<stderr log unreadable: {err}>"),
    };
    let tail: String = text.chars().rev().take(600).collect();
    tail.chars().rev().collect()
}

fn terminate_child(child: &mut Child) -> BenchResult<()> {
    let status = child.try_wait().map_err(|err| {
        BenchError::Daemon(format!("failed to poll searchd during shutdown: {err}"))
    })?;
    if status.is_none() {
        child
            .kill()
            .map_err(|err| BenchError::Daemon(format!("failed to stop searchd: {err}")))?;
    }
    let _status = child
        .wait()
        .map_err(|err| BenchError::Daemon(format!("failed to reap searchd: {err}")))?;
    Ok(())
}

/// A booted real daemon plus its connected full SDK client.
pub struct DaemonSession {
    state_root: PathBuf,
    child: Option<Child>,
    client: QuantaIndex,
    searchd_binary: PathBuf,
    embedder: String,
}

pub struct DaemonConfig<'a> {
    pub state_root: &'a Path,
    pub searchd_binary: Option<&'a Path>,
    pub embedder: &'a str,
    /// Optional explicit local-model directory. Used by failure probes and
    /// qualified captures that must not inherit a workstation cache path.
    pub model_dir: Option<&'a Path>,
    pub repo_id: &'a RepoId,
    pub revision_id: &'a RevisionId,
    pub ready_timeout: Duration,
    pub io_timeout: Duration,
    pub history_max_generations: usize,
}

// Benchmark captures can index substantially larger real repositories than
// the SDK smoke fixture. This is a runner-owned history retention policy, not
// a product default or a measured cache budget.
const BENCH_HISTORY_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const BENCH_HISTORY_MAX_TOTAL_BYTES: u64 = 4 * BENCH_HISTORY_MAX_BYTES;

impl DaemonSession {
    /// Boot over a runner-owned fresh state root. A pre-existing non-empty
    /// root is refused: the runner never inherits a possibly stale index.
    /// A symlink root is refused: the daemon must serve the exact directory
    /// the runner named, never a redirected one. Boot completes only after
    /// the fresh-index product query proves no active generation, so
    /// readiness precedes publish by construction, not by caller ordering.
    pub fn boot(config: &DaemonConfig<'_>) -> BenchResult<Self> {
        if config.embedder.trim().is_empty() {
            return Err(BenchError::Config(
                "embedder profile must not be empty".to_string(),
            ));
        }
        if let Some(model_dir) = config.model_dir
            && !model_dir.is_dir()
        {
            return Err(BenchError::Daemon(format!(
                "explicit model directory does not exist or is not a directory: {}",
                model_dir.display()
            )));
        }
        if std::fs::symlink_metadata(config.state_root)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(BenchError::Daemon(format!(
                "state root must not be a symlink: {}",
                config.state_root.display()
            )));
        }
        if config.state_root.exists() {
            let non_empty = std::fs::read_dir(config.state_root)
                .map_err(|err| BenchError::Io {
                    path: config.state_root.display().to_string(),
                    message: format!("cannot inspect state root: {err}"),
                })?
                .next()
                .transpose()
                .map_err(|err| BenchError::Io {
                    path: config.state_root.display().to_string(),
                    message: format!("cannot enumerate state root: {err}"),
                })?
                .is_some();
            if non_empty {
                return Err(BenchError::Daemon(format!(
                    "state root is not fresh (refusing stale-index reuse): {}",
                    config.state_root.display()
                )));
            }
        } else {
            std::fs::create_dir_all(config.state_root).map_err(|err| BenchError::Io {
                path: config.state_root.display().to_string(),
                message: err.to_string(),
            })?;
        }
        secure_state_root(config.state_root)?;
        let binary = resolve_searchd_binary(config.searchd_binary)?;
        // Daemon logs are sibling evidence, never state-root data. Writing
        // them inside the root would contaminate format detection before the
        // daemon can establish the current layout.
        let (stdout_log_path, stderr_log_path) = daemon_log_paths(config.state_root)?;
        let stdout_log = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stdout_log_path)
            .map_err(|err| BenchError::Io {
                path: stdout_log_path.display().to_string(),
                message: format!("refusing to overwrite daemon stdout evidence: {err}"),
            })?;
        let stderr_log = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stderr_log_path)
            .map_err(|err| BenchError::Io {
                path: stderr_log_path.display().to_string(),
                message: format!("refusing to overwrite daemon stderr evidence: {err}"),
            })?;
        let mut command = Command::new(&binary);
        let _configured = command
            .stdout(std::process::Stdio::from(stdout_log))
            .stderr(std::process::Stdio::from(stderr_log))
            .arg("serve")
            .arg("--state-root")
            .arg(config.state_root)
            .env(EMBEDDER_ENV, config.embedder)
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS",
                config.history_max_generations.to_string(),
            )
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
                BENCH_HISTORY_MAX_BYTES.to_string(),
            )
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS",
                "128",
            )
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
                BENCH_HISTORY_MAX_TOTAL_BYTES.to_string(),
            );
        if let Some(model_dir) = config.model_dir {
            let _configured = command.env("QUANTA_INDEX_EMBED_MODEL_DIR", model_dir);
        }
        let mut child = command.spawn().map_err(|err| {
            BenchError::Daemon(format!("failed to spawn {}: {err}", binary.display()))
        })?;
        let sockets = daemon_socket_paths(config.state_root);
        let start = Instant::now();
        let ready = loop {
            if sockets
                .iter()
                .all(|socket| socket_accepts_connection(socket))
            {
                break true;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    let tail = daemon_stderr_tail(&stderr_log_path);
                    let _cleanup = remove_socket_files(config.state_root);
                    return Err(BenchError::Daemon(format!(
                        "searchd exited before opening sockets: {status}; stderr tail: {tail}"
                    )));
                }
                Ok(None) => {}
                Err(err) => {
                    let _termination = terminate_child(&mut child);
                    let _cleanup = remove_socket_files(config.state_root);
                    return Err(BenchError::Daemon(format!("failed to poll searchd: {err}")));
                }
            }
            if start.elapsed() >= config.ready_timeout {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if !ready {
            let _termination = terminate_child(&mut child);
            let _cleanup = remove_socket_files(config.state_root);
            return Err(BenchError::Timeout(
                config.ready_timeout,
                "searchd did not open query/control/ingest sockets".to_string(),
            ));
        }
        let client = QuantaIndex::connect(
            ConnectOptions::from_state_root(config.state_root)
                .with_request_io_timeout(config.io_timeout),
        )
        .map_err(|err| {
            let _termination = terminate_child(&mut child);
            let _cleanup = remove_socket_files(config.state_root);
            BenchError::Sdk(format!("SDK connect failed: {err}"))
        })?;
        // The fresh-index product query: boot is incomplete until the
        // daemon proves no active generation for this exact repository.
        // Transport, protocol, and readiness errors are not evidence of
        // an empty index.
        let report = client
            .generations()
            .status(config.repo_id.clone(), config.revision_id.clone())
            .map_err(|err| {
                let _termination = terminate_child(&mut child);
                let _cleanup = remove_socket_files(config.state_root);
                BenchError::Sdk(format!("fresh-index status probe failed: {err}"))
            })?;
        if let Err(err) = verify_empty_status(&report, config.repo_id, config.revision_id) {
            let _termination = terminate_child(&mut child);
            let _cleanup = remove_socket_files(config.state_root);
            return Err(err);
        }
        Ok(Self {
            state_root: config.state_root.to_path_buf(),
            child: Some(child),
            client,
            searchd_binary: binary,
            embedder: config.embedder.to_string(),
        })
    }

    #[must_use]
    pub fn client(&self) -> &QuantaIndex {
        &self.client
    }

    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    #[must_use]
    pub fn searchd_binary(&self) -> &Path {
        &self.searchd_binary
    }

    #[must_use]
    pub fn embedder(&self) -> &str {
        &self.embedder
    }

    /// Stop and reap the owned daemon while retaining the connected client.
    /// This is a benchmark failure probe: subsequent SDK calls must return a
    /// typed transport failure and can never be scored as empty success.
    pub fn terminate_for_failure_probe(&mut self) -> BenchResult<()> {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child)?;
        }
        remove_socket_files(&self.state_root)
    }

    /// Bounded shutdown of the runner-owned daemon.
    pub fn stop(mut self) -> BenchResult<()> {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child)?;
        }
        remove_socket_files(&self.state_root)
    }
}

fn verify_empty_status(
    report: &GenerationStatusReport,
    repo_id: &RepoId,
    revision_id: &RevisionId,
) -> BenchResult<()> {
    if &report.repo_id != repo_id || &report.revision_id != revision_id {
        return Err(BenchError::Protocol(
            "fresh-index status response names another repository or revision".to_string(),
        ));
    }
    if !report.tracks.is_empty() || report.semantic_content.is_some() {
        return Err(BenchError::Protocol(
            "fresh daemon unexpectedly has an active generation; refusing stale-index reuse"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod empty_status_tests {
    use super::*;

    #[test]
    fn empty_status_requires_correct_identity_and_no_active_tracks() {
        let repo = RepoId::new("bench-repo").expect("repo ID");
        let revision = RevisionId::new("bench-revision").expect("revision ID");
        let mut report = GenerationStatusReport {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            tracks: Vec::new(),
            semantic_content: None,
        };
        assert!(verify_empty_status(&report, &repo, &revision).is_ok());
        assert!(
            verify_empty_status(
                &report,
                &RepoId::new("other-repo").expect("repo ID"),
                &revision
            )
            .is_err()
        );
        report
            .tracks
            .push(quanta_index_contract::ipc::TrackReadinessRecord {
                track: quanta_index_contract::SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(1),
                manifest_digest: "existing".to_string(),
            });
        assert!(verify_empty_status(&report, &repo, &revision).is_err());
    }

    #[test]
    fn explicit_binary_pin_never_falls_back() {
        assert!(resolve_searchd_binary(Some(Path::new("/does-not-exist/searchd"))).is_err());
        assert!(require_searchd_binary(Path::new("relative/searchd"), "test").is_err());
    }

    #[test]
    fn unset_pin_refuses_without_discovery() {
        let err =
            resolve_searchd_binary_with_env(None, &|_key| None).expect_err("unset pin must refuse");
        let text = err.to_string();
        assert!(text.contains("not pinned"), "{text}");
        assert!(text.contains("refusing undiscovered"), "{text}");
    }

    #[test]
    fn error_classification_preserves_typed_outcomes() {
        use quanta_index_contract::SearchPlaneErrorCodeV2;
        use quanta_index_ipc::{IpcError, IpcIoOperation};
        use quanta_index_sdk::{ResponseBindingAxis, SdkError};

        let timeout = SdkError::Transport(IpcError::Timeout {
            operation: IpcIoOperation::Read,
            timeout: Duration::from_secs(1),
        });
        assert_eq!(classify_sdk_error(&timeout).0, "timeout");
        assert_eq!(classify_sdk_error(&timeout).1, "ipc_timeout");
        let fatal = SdkError::Transport(IpcError::Truncated);
        assert_eq!(classify_sdk_error(&fatal).0, "error");
        assert_eq!(classify_sdk_error(&fatal).1, "ipc_transport");
        let down = SdkError::PlaneUnavailable { plane: "search" };
        assert_eq!(classify_sdk_error(&down).0, "unavailable");

        let remote = |wire: &str| SdkError::Remote {
            code: SearchPlaneErrorCodeV2::from_wire_str(wire).expect("known wire code"),
            message: "wire".to_string(),
            repair: None,
        };
        assert_eq!(
            classify_sdk_error(&remote("SEM_NOT_READY")).0,
            "unavailable"
        );
        assert_eq!(
            classify_sdk_error(&remote("SEM_NOT_READY")).1,
            "SEM_NOT_READY"
        );
        assert_eq!(classify_sdk_error(&remote("QUERY_TIMEOUT")).0, "timeout");
        // Any code outside the timeout/unavailable tables is a typed error,
        // never silently downgraded to empty or success.
        let other = SearchPlaneErrorCodeV2::ALL
            .iter()
            .map(|code| code.as_wire_str())
            .find(|wire| {
                !matches!(
                    *wire,
                    "QUERY_TIMEOUT"
                        | "LEX_QUERY_TIMEOUT"
                        | "SEM_NOT_READY"
                        | "SEM_PROVIDER_UNAVAILABLE"
                        | "SEM_PROVIDER_AUTH"
                        | "SEM_PROVIDER_TRANSPORT"
                        | "HISTORY_SHARD_UNAVAILABLE"
                        | "FILE_CONTRIBUTOR_UNAVAILABLE"
                        | "FILE_OWNERSHIP_UNAVAILABLE"
                )
            })
            .expect("an error-table code exists");
        assert_eq!(classify_sdk_error(&remote(other)).0, "error");

        assert_eq!(
            classify_sdk_error(&SdkError::Usage("u".to_string())).1,
            "sdk_usage"
        );
        assert_eq!(
            classify_sdk_error(&SdkError::Protocol("p".to_string())).1,
            "sdk_protocol"
        );
        assert_eq!(
            classify_sdk_error(&SdkError::Serialization("s".to_string())).1,
            "sdk_serialization"
        );
        let binding = SdkError::Binding {
            route: "lexical",
            axis: ResponseBindingAxis::Variant,
            expected: "a".to_string(),
            actual: "b".to_string(),
        };
        assert_eq!(classify_sdk_error(&binding).1, "sdk_binding");

        for invalid in [
            QueryOutcome::SdkFailure {
                status: "success",
                code: "forged".to_string(),
                message: "forged".to_string(),
                latency: Duration::ZERO,
            },
            QueryOutcome::SdkFailure {
                status: "error",
                code: String::new(),
                message: "missing code".to_string(),
                latency: Duration::ZERO,
            },
            QueryOutcome::SdkFailure {
                status: "error",
                code: "forged".to_string(),
                message: String::new(),
                latency: Duration::ZERO,
            },
        ] {
            assert!(invalid.classification().is_err());
        }
    }

    fn forged_identity() -> BatchIdentity {
        BatchIdentity::new("bench-repo", "bench-rev", 7, "manifest:forged".to_string())
            .expect("identity")
    }

    fn forged_roots() -> quanta_index_contract::SemanticContentRootsV1 {
        quanta_index_contract::SemanticContentRootsV1 {
            row_root_digest: format!("sha256:{}", "a".repeat(64)),
            membership_root_digest: format!("sha256:{}", "b".repeat(64)),
        }
    }

    fn forged_receipt(identity: &BatchIdentity) -> BatchReceipt {
        BatchReceipt {
            generation: identity.generation,
            manifest_digest: Some(identity.manifest_digest.clone()),
            batch_digest: "batch:digest".to_string(),
            accepted_replace_scopes: 1,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 1,
            accepted_semantic_tombstone_scopes: 0,
            accepted_clear_surfaces: 0,
            sealed: true,
            applied: true,
            durable_sequence: 1,
            semantic_content: Some(forged_roots()),
        }
    }

    fn forged_active(
        identity: &BatchIdentity,
    ) -> quanta_index_contract::SearchCorpusGenerationIdentityV1 {
        use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
        let snapshot = |track| GenerationSnapshot {
            repo_id: identity.repo_id.clone(),
            revision_id: identity.revision_id.clone(),
            track,
            manifest_generation: identity.generation,
            manifest_digest: identity.manifest_digest.clone(),
        };
        quanta_index_contract::SearchCorpusGenerationIdentityV1 {
            lexical: snapshot(SearchPlaneTrackKind::Lexical),
            semantic: snapshot(SearchPlaneTrackKind::Semantic),
            semantic_content: forged_roots(),
        }
    }

    fn forged_head(identity: &BatchIdentity, sequence: u64) -> SearchCorpusActiveHeadV1 {
        SearchCorpusActiveHeadV1 {
            generation: forged_active(identity),
            activation_token: quanta_index_contract::SearchCorpusActivationTokenV1::new(
                [7; quanta_index_contract::ACTIVATION_ROOT_INCARNATION_BYTES_V1],
                std::num::NonZeroU64::new(sequence).expect("fixture sequence is positive"),
            )
            .expect("fixture incarnation is nonzero"),
        }
    }

    fn forged_ack(
        identity: &BatchIdentity,
        previous: Option<SearchCorpusActiveHeadV1>,
    ) -> SearchPlaneSearchCorpusActivationCasAck {
        let next_sequence = previous.as_ref().map_or(1, |head| {
            head.activation_token
                .activation_sequence()
                .get()
                .saturating_add(1)
        });
        SearchPlaneSearchCorpusActivationCasAck {
            active: forged_head(identity, next_sequence),
            previous_sealed_active: previous,
        }
    }

    #[test]
    fn sealed_receipt_forgeries_refuse() {
        let identity = forged_identity();
        let good = forged_receipt(&identity);
        assert!(verify_sealed_receipt(&good, "batch:digest", &identity).is_ok());
        assert!(verify_sealed_receipt(&good, "batch:other", &identity).is_err());
        for mutate in [
            |receipt: &mut BatchReceipt| receipt.sealed = false,
            |receipt: &mut BatchReceipt| receipt.applied = false,
            |receipt: &mut BatchReceipt| receipt.semantic_content = None,
            |receipt: &mut BatchReceipt| {
                receipt.manifest_digest = Some("manifest:other".to_string());
            },
            |receipt: &mut BatchReceipt| {
                receipt.generation = quanta_index_contract::ManifestGeneration::new(8);
            },
        ] {
            let mut receipt = good.clone();
            mutate(&mut receipt);
            assert!(verify_sealed_receipt(&receipt, "batch:digest", &identity).is_err());
        }
    }

    #[test]
    fn activation_ack_forgeries_refuse() {
        let identity = forged_identity();
        let good = forged_ack(&identity, None);
        assert!(verify_activation_ack(&good, &identity, None).is_ok());
        // A stale predecessor on a fresh daemon refuses.
        let stale = forged_ack(&identity, Some(forged_head(&identity, 1)));
        assert!(verify_activation_ack(&stale, &identity, None).is_err());
        // The CAS expectation must equal the predecessor exactly.
        let other_identity =
            BatchIdentity::new("bench-repo", "bench-rev", 6, "manifest:old".to_string())
                .expect("identity");
        let previous = forged_head(&other_identity, 1);
        let advanced = forged_ack(&identity, Some(previous.clone()));
        assert!(verify_activation_ack(&advanced, &identity, Some(&previous)).is_ok());
        assert!(verify_activation_ack(&advanced, &identity, None).is_err());
        // Wrong generation on one track refuses.
        let mut wrong_gen = forged_head(&identity, 1);
        wrong_gen.generation.semantic.manifest_generation =
            quanta_index_contract::ManifestGeneration::new(8);
        let ack = SearchPlaneSearchCorpusActivationCasAck {
            active: wrong_gen,
            previous_sealed_active: None,
        };
        assert!(verify_activation_ack(&ack, &identity, None).is_err());
        // An invalid identity (swapped tracks) refuses.
        let mut swapped = forged_head(&identity, 1);
        swapped.generation.semantic.track = quanta_index_contract::SearchPlaneTrackKind::Lexical;
        let ack = SearchPlaneSearchCorpusActivationCasAck {
            active: swapped,
            previous_sealed_active: None,
        };
        assert!(verify_activation_ack(&ack, &identity, None).is_err());
    }

    #[test]
    fn daemon_digest_pin_is_verified() {
        let dir = tempfile::tempdir().expect("tempdir");
        let binary = dir.path().join("searchd");
        std::fs::write(&binary, b"fake-daemon").expect("write");
        let digest = crate::sha256_hex(b"fake-daemon");
        assert_eq!(
            verify_searchd_digest(&binary, &digest).expect("pin holds"),
            digest
        );
        assert!(verify_searchd_digest(&binary, &"0".repeat(64)).is_err());
        assert!(verify_searchd_digest(&binary, "not-hex").is_err());
        assert!(verify_searchd_digest(&dir.path().join("absent"), &digest).is_err());
    }
}

impl Drop for DaemonSession {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _termination = terminate_child(&mut child);
        }
        let _cleanup = remove_socket_files(&self.state_root);
    }
}

/// Publish + activate, asserting the receipt names this exact batch and
/// the ACK promotes this exact candidate.
///
/// Both tracks of the ACK must name the expected repo/revision/generation,
/// the identity must validate, and the CAS predecessor must equal our
/// expectation (`None` on a fresh daemon, so a replay or a stale active
/// refuses). Callers testing conflicts pass an explicit expectation.
pub fn publish_and_activate(
    session: &DaemonSession,
    batch: &SearchCorpusBatch,
    expected: &BatchIdentity,
    expected_active: Option<&SearchCorpusActiveHeadV1>,
) -> BenchResult<(BatchReceipt, SearchPlaneSearchCorpusActivationCasAck)> {
    let digest = batch
        .batch_digest()
        .map_err(|err| BenchError::Sdk(format!("failed to compute batch digest: {err}")))?;
    let (receipt, ack) = session
        .client()
        .search_corpus()
        .publish_and_activate(batch, expected_active.cloned())
        .map_err(|err| BenchError::Sdk(format!("publish_and_activate failed: {err}")))?;
    verify_sealed_receipt(&receipt, &digest, expected)?;
    verify_activation_ack(&ack, expected, expected_active)?;
    if receipt.semantic_content.as_ref() != Some(&ack.active.generation.semantic_content) {
        return Err(BenchError::Protocol(
            "sealed receipt roots differ from the activated roots".to_string(),
        ));
    }
    Ok((receipt, ack))
}

pub(crate) fn verify_sealed_receipt(
    receipt: &BatchReceipt,
    digest: &str,
    expected: &BatchIdentity,
) -> BenchResult<()> {
    if receipt.batch_digest != digest {
        return Err(BenchError::Protocol(format!(
            "sealed receipt names batch {} but the runner published {digest}",
            receipt.batch_digest
        )));
    }
    if !receipt.sealed {
        return Err(BenchError::Protocol(
            "publish receipt is not sealed: failed or partial seal".to_string(),
        ));
    }
    if !receipt.applied {
        return Err(BenchError::Protocol(
            "publish receipt is a replay ack on a fresh daemon: stale index suspected".to_string(),
        ));
    }
    if receipt.generation != expected.generation {
        return Err(BenchError::Protocol(format!(
            "sealed receipt names generation {} but the runner published {}",
            receipt.generation.get(),
            expected.generation.get(),
        )));
    }
    if receipt.manifest_digest.as_deref() != Some(expected.manifest_digest.as_str()) {
        return Err(BenchError::Protocol(
            "sealed receipt manifest digest differs from the published batch".to_string(),
        ));
    }
    if receipt.semantic_content.is_none() {
        return Err(BenchError::Protocol(
            "sealed receipt attests no semantic content roots".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn verify_activation_ack(
    ack: &SearchPlaneSearchCorpusActivationCasAck,
    expected: &BatchIdentity,
    expected_active: Option<&SearchCorpusActiveHeadV1>,
) -> BenchResult<()> {
    ack.active.validate_v1().map_err(|err| {
        BenchError::Protocol(format!("activation ACK identity is invalid: {err}"))
    })?;
    for (track, snapshot) in [
        ("lexical", &ack.active.generation.lexical),
        ("semantic", &ack.active.generation.semantic),
    ] {
        if snapshot.repo_id != expected.repo_id || snapshot.revision_id != expected.revision_id {
            return Err(BenchError::Protocol(format!(
                "activation ACK promotes another repository on the {track} track"
            )));
        }
        if snapshot.manifest_generation != expected.generation {
            return Err(BenchError::Protocol(format!(
                "activation ACK promotes generation {} on the {track} track, expected {}",
                snapshot.manifest_generation.get(),
                expected.generation.get(),
            )));
        }
    }
    if ack.previous_sealed_active.as_ref() != expected_active {
        return Err(BenchError::Protocol(
            "activation ACK predecessor differs from the CAS expectation".to_string(),
        ));
    }
    let expected_sequence = expected_active.map_or(Some(1), |head| {
        head.activation_token
            .activation_sequence()
            .get()
            .checked_add(1)
    });
    if expected_sequence != Some(ack.active.activation_token.activation_sequence().get())
        || expected_active.is_some_and(|head| {
            head.activation_token.root_incarnation()
                != ack.active.activation_token.root_incarnation()
        })
    {
        return Err(BenchError::Protocol(
            "activation ACK token does not advance the expected catalog head".to_string(),
        ));
    }
    Ok(())
}

/// One ranked SDK hit normalized across routes.
#[derive(Debug, Clone)]
pub struct RankedHit {
    pub candidate_id: String,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub snippet: String,
    pub score: f64,
    /// Hybrid fusion provenance. Empty for non-hybrid routes; not part of the
    /// frozen v3 score record, but retained for the bound diagnostic artifact.
    pub contributions: Vec<RankedLaneContribution>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedLaneContribution {
    pub lane: &'static str,
    pub rank: u32,
    pub raw_score: f32,
}

/// Route explanation preserved from the actual SDK response. Window and
/// lane authority remain in the typed [`QueryResultWindowV2`]; this value
///
/// carries only explanation fields and never mirrors window counts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteExplanation {
    /// Transport request id the response answers; `0` marks an
    /// off-transport response.
    pub request_id: Option<u64>,
    pub early_stop_reason: Option<&'static str>,
    pub engines_executed: Option<Vec<&'static str>>,
    pub engines_touched: Option<Vec<&'static str>>,
    pub strategy: Option<String>,
    /// Measured inside the server route and bound to this response's
    /// request id and generation. `None` is unmeasured, not zero cost.
    pub stage_timings: Option<Vec<quanta_index_contract::QueryStageTimingV1>>,
}

fn route_explanation(explanation: &SearchExplanation) -> RouteExplanation {
    RouteExplanation {
        request_id: Some(explanation.request_id),
        early_stop_reason: explanation
            .early_stop_reason
            .as_ref()
            .map(|reason| reason.as_str()),
        engines_executed: Some(
            explanation
                .engines_executed
                .iter()
                .map(|engine| engine.as_str())
                .collect(),
        ),
        engines_touched: Some(
            explanation
                .engines_touched
                .iter()
                .map(|engine| engine.as_str())
                .collect(),
        ),
        strategy: Some(explanation.strategy.clone()),
        stage_timings: explanation.stage_timings.clone(),
    }
}

/// Typed query outcome. A returned window, a response rejected after it was
/// observed, and an SDK failure without a typed response are disjoint.
#[derive(Debug, Clone)]
pub enum QueryOutcome {
    ReturnedWindow {
        hits: Vec<RankedHit>,
        window: QueryResultWindowV2,
        explanation: Option<RouteExplanation>,
        latency: Duration,
    },
    RejectedResponse {
        code: String,
        message: String,
        observed_hit_count: usize,
        window: QueryResultWindowV2,
        explanation: Option<RouteExplanation>,
        expected_pin: GenerationPin,
        observed_pin: GenerationPin,
        latency: Duration,
    },
    SdkFailure {
        status: &'static str,
        code: String,
        message: String,
        latency: Duration,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeClassification {
    pub status: &'static str,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

impl QueryOutcome {
    /// One shared status projection consumed by both record and diagnostic
    /// emitters. It also rejects a hit/window cardinality contradiction.
    pub fn classification(&self) -> Result<OutcomeClassification, &'static str> {
        match self {
            Self::ReturnedWindow { hits, window, .. } => {
                let returned = usize::try_from(window.returned())
                    .map_err(|_conversion_error| "typed window returned count cannot fit usize")?;
                if returned != hits.len() {
                    return Err("typed window returned count differs from SDK hit count");
                }
                if hits.is_empty() && !window.outcome().is_exhausted() {
                    Ok(OutcomeClassification {
                        status: "error",
                        error_code: Some("empty_non_exhausted_window".to_string()),
                        error_message: Some(
                            "zero hits under a non-exhausted window cannot score".to_string(),
                        ),
                    })
                } else if hits.is_empty() {
                    Ok(OutcomeClassification {
                        status: "abstained",
                        error_code: None,
                        error_message: None,
                    })
                } else if window.outcome().is_exhausted() {
                    Ok(OutcomeClassification {
                        status: "success",
                        error_code: None,
                        error_message: None,
                    })
                } else {
                    Ok(OutcomeClassification {
                        status: "capped",
                        error_code: None,
                        error_message: None,
                    })
                }
            }
            Self::RejectedResponse {
                code,
                message,
                observed_hit_count,
                window,
                expected_pin,
                observed_pin,
                ..
            } => {
                if code != "stale_generation"
                    || message.is_empty()
                    || expected_pin == observed_pin
                    || usize::try_from(window.returned()) != Ok(*observed_hit_count)
                {
                    return Err("rejected response evidence is contradictory");
                }
                Ok(OutcomeClassification {
                    status: "error",
                    error_code: Some(code.clone()),
                    error_message: Some(message.clone()),
                })
            }
            Self::SdkFailure {
                status,
                code,
                message,
                ..
            } => {
                if !matches!(*status, "error" | "timeout" | "unavailable")
                    || code.is_empty()
                    || message.is_empty()
                {
                    return Err("SDK failure status, code, or message is invalid");
                }
                Ok(OutcomeClassification {
                    status,
                    error_code: Some(code.clone()),
                    error_message: Some(message.clone()),
                })
            }
        }
    }

    #[must_use]
    pub const fn latency(&self) -> Duration {
        match self {
            Self::ReturnedWindow { latency, .. }
            | Self::RejectedResponse { latency, .. }
            | Self::SdkFailure { latency, .. } => *latency,
        }
    }
}

fn lexical_hit(candidate: &LexicalCandidate) -> RankedHit {
    RankedHit {
        candidate_id: candidate.candidate_id.clone(),
        path: candidate.repo_relative_path.as_str().to_string(),
        start_line: candidate.start_line,
        end_line: candidate.end_line,
        snippet: candidate.snippet.clone(),
        score: f64::from(candidate.score),
        contributions: Vec::new(),
    }
}

fn hybrid_hit(candidate: &HybridCandidateV1) -> RankedHit {
    RankedHit {
        candidate_id: candidate.candidate.candidate_id.clone(),
        path: candidate.candidate.repo_relative_path.as_str().to_string(),
        start_line: candidate.candidate.start_line,
        end_line: candidate.candidate.end_line,
        snippet: candidate.candidate.snippet.clone(),
        score: candidate.fused_score,
        contributions: candidate
            .contributions
            .iter()
            .map(|contribution| RankedLaneContribution {
                lane: contribution.lane.as_code_str(),
                rank: contribution.rank,
                raw_score: contribution.raw_score,
            })
            .collect(),
    }
}

/// Convert one symbol candidate. The snippet is the engine's reference
/// name, not source bytes: span authority stays with the published-unit
/// registry (RBR-05).
fn symbol_hit(candidate: &quanta_index_contract::SymbolCandidate) -> RankedHit {
    RankedHit {
        candidate_id: candidate.candidate_id.clone(),
        path: candidate.repo_relative_path.as_str().to_string(),
        start_line: candidate.start_line,
        end_line: candidate.end_line,
        snippet: candidate.snippet.clone(),
        score: f64::from(candidate.score),
        contributions: Vec::new(),
    }
}

/// Classify an SDK failure into a runner-record status. Returns
/// `(status, code, message)`.
#[must_use]
pub fn classify_sdk_error(err: &SdkError) -> (&'static str, String, String) {
    match err {
        SdkError::Transport(ipc) => {
            let text = ipc.to_string();
            if is_timeout_ipc(ipc) {
                ("timeout", "ipc_timeout".to_string(), text)
            } else {
                ("error", "ipc_transport".to_string(), text)
            }
        }
        SdkError::PlaneUnavailable { plane } => (
            "unavailable",
            "plane_unavailable".to_string(),
            format!("{plane} transport is not configured for this client profile"),
        ),
        SdkError::Remote { code, message, .. } => {
            let wire = code.as_wire_str().to_string();
            (remote_status(code), wire, message.clone())
        }
        SdkError::Usage(message) => ("error", "sdk_usage".to_string(), message.clone()),
        SdkError::Protocol(message) => ("error", "sdk_protocol".to_string(), message.clone()),
        SdkError::Serialization(message) => {
            ("error", "sdk_serialization".to_string(), message.clone())
        }
        SdkError::Binding {
            route,
            axis,
            expected,
            actual,
        } => (
            "error",
            "sdk_binding".to_string(),
            format!("binding mismatch on {route} axis {axis}: expected {expected}, got {actual}"),
        ),
    }
}

fn is_timeout_ipc(ipc: &quanta_index_ipc::IpcError) -> bool {
    matches!(
        ipc,
        quanta_index_ipc::IpcError::Timeout { .. }
            | quanta_index_ipc::IpcError::ReadinessTimeout { .. }
    )
}

fn remote_status(code: &SearchPlaneErrorCodeV2) -> &'static str {
    // Provider/query timeouts are timeouts; semantic-not-ready and provider
    // absence are unavailable; everything else is a typed error. Matching on
    // the wire string keeps the mapping total across code-table growth.
    match code.as_wire_str() {
        "QUERY_TIMEOUT" | "LEX_QUERY_TIMEOUT" => "timeout",
        "SEM_NOT_READY"
        | "SEM_PROVIDER_UNAVAILABLE"
        | "SEM_PROVIDER_AUTH"
        | "SEM_PROVIDER_TRANSPORT"
        | "HISTORY_SHARD_UNAVAILABLE"
        | "FILE_CONTRIBUTOR_UNAVAILABLE"
        | "FILE_OWNERSHIP_UNAVAILABLE" => "unavailable",
        _ => "error",
    }
}

/// Query one route with monotonic timing and generation binding.
///
/// `lexical_request` and `semantic_text` come from one
/// [`crate::query_plan::QueryPlan`] built once per task under an explicit
/// query input policy (RBR-02); the raw query text is never injected into a
/// lane without the planner.
pub struct RouteQuery<'a> {
    pub client: &'a QuantaIndex,
    pub route: &'static str,
    pub lexical_request: &'a str,
    pub semantic_text: &'a str,
    pub repo_id: &'a RepoId,
    pub revision_id: &'a RevisionId,
    pub generation: ManifestGeneration,
    pub top_k: u32,
}

pub fn query_route(query: &RouteQuery<'_>) -> QueryOutcome {
    let expected_pin = GenerationPin::new(
        query.repo_id.clone(),
        query.revision_id.clone(),
        query.generation,
    );
    let start = Instant::now();
    match query.route {
        "lexical" => {
            match query
                .client
                .lexical()
                .query()
                .native(query.lexical_request)
                .active(query.repo_id.clone(), query.revision_id.clone())
                .top_k(query.top_k)
                .execute()
            {
                Ok(response) => {
                    let hits: Vec<RankedHit> = response.results.iter().map(lexical_hit).collect();
                    let explanation = route_explanation(&response.explanation);
                    observed_response(
                        query.route,
                        hits,
                        response.window,
                        Some(explanation),
                        response.generation,
                        expected_pin,
                        start,
                    )
                }
                Err(err) => failed_outcome(&err, start),
            }
        }
        "semantic" => {
            match query
                .client
                .semantic()
                .query()
                .text(query.semantic_text)
                .active(query.repo_id.clone(), query.revision_id.clone())
                .top_k(query.top_k)
                .execute()
            {
                Ok(response) => {
                    let hits: Vec<RankedHit> = response.results.iter().map(lexical_hit).collect();
                    let explanation = route_explanation(&response.explanation);
                    observed_response(
                        query.route,
                        hits,
                        response.window,
                        Some(explanation),
                        response.generation,
                        expected_pin,
                        start,
                    )
                }
                Err(err) => failed_outcome(&err, start),
            }
        }
        "hybrid" => {
            match query
                .client
                .search()
                .hybrid()
                .native(query.lexical_request)
                .semantic_text(query.semantic_text)
                .active(query.repo_id.clone(), query.revision_id.clone())
                .top_k(query.top_k)
                .execute()
            {
                Ok(response) => {
                    let hits: Vec<RankedHit> = response.results.iter().map(hybrid_hit).collect();
                    let explanation = route_explanation(&response.explanation);
                    observed_response(
                        query.route,
                        hits,
                        response.window,
                        Some(explanation),
                        response.generation,
                        expected_pin,
                        start,
                    )
                }
                Err(err) => failed_outcome(&err, start),
            }
        }
        "symbol" => {
            match query
                .client
                .symbol()
                .query()
                .native(query.lexical_request)
                .active(query.repo_id.clone(), query.revision_id.clone())
                .top_k(query.top_k)
                .execute()
            {
                Ok(response) => {
                    let hits: Vec<RankedHit> = response.results.iter().map(symbol_hit).collect();
                    observed_response(
                        query.route,
                        hits,
                        response.window,
                        None,
                        response.generation,
                        expected_pin,
                        start,
                    )
                }
                Err(err) => failed_outcome(&err, start),
            }
        }
        other => QueryOutcome::SdkFailure {
            status: "error",
            code: "unknown_route".to_string(),
            message: format!("unknown SDK route: {other}"),
            latency: start.elapsed(),
        },
    }
}

fn observed_response(
    route: &str,
    hits: Vec<RankedHit>,
    window: QueryResultWindowV2,
    explanation: Option<RouteExplanation>,
    observed_pin: GenerationPin,
    expected_pin: GenerationPin,
    start: Instant,
) -> QueryOutcome {
    if observed_pin == expected_pin {
        QueryOutcome::ReturnedWindow {
            hits,
            window,
            explanation,
            latency: start.elapsed(),
        }
    } else {
        QueryOutcome::RejectedResponse {
            code: "stale_generation".to_string(),
            message: format!(
                "{route} response generation {observed_pin:?} differs from published {expected_pin:?}"
            ),
            observed_hit_count: hits.len(),
            window,
            explanation,
            expected_pin,
            observed_pin,
            latency: start.elapsed(),
        }
    }
}

fn failed_outcome(err: &SdkError, start: Instant) -> QueryOutcome {
    let (status, code, message) = classify_sdk_error(err);
    QueryOutcome::SdkFailure {
        status,
        code,
        message,
        latency: start.elapsed(),
    }
}

/// Route-name inventory for record provenance, in canonical order.
#[must_use]
pub fn canonical_routes(routes: &[&str]) -> Vec<String> {
    let mut ordered: BTreeSet<&str> = BTreeSet::new();
    for route in routes {
        let _inserted = ordered.insert(*route);
    }
    ordered.into_iter().map(ToString::to_string).collect()
}
