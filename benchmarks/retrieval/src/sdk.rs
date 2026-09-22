//! Real-daemon SDK session (RB-02).
//!
//! Boots a separately pinned `searchd` binary over a runner-owned fresh
//! state root, waits readiness through socket acceptance plus a product
//! query, proves the index empty, publishes via
//! `SearchCorpusNamespace::publish_and_activate`, and queries the lexical,
//! semantic and hybrid SDK routes. No direct IPC, no fixture harness.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ExecutionOutcomeV2, GenerationPin, HybridCandidateV1, LexicalCandidate, ManifestGeneration,
    RepoId, RevisionId, SearchPlaneErrorCodeV2, SearchPlaneSearchCorpusActivationCasAck,
};
use quanta_index_sdk::{BatchReceipt, ConnectOptions, QuantaIndex, SdkError, SearchCorpusBatch};

use crate::{BenchError, BenchResult};

pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(30);
pub const SEARCHD_BIN_ENV: &str = "QUANTA_INDEX_SEARCHD_BIN";
pub const EMBEDDER_ENV: &str = "QUANTA_INDEX_EMBEDDER";

/// Resolve the daemon binary: explicit path, then `QUANTA_INDEX_SEARCHD_BIN`,
/// then next to the current executable (both same-dir and `deps/` layouts).
/// Every miss is reported; nothing is guessed.
pub fn resolve_searchd_binary(explicit: Option<&Path>) -> BenchResult<PathBuf> {
    let mut tried: Vec<String> = Vec::new();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = explicit {
        candidates.push(path.to_path_buf());
    }
    if let Some(env) = std::env::var_os(SEARCHD_BIN_ENV) {
        candidates.push(PathBuf::from(env));
    }
    // Compile-time workspace layout (this crate lives two levels below root).
    for profile in ["debug", "release"] {
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join(profile)
                .join("quanta-index-searchd"),
        );
    }
    // Runtime target-dir override (custom lanes set CARGO_TARGET_DIR).
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        let target = PathBuf::from(target);
        for profile in ["debug", "release"] {
            candidates.push(target.join(profile).join("quanta-index-searchd"));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("quanta-index-searchd"));
            if dir.ends_with("deps") {
                if let Some(parent) = dir.parent() {
                    candidates.push(parent.join("quanta-index-searchd"));
                }
            }
        }
    }
    for candidate in candidates {
        tried.push(candidate.display().to_string());
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(BenchError::Daemon(format!(
        "searchd binary not found; tried: {} (pass --searchd-bin or set {SEARCHD_BIN_ENV}, or build the searchd package first)",
        tried.join(", ")
    )))
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

fn remove_socket_files(state_root: &Path) {
    for socket in daemon_socket_paths(state_root) {
        match std::fs::remove_file(&socket) {
            Ok(()) | Err(_) => {}
        }
    }
}

/// The daemon requires a 0700 state root; enforce it on roots the runner
/// creates so a umask-dependent boot can never fail closed spuriously.
#[cfg(unix)]
fn secure_state_root(state_root: &Path) -> BenchResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(state_root, std::fs::Permissions::from_mode(0o700)).map_err(|err| {
        BenchError::Io {
            path: state_root.display().to_string(),
            message: format!("failed to secure state root mode 0700: {err}"),
        }
    })
}

#[cfg(not(unix))]
fn secure_state_root(_state_root: &Path) -> BenchResult<()> {
    Ok(())
}

fn daemon_stderr_tail(state_root: &Path) -> String {
    let text = std::fs::read_to_string(state_root.join("searchd.stderr.log"))
        .unwrap_or_else(|_| "<stderr log unreadable>".to_string());
    let tail: String = text.chars().rev().take(600).collect();
    tail.chars().rev().collect()
}

fn terminate_child(child: &mut Child) {
    if child.try_wait().unwrap_or(None).is_none() {
        let _ignored = child.kill();
    }
    let _status = child.wait();
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
    pub ready_timeout: Duration,
    pub io_timeout: Duration,
    pub history_max_generations: usize,
}

