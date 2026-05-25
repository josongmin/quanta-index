//! E2E-00 — reusable tempdir-backed runtime harness.
//!
//! Parent harness for E2E-01..07. Owns a `TempDir` plus a lazily-started
//! searchd driver thread so a single test can: write records through the
//! real `LexicalChannelOp` publish path, seal a generation, drop+reopen
//! the runtime, then issue public `TextQueryRequest`s through the IPC
//! socket and read typed responses back. No in-memory shortcut: every
//! byte goes through the same wire surface the production daemon uses.
//!
//! Lifecycle constraint:
//!
//! The lexical WAL publisher is a single-writer file lock. The running
//! searchd runtime holds that lock for its entire lifetime. That means a
//! test cannot publish chunks while the driver is running. The harness
//! therefore keeps a single owned publisher alive across `ingest_text` /
//! `seal` calls, and lazily starts the driver thread on the first
//! `query_text` (releasing the publisher first). `reopen` tears the
//! driver down, leaves the publisher dropped, restarts the driver against
//! the same `state_root`.
//!
//! The harness intentionally does not add any new public API to
//! `quanta-index-searchd-runtime`. `reopen` is implemented by dropping
//! the current driver thread and reconstructing a fresh runtime over
//! the same `state_root`.

#![expect(
    dead_code,
    reason = "harness API surface is consumed across E2E-00..07; only the inventory smoke exercises a slice today"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use ciborium::into_writer;
use quanta_index_channel::{BundleChannelPublisher, LexicalWalPublisher, open_lexical_publisher};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, LexicalCandidate, LexicalChannelOp, ManifestGeneration,
    RepoId, RepoRelativePath, RevisionId, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax, UpsertChunk,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;
use tempfile::TempDir;

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);
const SOCKET_APPEAR_TIMEOUT: Duration = Duration::from_secs(5);

type DriverJoin = thread::JoinHandle<AnyResult<()>>;

/// Tempdir-backed runtime handle.
#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) struct E2eRuntime {
    tempdir: Option<TempDir>,
    state_root: PathBuf,
    /// Owned publisher kept alive during ingest. Dropped before the driver
    /// is started so the runtime can reacquire the WAL lock.
    publisher: Option<LexicalWalPublisher>,
    driver: Option<DriverState>,
    request_id_counter: AtomicU64,
    generation_counter: u64,
}

struct DriverState {
    socket: PathBuf,
    shutdown: Arc<AtomicBool>,
    join: Option<DriverJoin>,
}

/// Test-only response shape.
///
/// Carries either the candidate list or a typed error code.
/// `engines_touched` is best-effort; the `Text` response variant has no
/// explanation today so this stays empty for plain text queries. Semantic and
/// hybrid rows populate it in later E2E tickets.
#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) struct E2eQueryResult {
    pub(super) candidates: Vec<LexicalCandidate>,
    pub(super) engines_touched: Vec<String>,
    pub(super) typed_error: Option<E2eTypedError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::redundant_pub_crate,
    reason = "sibling test modules import this private-module harness surface"
)]
pub(super) struct E2eTypedError {
    pub(super) code: String,
    pub(super) message: String,
}

impl E2eRuntime {
    /// Create a fresh tempdir and an owned publisher. The driver is NOT
    /// started yet — it boots lazily on first `query_text`.
    pub(super) fn boot() -> AnyResult<Self> {
        let tempdir = tempfile::tempdir()?;
        let state_root = tempdir.path().to_path_buf();
        let publisher = open_lexical_publisher(&state_root)?;
        Ok(Self {
            tempdir: Some(tempdir),
            state_root,
            publisher: Some(publisher),
            driver: None,
            request_id_counter: AtomicU64::new(1),
            generation_counter: 1,
        })
    }

    /// Stop the driver (if running) and reconstruct a publisher over the
    /// same `state_root` so further ingest is possible, then leave the
    /// driver stopped so first query lazy-starts a fresh runtime.
    /// Mirrors a process restart against persistent storage.
    pub(super) fn reopen(mut self) -> AnyResult<Self> {
        self.stop_driver();
        if self.publisher.is_none() {
            self.publisher = Some(open_lexical_publisher(&self.state_root)?);
        }
        Ok(self)
    }

