//! Actual disk-backed runtime child at the selected Active/read-view seam.
//!
//! The only scheduling hook is compiled with `test-runtime-barriers` and
//! installed by this test child. Production state, UDS, catalog, lexical and
//! semantic adapters, activation and retention remain the real owners.

#![forbid(unsafe_code)]

use std::error::Error;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationSelector, ManifestGeneration, RepoId, RepoRelativePath,
    RevisionId, SearchCorpusActiveHeadV1, SearchPlaneErrorCodeV2, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SourceFileKey, SourcePublicationEvent, TextQueryRequest,
    TextQuerySyntax, lex::LanguageCode,
};
use quanta_index_core::{CoreError, GenerationStorageKeyV1};
use quanta_index_ipc::{ClientIoPolicy, send_request};
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SearchCorpusBatch};
use quanta_index_search_plane::test_runtime_barriers::{
    install_lexical_active_before_view, lexical_view_acquire_attempts,
};
use quanta_index_searchd::SearchdConfig;
use quanta_index_searchd_harness::{fixture_source_scope_v1, private_tempdir};

type TestResult = Result<(), Box<dyn Error>>;

// Keep the original thread panic payload in the test error. String payloads
// remain readable; other payloads retain their concrete type for inspection.
struct ThreadPanic {
    worker: &'static str,
    payload: Box<dyn std::any::Any + Send + 'static>,
}

impl std::fmt::Display for ThreadPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(message) = self
            .payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| self.payload.downcast_ref::<&str>().copied())
        {
            write!(f, "{} panicked: {message}", self.worker)
        } else {
            write!(
                f,
                "{} panicked with non-string payload of type {:?}",
                self.worker,
                self.payload.as_ref().type_id()
            )
        }
    }
}

impl std::fmt::Debug for ThreadPanic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl Error for ThreadPanic {}

const REPO: &str = "repo-active-process-race";
const REVISION: &str = "revision-active-process-race";
const WAIT: Duration = Duration::from_secs(10);
const RACE_WAIT: Duration = Duration::from_secs(90);
const CHILD_ROOT_ENV: &str = "QI_E3_01_ACTIVE_CHILD_ROOT";
const CHILD_GATE_ENV: &str = "QI_E3_01_ACTIVE_CHILD_GATE";
const CHILD_INSPECT_ENV: &str = "QI_E3_01_ACTIVE_CHILD_INSPECT";
const CHILD_STOP_ENV: &str = "QI_E3_01_ACTIVE_CHILD_STOP";

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new(REPO)?)
}

fn revision() -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new(REVISION)?)
}

fn corpus_batch(number: u64) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let repo = repo()?;
    let revision = revision()?;
    let path = "src/selection.rs";
    let text = format!("fn selected_g{number}() {{}} ");
    let scope = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id: repo.clone(),
            repo_relative_path: RepoRelativePath::new(path),
        },
        revision.clone(),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("selected-g{number}-chunk")),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte: u32::try_from(text.len())?,
            start_line: 1,
            end_line: 1,
            text: text.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(repo.clone()),
        }],
        Vec::new(),
    )?;
    Ok(SearchCorpusBatch::replace_generation(
        repo,
        revision,
        ManifestGeneration::new(number),
        format!("manifest:active-process-g{number}"),
    )
    .source_event(SourcePublicationEvent {
        stream_id: "fixture:active-process-race".to_string(),
        event_id: format!("fixture:active-process-race:g{number}"),
        expected_base_event_id: number
            .checked_sub(1)
            .filter(|previous| *previous != 0)
            .map(|previous| format!("fixture:active-process-race:g{previous}")),
        payload_sha256: [0; 32],
    })
    .replace_scope(
        scope.coverage,
        scope.source_bytes,
        scope.chunks,
        scope.symbols,
    ))
}

fn publish_and_activate(
    client: &QuantaIndex,
    number: u64,
    expected: Option<SearchCorpusActiveHeadV1>,
) -> Result<SearchCorpusActiveHeadV1, Box<dyn Error>> {
    let (_receipt, activation) = client
        .search_corpus()
        .publish_and_activate(&corpus_batch(number)?, expected)?;
    let head = activation.active;
    if head.generation.lexical.manifest_generation != ManifestGeneration::new(number)
        || head.generation.semantic.manifest_generation != ManifestGeneration::new(number)
    {
        return Err(format!("generation {number} did not activate both tracks: {head:?}").into());
    }
    Ok(head)
}

fn query(
    socket: &Path,
    request_id: u64,
    phrase: &str,
) -> Result<SearchPlaneQueryIpcResponse, Box<dyn Error>> {
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: phrase.to_owned(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(GenerationSelector::Active {
                repo_id: repo()?,
                revision_id: revision()?,
            }),
            top_k: 5,
            cursor: None,
        }),
    };
    let response: SearchPlaneQueryIpcResponseEnvelope = send_request(
        socket,
        &request,
        ClientIoPolicy::try_new(Duration::from_secs(120))?,
    )?;
    if response.request_id != request_id {
        return Err("query response request ID changed".into());
    }
    Ok(response.payload)
}