impl DaemonSession {
    /// Boot over a runner-owned fresh state root. A pre-existing non-empty
    /// root is refused: the runner never inherits a possibly stale index.
    pub fn boot(config: &DaemonConfig<'_>) -> BenchResult<Self> {
        if config.embedder.trim().is_empty() {
            return Err(BenchError::Config(
                "embedder profile must not be empty".to_string(),
            ));
        }
        if config.state_root.exists() {
            let non_empty = std::fs::read_dir(config.state_root)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(true);
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
            secure_state_root(config.state_root)?;
        }
        let binary = resolve_searchd_binary(config.searchd_binary)?;
        // Daemon output lands in runner-owned log files inside the fresh
        // state root: no pipe deadlock on long runs, and failures carry
        // the daemon's own tail.
        let stdout_log = std::fs::File::create(config.state_root.join("searchd.stdout.log"))
            .map_err(|err| BenchError::Io {
                path: config.state_root.display().to_string(),
                message: format!("failed to create daemon stdout log: {err}"),
            })?;
        let stderr_log = std::fs::File::create(config.state_root.join("searchd.stderr.log"))
            .map_err(|err| BenchError::Io {
                path: config.state_root.display().to_string(),
                message: format!("failed to create daemon stderr log: {err}"),
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
                (16 * 1024 * 1024).to_string(),
            )
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS",
                "128",
            )
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
                (256 * 1024 * 1024).to_string(),
            );
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
                    let tail = daemon_stderr_tail(config.state_root);
                    remove_socket_files(config.state_root);
                    return Err(BenchError::Daemon(format!(
                        "searchd exited before opening sockets: {status}; stderr tail: {tail}"
                    )));
                }
                Ok(None) => {}
                Err(err) => {
                    terminate_child(&mut child);
                    remove_socket_files(config.state_root);
                    return Err(BenchError::Daemon(format!("failed to poll searchd: {err}")));
                }
            }
            if start.elapsed() >= config.ready_timeout {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if !ready {
            terminate_child(&mut child);
            remove_socket_files(config.state_root);
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
            terminate_child(&mut child);
            remove_socket_files(config.state_root);
            BenchError::Sdk(format!("SDK connect failed: {err}"))
        })?;
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

    /// Prove the fresh daemon serves no readable generation: an active query
    /// must fail. Success here means stale-index reuse and is refused.
    pub fn assert_index_empty(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> BenchResult<()> {
        match self
            .client
            .lexical()
            .query()
            .native("retrieval-bench-empty-probe")
            .active(repo_id.clone(), revision_id.clone())
            .top_k(1)
            .execute()
        {
            Ok(_) => Err(BenchError::Protocol(
                "fresh daemon unexpectedly serves an active generation; refusing stale-index reuse"
                    .to_string(),
            )),
            Err(_) => Ok(()),
        }
    }

    /// Bounded shutdown of the runner-owned daemon.
    pub fn stop(mut self) -> BenchResult<()> {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child);
        }
        remove_socket_files(&self.state_root);
        Ok(())
    }
}

impl Drop for DaemonSession {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child);
        }
        remove_socket_files(&self.state_root);
    }
}

/// Publish + activate, asserting the receipt names this exact batch and the
/// ACK promotes this exact candidate. `expected_active` is `None` on a fresh
/// daemon; callers testing conflicts pass an explicit expectation.
pub fn publish_and_activate(
    session: &DaemonSession,
    batch: &SearchCorpusBatch,
    expected_active: Option<quanta_index_contract::SearchCorpusGenerationIdentityV1>,
) -> BenchResult<(BatchReceipt, SearchPlaneSearchCorpusActivationCasAck)> {
    let digest = batch
        .batch_digest()
        .map_err(|err| BenchError::Sdk(format!("failed to compute batch digest: {err}")))?;
    let (receipt, ack) = session
        .client()
        .search_corpus()
        .publish_and_activate(batch, expected_active)
        .map_err(|err| BenchError::Sdk(format!("publish_and_activate failed: {err}")))?;
    if receipt.batch_digest != digest {
        return Err(BenchError::Protocol(format!(
            "sealed receipt names batch {} but the runner published {digest}",
            receipt.batch_digest
        )));
    }
    if receipt.semantic_content.is_none() {
        return Err(BenchError::Protocol(
            "sealed receipt attests no semantic content roots".to_string(),
        ));
    }
    Ok((receipt, ack))
}

/// One ranked SDK hit normalized across routes.
#[derive(Debug, Clone)]
pub struct RankedHit {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f64,
}

/// Typed query outcome: either ranked hits under the expected generation or
/// a classified failure. Timeouts and unavailable/degraded states are never
/// converted to empty success.
#[derive(Debug, Clone)]
pub enum QueryOutcome {
    Hits {
        hits: Vec<RankedHit>,
        outcome: ExecutionOutcomeV2,
        latency: Duration,
    },
    Failed {
        status: &'static str,
        code: String,
        message: String,
        latency: Duration,
    },
}