    fn stop_driver(&mut self) {
        if let Some(mut driver) = self.driver.take() {
            driver.shutdown.store(true, Ordering::Release);
            if let Some(join) = driver.join.take() {
                drop(join.join());
            }
        }
    }

    fn ensure_driver(&mut self) -> AnyResult<PathBuf> {
        if self.driver.is_none() {
            // Release publisher lock before booting the runtime.
            drop(self.publisher.take());
            let (socket, shutdown, join) = start_driver(&self.state_root)?;
            self.driver = Some(DriverState {
                socket,
                shutdown,
                join: Some(join),
            });
        }
        self.driver
            .as_ref()
            .map(|driver| driver.socket.clone())
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: driver missing after ensure_driver"))
    }

    pub(super) fn repo(&self) -> RepoId {
        RepoId::new("repo-e2e")
    }

    pub(super) fn revision(&self) -> RevisionId {
        RevisionId::new("rev-e2e")
    }

    /// Current generation pin. Stable until `seal()` is called, then
    /// advances on the next ingest.
    pub(super) fn current_generation(&self) -> ManifestGeneration {
        ManifestGeneration::new(self.generation_counter)
    }

    pub(super) fn generation_pin(&self) -> GenerationPin {
        GenerationPin::new(self.repo(), self.revision(), self.current_generation())
    }