struct OwnedChild(Option<Child>);

impl OwnedChild {
    fn kill_and_reap(&mut self) -> TestResult {
        if let Some(mut child) = self.0.take() {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            let _status = child.wait()?;
        }
        Ok(())
    }

    fn await_clean_exit(&mut self) -> TestResult {
        let started = Instant::now();
        loop {
            let child = self.0.as_mut().ok_or("active child already reaped")?;
            if let Some(status) = child.try_wait()? {
                let _reaped_child = self.0.take();
                if status.success() {
                    return Ok(());
                }
                return Err(format!("active runtime child exited unsuccessfully: {status}").into());
            }
            if started.elapsed() >= WAIT {
                return Err("active runtime child did not stop within bound".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _cleanup = self.kill_and_reap();
    }
}

fn run_child() -> TestResult {
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT_ENV).ok_or("active child root missing")?);
    let gate_path =
        PathBuf::from(std::env::var_os(CHILD_GATE_ENV).ok_or("active child gate missing")?);
    let inspect_path =
        PathBuf::from(std::env::var_os(CHILD_INSPECT_ENV).ok_or("active child inspector missing")?);
    let stop_path =
        PathBuf::from(std::env::var_os(CHILD_STOP_ENV).ok_or("active child stop missing")?);
    let entered = Arc::new(AtomicBool::new(false));
    let single_gate = Arc::clone(&entered);
    let _installed = install_lexical_active_before_view(move |pin| {
        if single_gate.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let mut gate = UnixStream::connect(&gate_path)
            .map_err(|error| CoreError::Storage(format!("active child gate connect: {error}")))?;
        gate.set_read_timeout(Some(RACE_WAIT))
            .map_err(|error| CoreError::Storage(format!("active child gate deadline: {error}")))?;
        gate.write_all(&pin.manifest_generation.get().to_le_bytes())
            .map_err(|error| CoreError::Storage(format!("active child selected pin: {error}")))?;
        let mut release = [0];
        gate.read_exact(&mut release)
            .map_err(|error| CoreError::Storage(format!("active child gate release: {error}")))?;
        if release != [1] {
            return Err(CoreError::Storage(
                "active child gate released with wrong token".into(),
            ));
        }
        Ok(())
    })?;
    let inspector = UnixListener::bind(&inspect_path)?;
    inspector.set_nonblocking(true)?;
    let config = SearchdConfig::from_test_state_root(root)
        .try_with_search_corpus_history_retention_limits(
            2,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )?;
    let runtime = quanta_index_searchd_runtime::build_runtime(config)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&shutdown);
    let monitor = thread::spawn(move || -> Result<(), String> {
        let started = Instant::now();
        loop {
            match inspector.accept() {
                Ok((mut peer, _)) => {
                    peer.set_nonblocking(false).map_err(|error| {
                        format!("active child inspector blocking mode: {error}")
                    })?;
                    peer.write_all(&lexical_view_acquire_attempts().to_le_bytes())
                        .map_err(|error| format!("active child inspector write: {error}"))?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("active child inspector accept: {error}")),
            }
            if stop_path.exists() {
                signal.store(true, Ordering::Release);
                return Ok(());
            }
            if started.elapsed() >= Duration::from_secs(120) {
                signal.store(true, Ordering::Release);
                return Err("active child parent never completed the proof".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    let result = quanta_index_searchd::drive(runtime, &shutdown);
    shutdown.store(true, Ordering::Release);
    monitor.join().map_err(|payload| ThreadPanic {
        worker: "active child inspector monitor",
        payload,
    })??;
    result?;
    Ok(())
}

fn wait_for_control(root: &Path, child: &mut OwnedChild) -> Result<QuantaIndex, Box<dyn Error>> {
    let started = Instant::now();
    loop {
        if let Ok(client) = QuantaIndex::connect(ConnectOptions::from_state_root(root))
            && client.observability().metrics_snapshot().is_ok()
        {
            return Ok(client);
        }
        if let Some(status) = child
            .0
            .as_mut()
            .ok_or("active child already reaped")?
            .try_wait()?
        {
            return Err(format!("active child exited before control UDS: {status}").into());
        }
        if started.elapsed() >= WAIT {
            return Err("active child control UDS did not become ready".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_selected_pin(
    listener: &UnixListener,
    child: &mut OwnedChild,
) -> Result<UnixStream, Box<dyn Error>> {
    listener.set_nonblocking(true)?;
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut gate, _)) => {
                gate.set_nonblocking(false)
                    .map_err(|error| format!("active parent gate blocking mode: {error}"))?;
                gate.set_read_timeout(Some(WAIT))
                    .map_err(|error| format!("active parent gate read deadline: {error}"))?;
                let mut encoded = [0; 8];
                gate.read_exact(&mut encoded)
                    .map_err(|error| format!("active parent selected pin read: {error}"))?;
                if u64::from_le_bytes(encoded) != 1 {
                    return Err(format!(
                        "Active query selected G{}, expected G1",
                        u64::from_le_bytes(encoded)
                    )
                    .into());
                }
                return Ok(gate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        if let Some(status) = child
            .0
            .as_mut()
            .ok_or("active child already reaped")?
            .try_wait()?
        {
            return Err(format!("active child exited before selected gate: {status}").into());
        }
        if started.elapsed() >= WAIT {
            return Err("Active query did not reach selected/view barrier".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn acquire_attempts(inspect: &Path) -> Result<u64, Box<dyn Error>> {
    let mut stream = UnixStream::connect(inspect)?;
    stream.set_read_timeout(Some(WAIT))?;
    let mut bytes = [0; 8];
    stream.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

/// The query chooses G1 inside the child, pauses before ledger/view acquire,
/// and resumes only after actual G2/G3 activation physically retires G1.
#[test]
fn os_child_active_selection_retired_before_view_refuses_without_opening_g1() -> TestResult {
    if std::env::var_os(CHILD_ROOT_ENV).is_some() {
        return run_child();
    }
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let gate_path = parent.path().join("g.sock");
    let inspect_path = parent.path().join("i.sock");
    let stop_path = parent.path().join("stop");
    let listener = UnixListener::bind(&gate_path)?;
    let child = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("os_child_active_selection_retired_before_view_refuses_without_opening_g1")
        .arg("--nocapture")
        .env(CHILD_ROOT_ENV, &state_root)
        .env(CHILD_GATE_ENV, &gate_path)
        .env(CHILD_INSPECT_ENV, &inspect_path)
        .env(CHILD_STOP_ENV, &stop_path)
        .spawn()?;
    let mut child = OwnedChild(Some(child));
    let client = wait_for_control(&state_root, &mut child)?;
    let g1 = publish_and_activate(&client, 1, None)?;
    let storage = GenerationStorageKeyV1::for_repo_revision(&repo()?, &revision()?);
    let root = std::fs::canonicalize(&state_root)?;
    let old_lexical =
        storage.generation_dir(&root.join("indexes/lexical"), ManifestGeneration::new(1));
    let old_semantic =
        storage.generation_dir(&root.join("indexes/semantic"), ManifestGeneration::new(1));
    if !old_lexical.is_dir() || !old_semantic.is_dir() {
        return Err("G1 physical tracks absent before retention".into());
    }
    let query_socket = state_root.join("search-plane/query.sock");
    let stalled_socket = query_socket.clone();
    let stalled = thread::spawn(move || {
        query(&stalled_socket, 0xe301, "selected_g1").map_err(|error| error.to_string())
    });
    let mut gate = wait_for_selected_pin(&listener, &mut child)?;
    let g2 = publish_and_activate(&client, 2, Some(g1.clone()))?;
    let g3 = publish_and_activate(&client, 3, Some(g2))?;
    if old_lexical.exists() || old_semantic.exists() {
        return Err("G1 physical lexical/semantic tracks survived two-generation retention".into());
    }
    if g3.activation_token == g1.activation_token {
        return Err("G3 did not advance the Active token".into());
    }
    let opens_before = acquire_attempts(&inspect_path)?;
    gate.write_all(&[1])
        .map_err(|error| format!("active parent gate release write: {error}"))?;
    let stale = stalled.join().map_err(|payload| ThreadPanic {
        worker: "stalled query thread",
        payload,
    })??;
    if !matches!(
        &stale,
        SearchPlaneQueryIpcResponse::Error(error)
            if error.code == SearchPlaneErrorCodeV2::UnknownGeneration
    ) {
        return Err(format!("retired selected G1 did not refuse typed: {stale:?}").into());
    }
    if acquire_attempts(&inspect_path)? != opens_before {
        return Err("retired selected G1 reached lexical view acquisition".into());
    }
    let active = query(&query_socket, 0xe302, "selected_g3")?;
    let SearchPlaneQueryIpcResponse::Text(page) = active else {
        return Err(format!("fresh Active G3 did not serve text: {active:?}").into());
    };
    if page.generation.manifest_generation != ManifestGeneration::new(3)
        || page.selected_active_head.as_ref() != Some(&g3)
        || page.results.len() != 1
        || page
            .results
            .iter()
            .any(|row| row.manifest_generation != ManifestGeneration::new(3))
    {
        return Err(format!("fresh Active G3 response mixed or omitted identity: {page:?}").into());
    }
    if acquire_attempts(&inspect_path)? != opens_before + 1 {
        return Err("fresh Active G3 did not acquire one lexical view".into());
    }
    std::fs::write(&stop_path, b"stop")?;
    child.await_clean_exit()
}