fn lexical_hit(candidate: &LexicalCandidate) -> RankedHit {
    RankedHit {
        path: candidate.repo_relative_path.as_str().to_string(),
        start_line: candidate.start_line,
        end_line: candidate.end_line,
        score: f64::from(candidate.score),
    }
}

fn hybrid_hit(candidate: &HybridCandidateV1) -> RankedHit {
    RankedHit {
        path: candidate.candidate.repo_relative_path.as_str().to_string(),
        start_line: candidate.candidate.start_line,
        end_line: candidate.candidate.end_line,
        score: candidate.fused_score,
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
pub struct RouteQuery<'a> {
    pub client: &'a QuantaIndex,
    pub route: &'static str,
    pub query_text: &'a str,
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
                .native(query.query_text)
                .active(query.repo_id.clone(), query.revision_id.clone())
                .top_k(query.top_k)
                .execute()
            {
                Ok(response) => {
                    let hits: Vec<RankedHit> = response.results.iter().map(lexical_hit).collect();
                    let outcome = response.window.outcome();
                    match check_pin(query.route, &response.generation, &expected_pin, start) {
                        Ok(guard) => guard.with_hits(hits, outcome),
                        Err(failed) => failed,
                    }
                }
                Err(err) => failed_outcome(&err, start),
            }
        }
        "semantic" => {
            match query
                .client
                .semantic()
                .query()
                .text(query.query_text)
                .active(query.repo_id.clone(), query.revision_id.clone())
                .top_k(query.top_k)
                .execute()
            {
                Ok(response) => {
                    let hits: Vec<RankedHit> = response.results.iter().map(lexical_hit).collect();
                    let outcome = response.window.outcome();
                    match check_pin(query.route, &response.generation, &expected_pin, start) {
                        Ok(guard) => guard.with_hits(hits, outcome),
                        Err(failed) => failed,
                    }
                }
                Err(err) => failed_outcome(&err, start),
            }
        }
        "hybrid" => {
            match query
                .client
                .search()
                .hybrid()
                .native(query.query_text)
                .semantic_text(query.query_text)
                .active(query.repo_id.clone(), query.revision_id.clone())
                .top_k(query.top_k)
                .execute()
            {
                Ok(response) => {
                    let hits: Vec<RankedHit> = response.results.iter().map(hybrid_hit).collect();
                    let outcome = response.window.outcome();
                    match check_pin(query.route, &response.generation, &expected_pin, start) {
                        Ok(guard) => guard.with_hits(hits, outcome),
                        Err(failed) => failed,
                    }
                }
                Err(err) => failed_outcome(&err, start),
            }
        }
        other => QueryOutcome::Failed {
            status: "error",
            code: "unknown_route".to_string(),
            message: format!("unknown SDK route: {other}"),
            latency: start.elapsed(),
        },
    }
}

struct PinGuard {
    latency: Duration,
}

impl PinGuard {
    fn with_hits(self, hits: Vec<RankedHit>, outcome: ExecutionOutcomeV2) -> QueryOutcome {
        QueryOutcome::Hits {
            hits,
            outcome,
            latency: self.latency,
        }
    }
}

fn check_pin(
    route: &str,
    observed: &GenerationPin,
    expected: &GenerationPin,
    start: Instant,
) -> Result<PinGuard, QueryOutcome> {
    if observed == expected {
        Ok(PinGuard {
            latency: start.elapsed(),
        })
    } else {
        Err(QueryOutcome::Failed {
            status: "error",
            code: "stale_generation".to_string(),
            message: format!(
                "{route} response generation {observed:?} differs from published {expected:?}"
            ),
            latency: start.elapsed(),
        })
    }
}

fn failed_outcome(err: &SdkError, start: Instant) -> QueryOutcome {
    let (status, code, message) = classify_sdk_error(err);
    QueryOutcome::Failed {
        status,
        code,
        message,
        latency: start.elapsed(),
    }
}

/// Route-name inventory for record provenance, in canonical order.
#[must_use]
pub fn canonical_routes(routes: &[&str]) -> Vec<String> {
    let mut ordered: BTreeMap<&str, ()> = BTreeMap::new();
    for route in routes {
        let _previous = ordered.insert(*route, ());
    }
    ordered.keys().map(ToString::to_string).collect()
}