    /// Ingest one chunk through the real `LexicalChannelOp` publish path.
    ///
    /// `_repo` is informational metadata only — the publish itself goes
    /// against the harness's owning `repo()` so the matching query can
    /// pin to a stable triple.
    ///
    /// Requires the driver to be stopped (no concurrent WAL writer). If
    /// the driver is running, this stops it first.
    pub(super) fn ingest_text(&mut self, _repo: &str, path: &str, content: &str) -> AnyResult<()> {
        if self.driver.is_some() {
            self.stop_driver();
        }
        if self.publisher.is_none() {
            self.publisher = Some(open_lexical_publisher(&self.state_root)?);
        }
        let publisher = self
            .publisher
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: publisher missing after re-open"))?;
        let chunk_id = ChunkId::new(format!(
            "e2e-{}-{path}",
            self.request_id_counter.fetch_add(1, Ordering::Relaxed)
        ));
        let record = ChunkRecord {
            repo_relative_path: RepoRelativePath::new(path),
            language: language_from_path(path).to_string().into_boxed_str(),
            start_line: 1,
            end_line: 2,
            snippet: content.to_string().into_boxed_str(),
        };
        let mut buf: Vec<u8> = Vec::new();
        into_writer(&record, &mut buf)?;
        let _seq = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation: self.current_generation(),
            chunk_id,
            payload: buf,
        }))?;
        Ok(())
    }

    /// Seal the current generation. Returns the sealed `ManifestGeneration`
    /// then advances the harness's pin so subsequent ingests target the
    /// next generation.
    pub(super) fn seal(&mut self) -> AnyResult<ManifestGeneration> {
        if self.driver.is_some() {
            self.stop_driver();
        }
        if self.publisher.is_none() {
            self.publisher = Some(open_lexical_publisher(&self.state_root)?);
        }
        let publisher = self
            .publisher
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: publisher missing for seal"))?;
        let sealed = self.current_generation();
        let _seq = publisher.seal(self.repo(), self.revision(), sealed)?;
        self.generation_counter = self.generation_counter.saturating_add(1);
        Ok(sealed)
    }

    /// Issue a `TextQueryRequest` against the live socket, lazy-starting
    /// the driver on first call. The query is pinned to whatever
    /// generation was most recently sealed (i.e. `current_generation() -
    /// 1`), matching how production callers pin queries to a sealed
    /// manifest.
    pub(super) fn query_text(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
    ) -> E2eQueryResult {
        let pin = self.last_sealed_pin();
        self.query_text_with_pin(syntax, query_text, top_k, pin)
    }

    /// Variant that lets a self-test exercise the "no generation pin"
    /// invalid-contract path explicitly.
    pub(super) fn query_text_with_pin(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        pin: Option<GenerationPin>,
    ) -> E2eQueryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax,
                query_text: query_text.to_string(),
                generation: pin,
                generation_selector: None,
                top_k,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    engines_touched: Vec::new(),
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        // Wait for the runtime to leave NOT_READY before reading the real
        // payload — mirrors the readiness wait every existing dsl_scenarios
        // test does. For an invalid-contract request (no pin), the runtime
        // returns INVALID_REQUEST immediately, which already satisfies the
        // "non-NOT_READY" predicate.
        let readiness_reached = wait_until(READINESS_TIMEOUT, || {
            match send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(&socket, &envelope) {
                Ok(response) => match &response.payload {
                    SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                    SearchPlaneQueryIpcResponse::Text(_)
                    | SearchPlaneQueryIpcResponse::Symbol(_)
                    | SearchPlaneQueryIpcResponse::Semantic(_)
                    | SearchPlaneQueryIpcResponse::Hybrid(_)
                    | SearchPlaneQueryIpcResponse::History(_)
                    | SearchPlaneQueryIpcResponse::Structural(_)
                    | SearchPlaneQueryIpcResponse::Bridge(_)
                    | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                    | SearchPlaneQueryIpcResponse::Explain(_)
                    | SearchPlaneQueryIpcResponse::Sourcegraph(_) => true,
                },
                Err(_transport_error) => false,
            }
        });
        let response: SearchPlaneQueryIpcResponseEnvelope = match send_request(&socket, &envelope) {
            Ok(r) => r,
            Err(err) => {
                let message = if readiness_reached {
                    err.to_string()
                } else {
                    format!("readiness timeout before IPC response: {err}")
                };
                return E2eQueryResult {
                    candidates: Vec::new(),
                    engines_touched: Vec::new(),
                    typed_error: Some(E2eTypedError {
                        code: "IPC_TRANSPORT".to_string(),
                        message,
                    }),
                };
            }
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Text(text) => E2eQueryResult {
                candidates: text.results,
                engines_touched: Vec::new(),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                engines_touched: Vec::new(),
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_response("Semantic"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_response("Hybrid"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_response("Structural"),
            SearchPlaneQueryIpcResponse::Bridge(_) => unexpected_response("Bridge"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::Sourcegraph(_) => unexpected_response("Sourcegraph"),
        }
    }

    fn last_sealed_pin(&self) -> Option<GenerationPin> {
        if self.generation_counter <= 1 {
            None
        } else {
            Some(GenerationPin::new(
                self.repo(),
                self.revision(),
                ManifestGeneration::new(self.generation_counter.saturating_sub(1)),
            ))
        }
    }
}

impl Drop for E2eRuntime {
    fn drop(&mut self) {
        self.stop_driver();
        // Drop publisher before tempdir so file locks release first.
        drop(self.publisher.take());
        drop(self.tempdir.take());
    }
}

fn start_driver(state_root: &Path) -> AnyResult<(PathBuf, Arc<AtomicBool>, DriverJoin)> {
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("e2e-harness-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;
    if !wait_until(SOCKET_APPEAR_TIMEOUT, || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(anyhow::anyhow!(
            "e2e-harness: socket {} never appeared",
            socket.display()
        ));
    }
    Ok((socket, shutdown, join))
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf());
    let (query_socket, control_socket) = unique_socket_paths();
    cfg = SearchdConfig::with_socket_overrides(cfg, query_socket, control_socket);
    cfg
}

fn unexpected_response(kind: &str) -> E2eQueryResult {
    E2eQueryResult {
        candidates: Vec::new(),
        engines_touched: Vec::new(),
        typed_error: Some(E2eTypedError {
            code: "UNEXPECTED_RESPONSE".to_string(),
            message: format!("expected Text, got {kind}"),
        }),
    }
}

fn unique_socket_paths() -> (PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query = std::env::temp_dir().join(format!("qi-e2e-query-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-e2e-control-{pid}-{nanos}-{sequence}.sock"));
    (query, control)
}

fn wait_until<F>(timeout: Duration, mut cond: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

fn language_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("js") => "javascript",
        Some("md") => "markdown",
        Some(_) | None => "text",
    }
}
