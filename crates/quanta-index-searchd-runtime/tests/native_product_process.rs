//! The product daemon entry must serve a configured native profile over its real UDS.
//! A listener alone is insufficient: this test publishes and queries one file.
#[path = "native_product_process/ann_quality.rs"]
mod ann_quality;
#[path = "native_product_process/ann_regions.rs"]
mod ann_regions;
#[path = "native_product_process/native_boot.rs"]
mod native_boot;
#[path = "common/native_physical_profile.rs"]
mod native_physical_profile;
#[path = "native_product_process/uds_query_trace.rs"]
mod uds_query_trace;

use std::{
    error::Error,
    fmt::Debug,
    fs,
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::{
        fs::{MetadataExt as _, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use quanta_index_contract::{
    QueryConstraintSetV1, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
    index::{
        CollectionId, CoverageStatus, Digest, GroupActivation, IndexInstanceId, IndexKind,
        IndexProfile, InputDomain, InstanceSpec, OperationKey, OperationStatus, OwnerKey,
        OwnerMutation, ProducerId, PublicationGroupId, Refusal, SchemaId, SlotId, SourceEvent,
        SourceInput, SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId,
        StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewSelection, WorkspaceKey,
        WorktreeId,
    },
    native_protocol::{
        NativeEffortPreference, NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse,
        NativeTextResult,
    },
};
use quanta_index_core::{
    L2UnitEmbeddingProvider, RequestBudgetV1,
    index::{ObjectRef, WorkBudget, WorkLimits, ports::ImmutableObjectPort},
};
use quanta_index_embed::{OpenAiEmbeddingProvider, OpenAiProviderConfig};
use quanta_index_lexical::native::TextRecipe;
use quanta_index_sdk::native_uds::{
    NativeEffortStreamQuery, NativeEffortStreamStage, NativeUdsClient, NativeUdsError,
};
use quanta_index_semantic::native::TextEmbeddingBridge;
use quanta_index_storage::FileObjectStore;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
// A functional process watchdog includes platform executable assessment. The
// config-preflight refusal deadline below measures only the entered phase.
const NATIVE_STARTUP_LIMIT: Duration = Duration::from_secs(30);
const NATIVE_CONFIG_PREFLIGHT_STAGE: &str = "searchd: native_boot_stage=config_preflight";
const STDERR_OBSERVATION_BYTES: u64 = 16_384;

fn stderr_prefix(path: &Path) -> TestResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let _read_bytes = fs::File::open(path)?
        .take(STDERR_OBSERVATION_BYTES)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn require(condition: bool, message: impl Into<String>) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

fn require_equal<T: Debug + PartialEq>(actual: &T, expected: &T, label: &str) -> TestResult {
    require(
        actual == expected,
        format!("{label}: expected {expected:?}, got {actual:?}"),
    )
}

fn set_fixture_json(
    value: &mut serde_json::Value,
    pointer: &str,
    replacement: serde_json::Value,
) -> TestResult {
    let target = value
        .pointer_mut(pointer)
        .ok_or_else(|| format!("missing config fixture field {pointer}"))?;
    *target = replacement;
    Ok(())
}

fn parse_native_trace_max_output_bytes(raw: Option<&std::ffi::OsStr>) -> TestResult<Option<u64>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = raw
        .to_str()
        .ok_or("QIDX_NATIVE_TRACE_MAX_OUTPUT_BYTES must be UTF-8")?;
    let bytes = value
        .parse::<u64>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let _validated_limits =
        quanta_index_searchd::DiagnosticCaptureLimitsV1::default().with_max_output_bytes(bytes)?;
    Ok(Some(bytes))
}

#[test]
fn native_trace_output_limit_env_is_explicit_and_bounded() -> TestResult {
    use std::os::unix::ffi::OsStringExt as _;

    require_equal(
        &parse_native_trace_max_output_bytes(None)?,
        &None,
        "absent cap",
    )?;
    require_equal(
        &parse_native_trace_max_output_bytes(Some(std::ffi::OsStr::new("1073741824")))?,
        &Some(1_073_741_824),
        "maximum cap",
    )?;
    for invalid in [
        "0",
        "6143",
        "1073741825",
        "-1",
        "true",
        "18446744073709551616",
    ] {
        require(
            parse_native_trace_max_output_bytes(Some(std::ffi::OsStr::new(invalid))).is_err(),
            format!("invalid cap accepted: {invalid}"),
        )?;
    }
    let invalid_unicode = std::ffi::OsString::from_vec(vec![0xff]);
    require(
        parse_native_trace_max_output_bytes(Some(invalid_unicode.as_os_str())).is_err(),
        "non-UTF-8 cap accepted",
    )
}

struct Daemon(Child, Option<tempfile::NamedTempFile>);

impl Daemon {
    fn start(config: &Path, socket: &Path) -> TestResult<Self> {
        Self::start_with_api_key(config, socket, None)
    }

    fn start_with_api_key(config: &Path, socket: &Path, api_key: Option<&str>) -> TestResult<Self> {
        Self::start_with_api_key_and_trace(config, socket, api_key, None)
    }

    fn start_with_api_key_and_trace(
        config: &Path,
        socket: &Path,
        api_key: Option<&str>,
        trace: Option<&Path>,
    ) -> TestResult<Self> {
        let trace_max_output_bytes = if trace.is_some() {
            parse_native_trace_max_output_bytes(
                std::env::var_os("QIDX_NATIVE_TRACE_MAX_OUTPUT_BYTES").as_deref(),
            )?
        } else {
            None
        };
        let stderr_log = tempfile::NamedTempFile::new_in(
            config
                .parent()
                .ok_or("native config has no parent directory")?,
        )?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"));
        let _args = command
            .arg("serve-native")
            .arg("--config")
            .arg(config)
            // Failure-only bounded diagnostics are identical in Off/Record.
            .env("QUANTA_INDEX_NATIVE_WORK_DIAGNOSTICS_V1", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_log.reopen()?));
        if let Some(api_key) = api_key {
            let _environment = command.env("QIDX_NATIVE_TEST_API_KEY", api_key);
        }
        if let Some(trace) = trace {
            let _trace = command.arg("--diagnostic-trace").arg(trace);
            if let Some(bytes) = trace_max_output_bytes {
                let _limit = command
                    .arg("--trace-max-output-bytes")
                    .arg(bytes.to_string());
            }
        }
        let child = command.spawn()?;
        let mut daemon = Self(child, Some(stderr_log));
        let deadline = Instant::now()
            .checked_add(NATIVE_STARTUP_LIMIT)
            .ok_or("native startup deadline overflow")?;
        loop {
            if UnixStream::connect(socket).is_ok() {
                return Ok(daemon);
            }
            if let Some(status) = daemon.0.try_wait()? {
                let mut stderr = String::new();
                let _stderr_bytes = daemon
                    .stderr_reader()?
                    .take(64 * 1024)
                    .read_to_string(&mut stderr)?;
                return Err(format!("native daemon exited {status}: {stderr}").into());
            }
            if Instant::now() >= deadline {
                return Err("native daemon did not bind UDS within thirty seconds".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stderr_reader(&self) -> TestResult<fs::File> {
        Ok(self
            .1
            .as_ref()
            .ok_or("native daemon stderr log is absent")?
            .reopen()?)
    }

    fn stop(mut self) -> TestResult {
        self.stop_in_place()
    }

    fn stop_in_place(&mut self) -> TestResult {
        if let Some(status) = self.0.try_wait()? {
            return Err(format!("native daemon exited before shutdown: {status}").into());
        }
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(self.0.id().to_string())
            .status()?;
        if !status.success() {
            return Err("failed to signal native daemon".into());
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("native shutdown deadline overflow")?;
        loop {
            if let Some(status) = self.0.try_wait()? {
                if !status.success() {
                    return Err(format!("native daemon did not drain cleanly: {status}").into());
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("native daemon did not drain after SIGTERM".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[derive(Clone, Copy)]
enum EmbeddingFixture {
    Fixed8,
    AnnQuality,
    AnnFiltered,
}

impl EmbeddingFixture {
    fn dimension(self) -> usize {
        match self {
            Self::Fixed8 => 8,
            Self::AnnQuality | Self::AnnFiltered => {
                quanta_index_searchd_harness::ann::AnnRailConfig::DEFAULT.width
            }
        }
    }

    fn vector(self, input: &str) -> TestResult<Vec<f32>> {
        use quanta_index_searchd_harness::ann::{AnnRailConfig, query_direction, unit_direction};

        match self {
            Self::Fixed8 => Ok(vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            Self::AnnQuality | Self::AnnFiltered => {
                let filtered = matches!(self, Self::AnnFiltered);
                let config = if filtered {
                    ann_quality::FILTERED_CONFIG
                } else {
                    AnnRailConfig::DEFAULT
                };
                if let Some(input_row) = input.strip_prefix("row-") {
                    let row_text = if filtered {
                        input_row
                            .split_once(' ')
                            .map(|(row, _suffix)| row)
                            .ok_or("filtered ANN row lacks fixed lexical markers")?
                    } else {
                        input_row
                    };
                    let row = row_text.parse::<u64>()?;
                    let expected = if filtered {
                        ann_quality::filtered_quality::source_text(row)
                    } else {
                        format!("row-{row}")
                    };
                    require(
                        row < config.rows && input == expected,
                        "ANN provider received an invalid corpus row",
                    )?;
                    Ok(unit_direction(row, config.width))
                } else if let Some(query) = input.strip_prefix("query-") {
                    let query = query.parse::<u64>()?;
                    require(
                        query < config.queries && input == format!("query-{query}"),
                        "ANN provider received an invalid query",
                    )?;
                    Ok(query_direction(query, &config)?)
                } else {
                    Err("ANN provider received an unknown input".into())
                }
            }
        }
    }
}

struct EmbeddingStub {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    inputs: Arc<Mutex<Vec<String>>>,
    errors: Arc<Mutex<Vec<String>>>,
    join: Option<thread::JoinHandle<()>>,
}

impl EmbeddingStub {
    fn start() -> TestResult<Self> {
        Self::start_fixture(EmbeddingFixture::Fixed8)
    }

    fn start_ann_quality() -> TestResult<Self> {
        Self::start_fixture(EmbeddingFixture::AnnQuality)
    }

    fn start_ann_filtered() -> TestResult<Self> {
        Self::start_fixture(EmbeddingFixture::AnnFiltered)
    }

    fn start_fixture(fixture: EmbeddingFixture) -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let thread_stop = Arc::clone(&stop);
        let thread_inputs = Arc::clone(&inputs);
        let thread_errors = Arc::clone(&errors);
        let join = thread::Builder::new()
            .name("native-semantic-http-stub".into())
            .spawn(move || {
                while !thread_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Err(error) =
                                serve_embedding_request(stream, &thread_inputs, fixture)
                                && let Ok(mut errors) = thread_errors.lock()
                            {
                                errors.push(error.to_string());
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => {
                            if let Ok(mut errors) = thread_errors.lock() {
                                errors.push(error.to_string());
                            }
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            address,
            stop,
            inputs,
            errors,
            join: Some(join),
        })
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn observed_inputs(&self) -> TestResult<Vec<String>> {
        let errors = self
            .errors
            .lock()
            .map_err(|error| format!("stub error log poisoned: {error}"))?;
        let healthy = require(errors.is_empty(), format!("HTTP stub failures: {errors:?}"));
        drop(errors);
        healthy?;
        Ok(self
            .inputs
            .lock()
            .map_err(|error| format!("stub inputs poisoned: {error}"))?
            .clone())
    }

    fn finish(mut self) -> TestResult {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            join.join().map_err(|_panic| "HTTP stub worker panicked")?;
        }
        let _observed = self.observed_inputs()?;
        Ok(())
    }
}

impl Drop for EmbeddingStub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let joined = join.join();
            assert!(
                joined.is_ok() || thread::panicking(),
                "HTTP embedding stub worker panicked"
            );
        }
    }
}

fn serve_embedding_request(
    mut stream: TcpStream,
    inputs: &Arc<Mutex<Vec<String>>>,
    fixture: EmbeddingFixture,
) -> TestResult {
    // The listener polls without blocking; each accepted HTTP exchange uses
    // blocking IO with the explicit timeouts below on every supported OS.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut next = [0_u8; 4096];
        let count = stream.read(&mut next)?;
        if count == 0 || bytes.len().saturating_add(count) > 65_536 {
            return Err("HTTP embedding request was truncated or oversized".into());
        }
        bytes.extend_from_slice(
            next.get(..count)
                .ok_or("HTTP header chunk exceeds buffer")?,
        );
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position.checked_add(4).ok_or("HTTP header size overflow")?;
        }
    };
    let headers = std::str::from_utf8(bytes.get(..header_end).ok_or("HTTP header missing")?)?;
    require(
        headers.starts_with("POST /v1/embeddings "),
        "embedding request route differs",
    )?;
    let length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim())
        })
        .ok_or("embedding request has no Content-Length")?
        .parse::<usize>()?;
    let body_end = header_end
        .checked_add(length)
        .ok_or("HTTP embedding body size overflow")?;
    require(body_end <= 65_536, "HTTP embedding body exceeds limit")?;
    while bytes
        .len()
        .checked_sub(header_end)
        .ok_or("HTTP header exceeds request")?
        < length
    {
        let mut next = [0_u8; 4096];
        let count = stream.read(&mut next)?;
        if count == 0 || bytes.len().saturating_add(count) > 65_536 {
            return Err("HTTP embedding body was truncated or oversized".into());
        }
        bytes.extend_from_slice(next.get(..count).ok_or("HTTP body chunk exceeds buffer")?);
    }
    let body = bytes
        .get(header_end..body_end)
        .ok_or("HTTP embedding body missing")?;
    let body: serde_json::Value = serde_json::from_slice(body)?;
    require_equal(
        &body.get("model").and_then(serde_json::Value::as_str),
        &Some("test-model"),
        "embedding fixture model",
    )?;
    require_equal(
        &body.get("dimensions").and_then(serde_json::Value::as_u64),
        &Some(u64::try_from(fixture.dimension())?),
        "embedding fixture dimension",
    )?;
    let [input] = body
        .get("input")
        .and_then(serde_json::Value::as_array)
        .ok_or("embedding input was absent")?
        .as_slice()
    else {
        return Err("native Semantic must send one input per owner or query".into());
    };
    let input = input.as_str().ok_or("embedding input was not UTF-8 text")?;
    inputs
        .lock()
        .map_err(|error| format!("stub inputs poisoned: {error}"))?
        .push(input.to_owned());
    let response = serde_json::to_vec(&serde_json::json!({
        "data": [{"embedding": fixture.vector(input)?, "index": 0}]
    }))?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.len()
    )?;
    stream.write_all(&response)?;
    stream.flush()?;
    Ok(())
}

impl Drop for Daemon {
    fn drop(&mut self) {
        drop(self.0.kill());
        drop(self.0.wait());
    }
}

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn text_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([71; 16]),
        kind: IndexKind([72; 16]),
        group: PublicationGroupId([73; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([74; 16]),
        policy: TextRecipe::new(Digest([31; 32])).output_policy(),
        format: StorageFormatId([75; 16]),
    }
}

fn budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(30))
}

fn native_cli(args: Vec<String>) -> TestResult<Vec<u8>> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = quanta_index_searchctl::run(args, &mut stdout, &mut stderr);
    if exit != std::process::ExitCode::SUCCESS {
        return Err(format!(
            "native CLI exited {exit:?}: {}",
            String::from_utf8_lossy(&stderr)
        )
        .into());
    }
    Ok(stdout)
}

fn published(outcome: &SubmitOutcome) -> TestResult<quanta_index_contract::index::ViewRef> {
    let SubmitOutcome::Terminal(OperationStatus::Published(view)) = outcome else {
        return Err(format!("expected published view, got {outcome:?}").into());
    };
    Ok(*view)
}

fn config(root: &Path, socket: &Path) -> serde_json::Value {
    let unit = serde_json::json!({
        "cpu_units": 1,
        "io_bytes": 67_108_864,
        "memory_bytes": 67_108_864,
        "provider_units": 0
    });
    let physical = native_physical_profile::profile();
    serde_json::json!({
        "version": 1,
        "state_root": root.to_string_lossy(),
        "socket_path": socket.to_string_lossy(),
        "work_limits": {"reads": 1_000_000, "writes": 1_000_000, "bytes": 536_870_912, "mutations": 1_000_000},
        "resources": {
            "physical": {
                "inventory": {
                    "max_bytes": physical.inventory.max_bytes,
                    "max_files": physical.inventory.max_files,
                    "max_pending": physical.inventory.max_pending,
                    "max_entries": physical.inventory.max_entries,
                    "max_depth": physical.inventory.max_depth,
                    "max_pending_frames": physical.inventory.max_pending_frames
                },
                "max_reader_handles": physical.max_reader_handles,
                "max_tracked_entries": physical.max_tracked_entries,
                "max_process_roots": physical.max_process_roots,
                "global_max_sqlite_handles": physical.global_max_sqlite_handles,
                "maintenance_reserved_bytes": physical.maintenance_reserved_bytes,
                "maintenance_reserved_files": physical.maintenance_reserved_files
            },
            "scratch_reserved_bytes": 17_179_869_184_u64,
            "read_ticket_lease_ms": 30000,
            "execution": {
                "capacity": {"cpu_units": 4, "io_bytes": 268_435_456, "memory_bytes": 335_544_320, "provider_units": 0},
                "control": unit,
                "interactive": unit,
                "core": unit,
                "enrichment": unit
            }
        },
        "literal": {"kind": vec![8_u8; 16], "schema": vec![10_u8; 16], "format": vec![12_u8; 16]},
        "file_map": {"kind": vec![18_u8; 16], "schema": vec![10_u8; 16], "format": vec![19_u8; 16]},
        "text": {
            "descriptor": {"kind": vec![72_u8; 16], "schema": vec![74_u8; 16], "format": vec![75_u8; 16]},
            "admission_policy": vec![31_u8; 32],
            "bm25": {"k1": 1.2, "b": 0.75},
            "read_need": unit
        }
    })
}

fn config_with_reclaim(root: &Path, socket: &Path) -> TestResult<serde_json::Value> {
    let mut value = config(root, socket);
    for resource in ["cpu_units", "io_bytes", "memory_bytes"] {
        let capacity = value
            .pointer_mut(&format!("/resources/execution/capacity/{resource}"))
            .ok_or("missing capacity in native config fixture")?;
        *capacity = match resource {
            "cpu_units" => serde_json::json!(5),
            "memory_bytes" => serde_json::json!(402_653_184_u64),
            _ => serde_json::json!(335_544_320_u64),
        };
    }
    let object = value
        .as_object_mut()
        .ok_or("native config fixture object")?;
    let old = object.insert("maintenance".into(), serde_json::json!({
        "interval_ms": 20,
        "max_steps_per_round": 1,
        "child_limit": 32,
        "work_limits": {"reads": 1_000_000, "writes": 1_000_000, "bytes": 536_870_912, "mutations": 1_000_000},
        "round_timeout_ms": 2_000,
        "head_probe_every": 4
    }));
    require(old.is_none(), "maintenance fixture field already exists")?;
    Ok(value)
}

fn config_with_openai_semantic(root: &Path, socket: &Path) -> TestResult<serde_json::Value> {
    config_with_openai_semantic_dimension(root, socket, 8)
}

fn config_with_openai_semantic_dimension(
    root: &Path,
    socket: &Path,
    dimension: u32,
) -> TestResult<serde_json::Value> {
    let mut value = config(root, socket);
    set_fixture_json(
        &mut value,
        "/resources/execution/capacity/memory_bytes",
        serde_json::json!(335_544_320_u64),
    )?;
    set_fixture_json(
        &mut value,
        "/resources/execution/capacity/provider_units",
        serde_json::json!(2),
    )?;
    for class in ["interactive", "enrichment"] {
        set_fixture_json(
            &mut value,
            &format!("/resources/execution/{class}/provider_units"),
            serde_json::json!(1),
        )?;
    }
    let semantic = serde_json::json!({
        "scopes": [{"workspace": workspace(), "epoch": 1}],
        "descriptor": {"kind": vec![81_u8; 16], "schema": vec![82_u8; 16], "format": vec![83_u8; 16]},
        "provider": {
            "kind": "open_ai", "model": "test-model", "model_revision": "immutable-v1",
            "dimension": dimension, "api_key_env": "QIDX_NATIVE_TEST_API_KEY",
            "base_url": "https://api.openai.com", "max_batch": 8,
            "max_estimated_tokens_per_request": 1024, "max_retries": 0,
            "timeout_ms": 1000, "concurrency": 1, "max_response_bytes": 65_536,
            "egress": {
                "tenant_id": workspace().tenant, "endpoint": "https://api.openai.com/v1/embeddings",
                "region": "test-region", "retention": "test-retention", "profile": "native-semantic-test",
                "source_content_consent": true
            }
        },
        "max_source_bytes": 4096,
        "read": {"page_size": 8, "max_owners": 8, "max_query_bytes": 1024,
                 "need": {"cpu_units": 1, "io_bytes": 67_108_864,
                          "memory_bytes": 67_108_864, "provider_units": 1}}
    });
    let old = value
        .as_object_mut()
        .ok_or("native config fixture object")?
        .insert("semantic".into(), semantic);
    require(old.is_none(), "semantic fixture field already exists")?;
    Ok(value)
}

fn configure_semantic_endpoint(value: &mut serde_json::Value, base_url: &str) -> TestResult {
    set_fixture_json(
        value,
        "/semantic/provider/base_url",
        serde_json::json!(base_url),
    )?;
    set_fixture_json(
        value,
        "/semantic/provider/egress/endpoint",
        serde_json::json!(format!("{base_url}/v1/embeddings")),
    )
}

fn configure_owned_semantic(value: &mut serde_json::Value) -> TestResult {
    for (resource, capacity) in [
        ("cpu_units", 5_u64),
        ("io_bytes", 335_544_320),
        ("memory_bytes", 402_653_184),
    ] {
        set_fixture_json(
            value,
            &format!("/resources/execution/capacity/{resource}"),
            serde_json::json!(capacity),
        )?;
    }
    let owned_work = serde_json::json!({
        "scopes": [{"workspace": workspace(), "epoch": 1}],
        "batch_limit": 1, "max_in_flight": 1, "poll_interval_ms": 20,
        "dispatch_limits": {"reads": 1_000_000, "writes": 1_000_000,
                            "bytes": 536_870_912, "mutations": 1_000_000},
        "max_dispatches": 2, "total_timeout_ms": 10_000,
        "lease_ms": 2_000, "retry_backoff_ms": 100
    });
    let old = value
        .as_object_mut()
        .ok_or("native config fixture object")?
        .insert("owned_work".into(), owned_work);
    require(old.is_none(), "owned work fixture field already exists")
}

fn configure_effort(value: &mut serde_json::Value) -> TestResult {
    let effort = serde_json::json!({
        "thresholds": {
            "interactive_low_bps": 1000, "interactive_high_bps": 8000,
            "backlog_low": 1, "backlog_high": 100,
            "lag_low_ms": 10, "lag_high_ms": 1000,
            "memory_high_pressure_bytes": 1024,
            "memory_clear_bytes": 1_048_576
        },
        "stable_idle_ms": 1, "clear_dwell_ms": 1, "minimum_residence_ms": 0,
        "worker_lag_max_age_ms": 1000, "sample_ttl_ms": 1000,
        "interaction_source": "registered_foreground_publisher"
    });
    let old = value
        .as_object_mut()
        .ok_or("native config fixture object")?
        .insert("effort".into(), effort);
    require(old.is_none(), "effort fixture field already exists")
}

fn openai_semantic_spec(base_url: &str) -> TestResult<InstanceSpec> {
    openai_semantic_spec_dimension(base_url, 8)
}

fn openai_semantic_spec_dimension(base_url: &str, dimension: u32) -> TestResult<InstanceSpec> {
    let provider = OpenAiEmbeddingProvider::with_reqwest(
        OpenAiProviderConfig::new(
            "test-key".into(),
            "test-model".into(),
            "immutable-v1".into(),
            usize::try_from(dimension)?,
        )
        .with_base_url(base_url.into())
        .with_max_batch(8)
        .with_max_estimated_tokens_per_request(1024)
        .with_max_retries(0)
        .with_timeout(Duration::from_secs(1))
        .with_concurrency(1),
    )?;
    let bridge = TextEmbeddingBridge::new(L2UnitEmbeddingProvider::new(provider)?)?;
    let recipe = bridge.recipe(4096)?;
    Ok(InstanceSpec {
        instance: IndexInstanceId([80; 16]),
        kind: IndexKind([81; 16]),
        group: PublicationGroupId([84; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([82; 16]),
        policy: recipe.fingerprint(),
        format: StorageFormatId([83; 16]),
    })
}

fn expect_native_config_refusal(
    root: &Path,
    socket: &Path,
    path: &Path,
    expected: &str,
) -> TestResult {
    let stderr_path = path.with_extension("stderr");
    let stderr_file = fs::File::create(&stderr_path)?;
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
            .arg("serve-native")
            .arg("--config")
            .arg(path)
            .env_remove("QIDX_NATIVE_INTENTIONALLY_MISSING_API_KEY_70491")
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_file))
            .spawn()?,
        None,
    );
    let deadline = Instant::now()
        .checked_add(NATIVE_STARTUP_LIMIT)
        .ok_or("native config refusal deadline overflow")?;
    let status = loop {
        if let Some(status) = daemon.0.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err(format!("invalid native config did not refuse: {expected}").into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    require(!status.success(), "invalid native config was accepted")?;
    let stderr = String::from_utf8(stderr_prefix(&stderr_path)?)?;
    require(
        stderr.contains(expected),
        format!("native config refused for wrong reason: expected {expected:?}, got {stderr}"),
    )?;
    require(
        !root.exists() && !socket.exists(),
        "invalid native config created a state root or socket",
    )
}

fn native_object_file(root: &Path, reference: ObjectRef) -> TestResult<std::path::PathBuf> {
    let bytes = (reference.0).0;
    let mut name = String::with_capacity(64);
    for byte in bytes {
        std::fmt::Write::write_fmt(&mut name, format_args!("{byte:02x}"))?;
    }
    Ok(root
        .join("native-objects")
        .join("objects")
        .join(format!(
            "{:02x}",
            bytes.first().copied().unwrap_or_default()
        ))
        .join(name))
}

#[test]
fn product_daemon_serves_native_publish_query_and_reopen() -> TestResult {
    check_product_daemon_publish_query_and_reopen(false)
}

#[test]
fn product_daemon_refuses_ambiguous_commands_before_state_access() -> TestResult {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("uncreated-source");
    let destination = temp.path().join("uncreated-destination");
    for commands in [
        ["backup-state", "restore-state"],
        ["restore-state", "backup-state"],
        ["serve", "backup-state"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
            .args(commands)
            .arg("--source")
            .arg(&source)
            .arg("--destination")
            .arg(&destination)
            .output()?;
        require_equal(&output.status.code(), &Some(70), "ambiguous command exit")?;
        let stderr = String::from_utf8(output.stderr)?;
        require(
            stderr.contains("only one subcommand may be specified"),
            format!("wrong refusal phase: {stderr}"),
        )?;
        require(
            output.stdout.is_empty(),
            "ambiguous command emitted an offline receipt",
        )?;
        require(
            !source.exists() && !destination.exists(),
            "ambiguous command accessed state paths",
        )?;
    }
    Ok(())
}

#[test]
fn product_daemon_applies_retention_config_preserves_current_query_and_reopens() -> TestResult {
    check_product_daemon_publish_query_and_reopen(true)
}

#[test]
fn product_daemon_owns_openai_semantic_materialization_query_scope_and_reopen() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-semantic.json");
    let stub = EmbeddingStub::start()?;
    let base_url = stub.base_url();
    let mut configuration = config_with_openai_semantic(&root, &socket)?;
    configure_semantic_endpoint(&mut configuration, &base_url)?;
    configure_owned_semantic(&mut configuration)?;
    configure_effort(&mut configuration)?;
    // Clear dwell outlives any single signal sample; short repeated queries
    // must accumulate only contiguous observations from the service clock.
    set_fixture_json(
        &mut configuration,
        "/effort/clear_dwell_ms",
        serde_json::json!(1200),
    )?;
    set_fixture_json(
        &mut configuration,
        "/effort/minimum_residence_ms",
        serde_json::json!(1200),
    )?;
    fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
    let semantic = openai_semantic_spec(&base_url)?;
    let daemon = Daemon::start_with_api_key(&config_path, &socket, Some("test-key"))?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("native Semantic workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_spec(), semantic.clone()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let sources = client.stage_sources(
        ingress,
        vec![
            SourceInput {
                path: "semantic.rs".into(),
                bytes: b"needle needle".to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            },
            SourceInput {
                path: "semantic-second.rs".into(),
                bytes: b"needle needle".to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            },
        ],
        &budget(),
    )?;
    let [source, second_source] = sources.as_slice() else {
        return Err("native Semantic source staging failed".into());
    };
    let before = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owner = OwnerKey {
        producer: key.producer,
        stable_id: StableOwnerId([1; 16]),
    };
    let second_owner = OwnerKey {
        producer: key.producer,
        stable_id: StableOwnerId([2; 16]),
    };
    let owners = client.stage_mutations(
        ingress,
        [
            OwnerMutation {
                owner,
                expected: None,
                next: Some(*source),
            },
            OwnerMutation {
                owner: second_owner,
                expected: None,
                next: Some(*second_source),
            },
        ],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: before.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let source_view = published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let query = SemanticQueryRequest {
        query_text: "needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 1,
    };
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(15))
        .ok_or("native Semantic materialization deadline overflow")?;
    let mut last_pending = String::from("no post-source view observed");
    let (materialized, ranked) = loop {
        let latest = client.read_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?;
        if latest.identity.sequence > source_view.sequence {
            match client.search_semantic(
                ViewSelection::Exact(latest.identity),
                semantic.instance,
                query.clone(),
                &budget(),
            ) {
                Ok(ranked)
                    if ranked.coverage.status == CoverageStatus::Complete
                        && ranked.hits.len() == 1 =>
                {
                    break (latest.identity, ranked);
                }
                Ok(ranked) => {
                    last_pending = format!(
                        "view={:?} coverage={:?} hits={} scanned={}",
                        latest.identity,
                        ranked.coverage.status,
                        ranked.hits.len(),
                        ranked.scanned_owners
                    );
                }
                Err(error) => {
                    return Err(format!(
                        "native Semantic query failed during materialization: {error:?}; last={last_pending}"
                    )
                    .into());
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "product owned pump did not materialize Semantic result: {last_pending}"
            )
            .into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    require_equal(&ranked.view, &materialized, "materialized Semantic view")?;
    require_equal(&ranked.instance, &semantic.instance, "Semantic instance")?;
    let hit = ranked.hits.first().ok_or("Semantic hit missing")?;
    require_equal(&hit.owner, &owner, "Semantic owner identity")?;
    require_equal(
        &hit.vector_id,
        &b"whole-file".to_vec(),
        "Semantic vector identity",
    )?;
    require_equal(&hit.score.get(), &1.0, "Semantic cosine")?;
    require_equal(&ranked.scanned_owners, &2, "Semantic scanned owners")?;
    require_equal(&ranked.skipped_stale, &0, "Semantic stale owners")?;
    require(
        ranked.exhaustive,
        "Semantic fixture must have exhaustive recall",
    )?;
    let calls = stub.observed_inputs()?;
    require(
        calls.iter().any(|input| input == "needle needle")
            && calls.iter().any(|input| input == "needle"),
        format!("source/query did not use the configured provider: {calls:?}"),
    )?;
    let calls_before_tokenless = calls.len();
    let mut tokenless = query.clone();
    tokenless.query_text = "...".into();
    require(
        matches!(
            client.search_semantic(
                ViewSelection::Exact(materialized),
                semantic.instance,
                tokenless,
                &budget(),
            ),
            Err(NativeUdsError::Refused(_))
        ),
        "tokenless Semantic query reached a successful provider result",
    )?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &calls_before_tokenless,
        "tokenless Semantic query provider calls",
    )?;

    let text_query = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 1,
        cursor: None,
    };
    let mut scoped_query = query.clone();
    scoped_query.lexical_scope = Some(text_query.clone());
    let scoped = client.search_semantic(
        ViewSelection::Exact(materialized),
        semantic.instance,
        scoped_query.clone(),
        &budget(),
    )?;
    require_equal(&scoped.view, &materialized, "lexical scope selected view")?;
    require_equal(&scoped.hits.len(), &1, "lexical scope hit count")?;
    require_equal(
        &scoped.hits.first().ok_or("scoped hit")?.owner,
        &owner,
        "lexical scope owner",
    )?;
    require(
        scoped.exhaustive,
        "finite lexical universe was not exhausted",
    )?;
    // `count:1` caps returned Text rows, while both durable owners remain
    // matches. Semantic must validate against the planned result limit.
    let mut bounded_text = text_query.clone();
    bounded_text.query_text = "needle count:1".into();
    bounded_text.top_k = 2;
    let bounded_fast = client.search_text(
        ViewSelection::Exact(materialized),
        text_spec().instance,
        bounded_text.clone(),
        &budget(),
    )?;
    require_equal(&bounded_fast.matched, &2, "bounded Text match universe")?;
    require_equal(&bounded_fast.hits.len(), &1, "bounded Text returned rows")?;
    let mut bounded_semantic = query.clone();
    bounded_semantic.top_k = 2;
    bounded_semantic.lexical_scope = Some(bounded_text);
    let bounded = client.search_semantic(
        ViewSelection::Exact(materialized),
        semantic.instance,
        bounded_semantic,
        &budget(),
    )?;
    require_equal(&bounded.hits.len(), &1, "bounded Semantic hit count")?;
    require_equal(
        &bounded.scanned_owners,
        &1,
        "bounded Semantic scanned owners",
    )?;
    require_equal(
        &bounded.hits.first().ok_or("bounded scoped hit")?.owner,
        &owner,
        "bounded Semantic owner",
    )?;
    require(
        bounded.exhaustive,
        "bounded lexical universe was not exhausted",
    )?;
    let calls_before_empty_scope = stub.observed_inputs()?.len();
    let mut empty_scope = scoped_query.clone();
    empty_scope
        .lexical_scope
        .as_mut()
        .ok_or("scope missing")?
        .query_text = "absentscope".into();
    let empty = client.search_semantic(
        ViewSelection::Exact(materialized),
        semantic.instance,
        empty_scope,
        &budget(),
    )?;
    require(
        empty.hits.is_empty() && empty.exhaustive,
        "empty lexical universe widened",
    )?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &calls_before_empty_scope,
        "empty scope provider calls",
    )?;
    let mut tokenless_scope = scoped_query.clone();
    tokenless_scope.query_text = "...".into();
    tokenless_scope
        .lexical_scope
        .as_mut()
        .ok_or("scope missing")?
        .query_text = "absentscope".into();
    require(
        matches!(
            client.search_semantic(
                ViewSelection::Exact(materialized),
                semantic.instance,
                tokenless_scope,
                &budget(),
            ),
            Err(NativeUdsError::Refused(_))
        ),
        "empty scope bypassed semantic input admission",
    )?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &calls_before_empty_scope,
        "tokenless empty scope provider calls",
    )?;
    let mut mismatched_scope = scoped_query.clone();
    mismatched_scope.constraints.repo_relative_path_exact = Some(
        quanta_index_contract::ExactRepoRelativePathV1::new("different.rs")?,
    );
    require(
        matches!(
            client.search_semantic(
                ViewSelection::Exact(materialized),
                semantic.instance,
                mismatched_scope,
                &budget(),
            ),
            Err(NativeUdsError::Refused(_))
        ),
        "mismatched lexical constraints succeeded",
    )?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &calls_before_empty_scope,
        "mismatched scope provider calls",
    )?;
    let mut stages = Vec::new();
    let deep_done = client.query_effort_stream(
        NativeEffortStreamQuery {
            selection: ViewSelection::Exact(materialized),
            text_instance: text_spec().instance,
            semantic_instance: semantic.instance,
            text_query: text_query.clone(),
            semantic_query: query.clone(),
            preference: NativeEffortPreference::Deep,
            overlay: None,
        },
        &budget(),
        &mut |stage| {
            stages.push(match stage {
                NativeEffortStreamStage::Fast(_) => 1_u8,
                NativeEffortStreamStage::Deep(_) => 2_u8,
                NativeEffortStreamStage::OverlayFast(_)
                | NativeEffortStreamStage::OverlayDeep(_) => {
                    return Err(NativeUdsError::Protocol(
                        "durable product query returned overlay stage".into(),
                    ));
                }
            });
            Ok(())
        },
    )?;
    require_equal(&stages, &vec![1, 2], "product explicit Deep stream stages")?;
    require(
        deep_done.deep_delivered,
        "product explicit Deep was not delivered",
    )?;

    let calls_before_auto = stub.observed_inputs()?.len();
    stages.clear();
    let auto_done = client.query_effort_stream(
        NativeEffortStreamQuery {
            selection: ViewSelection::Exact(materialized),
            text_instance: text_spec().instance,
            semantic_instance: semantic.instance,
            text_query: text_query.clone(),
            semantic_query: query.clone(),
            preference: NativeEffortPreference::Auto,
            overlay: None,
        },
        &budget(),
        &mut |stage| {
            stages.push(match stage {
                NativeEffortStreamStage::Fast(_) => 1_u8,
                NativeEffortStreamStage::Deep(_) => 2_u8,
                NativeEffortStreamStage::OverlayFast(_)
                | NativeEffortStreamStage::OverlayDeep(_) => {
                    return Err(NativeUdsError::Protocol(
                        "durable product query returned overlay stage".into(),
                    ));
                }
            });
            Ok(())
        },
    )?;
    require_equal(&stages, &vec![1], "product Auto without IDE activity")?;
    require(
        !auto_done.deep_delivered,
        "Auto inferred idle time without an activity source",
    )?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &calls_before_auto,
        "Auto without IDE activity provider calls",
    )?;
    let publisher = client.begin_interaction_publisher(Duration::from_secs(10), &budget())?;
    require(
        matches!(client.begin_interaction_publisher(Duration::from_secs(10), &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict),
        "a second foreground publisher replaced the live session",
    )?;
    client.report_interaction_activity(publisher, 1, &budget())?;
    require(
        matches!(client.report_interaction_activity(publisher, 1, &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict),
        "replayed activity sequence was accepted",
    )?;
    client.renew_interaction_publisher(publisher, 2, &budget())?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &calls_before_auto,
        "foreground telemetry performed provider IO",
    )?;
    let auto_deadline = Instant::now()
        .checked_add(Duration::from_secs(8))
        .ok_or("product Auto refinement deadline overflow")?;
    loop {
        stages.clear();
        let completed = client.query_effort_stream(
            NativeEffortStreamQuery {
                selection: ViewSelection::Exact(materialized),
                text_instance: text_spec().instance,
                semantic_instance: semantic.instance,
                text_query: text_query.clone(),
                semantic_query: query.clone(),
                preference: NativeEffortPreference::Auto,
                overlay: None,
            },
            &budget(),
            &mut |stage| {
                stages.push(match stage {
                    NativeEffortStreamStage::Fast(_) => 1_u8,
                    NativeEffortStreamStage::Deep(_) => 2_u8,
                    NativeEffortStreamStage::OverlayFast(_)
                    | NativeEffortStreamStage::OverlayDeep(_) => {
                        return Err(NativeUdsError::Protocol(
                            "durable product query returned overlay stage".into(),
                        ));
                    }
                });
                Ok(())
            },
        )?;
        require_equal(&completed.view, &materialized, "product Auto pinned view")?;
        if completed.deep_delivered {
            require_equal(&stages, &vec![1, 2], "product Auto Fast then Deep")?;
            break;
        }
        require_equal(&stages, &vec![1], "product Auto before clear dwell")?;
        if Instant::now() >= auto_deadline {
            return Err(format!(
                "product Auto never refined across short requests: {:?}",
                completed.reason
            )
            .into());
        }
        thread::sleep(Duration::from_millis(30));
    }
    let expected_auto_calls = calls_before_auto
        .checked_add(1)
        .ok_or("product Auto call counter overflow")?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &expected_auto_calls,
        "product Auto performed exactly one optional provider request",
    )?;
    let calls_after_auto = stub.observed_inputs()?.len();
    client.end_interaction_publisher(publisher, 3, &budget())?;
    stages.clear();
    let ended = client.query_effort_stream(
        NativeEffortStreamQuery {
            selection: ViewSelection::Exact(materialized),
            text_instance: text_spec().instance,
            semantic_instance: semantic.instance,
            text_query,
            semantic_query: query.clone(),
            preference: NativeEffortPreference::Auto,
            overlay: None,
        },
        &budget(),
        &mut |stage| {
            stages.push(match stage {
                NativeEffortStreamStage::Fast(_) => 1_u8,
                NativeEffortStreamStage::Deep(_) => 2_u8,
                NativeEffortStreamStage::OverlayFast(_)
                | NativeEffortStreamStage::OverlayDeep(_) => {
                    return Err(NativeUdsError::Protocol(
                        "durable product query returned overlay stage".into(),
                    ));
                }
            });
            Ok(())
        },
    )?;
    require(
        !ended.deep_delivered,
        "ended foreground session retained Auto Deep",
    )?;
    require_equal(&stages, &vec![1], "product Auto after foreground End")?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &calls_after_auto,
        "ended Auto provider calls",
    )?;
    require(
        matches!(client.report_interaction_activity(publisher, 4, &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict),
        "ended publisher activity revived the session",
    )?;
    let expiring = client.begin_interaction_publisher(Duration::from_millis(20), &budget())?;
    require(
        expiring != publisher,
        "server reused a foreground publisher identity",
    )?;
    thread::sleep(Duration::from_millis(100));
    require(
        matches!(client.renew_interaction_publisher(expiring, 1, &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict),
        "expired foreground publisher renewed its idle observation",
    )?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &calls_after_auto,
        "foreground telemetry performed provider IO",
    )?;

    let outside = WorkspaceKey {
        slot: SlotId([9; 16]),
        ..workspace()
    };
    let NativeIndexResponse::Created(outside_genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: outside,
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("outside workspace creation failed".into());
    };
    let outside_operation = OperationKey {
        workspace: outside,
        ..operation(10)
    };
    let outside_ingress = client.begin_input(outside_operation, &budget())?;
    let outside_profile = client.stage_profile(
        outside_ingress,
        IndexProfile {
            instances: vec![text_spec(), semantic.clone()],
        },
        &budget(),
    )?;
    let calls_before = stub.observed_inputs()?.len();
    let outside_outcome = client.submit(
        UpdateRequest {
            operation: outside_operation,
            expected_view: outside_genesis,
            kind: UpdateKind::Reconfigure {
                profile: outside_profile,
            },
        },
        &budget(),
    )?;
    require(
        matches!(
            outside_outcome,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput))
        ),
        "unauthorized Semantic scope was published",
    )?;
    let outside_latest = client.read_view(
        ViewSelection::Latest {
            workspace: outside,
            epoch: 1,
        },
        &budget(),
    )?;
    require_equal(
        &outside_latest.identity,
        &outside_genesis,
        "unauthorized Semantic configuration changed the published view",
    )?;
    require_equal(
        &stub.observed_inputs()?.len(),
        &calls_before,
        "unauthorized scope provider calls",
    )?;
    daemon.stop()?;

    let reopened = Daemon::start_with_api_key(&config_path, &socket, Some("test-key"))?;
    let restored = NativeUdsClient::new(socket);
    require(
        matches!(restored.report_interaction_activity(publisher, 5, &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict),
        "reopened process accepted an old foreground session",
    )?;
    let after = restored.search_semantic(
        ViewSelection::Exact(materialized),
        semantic.instance,
        query,
        &budget(),
    )?;
    require_equal(&after.hits.len(), &1, "reopened Semantic hits")?;
    let reopened_hit = after.hits.first().ok_or("reopened Semantic hit missing")?;
    require_equal(&reopened_hit.owner, &owner, "reopened Semantic owner")?;
    require_equal(
        &reopened_hit.vector_id,
        &b"whole-file".to_vec(),
        "reopened Semantic vector identity",
    )?;
    let scoped_after = restored.search_semantic(
        ViewSelection::Exact(materialized),
        semantic.instance,
        scoped_query,
        &budget(),
    )?;
    require_equal(
        &scoped_after.view,
        &materialized,
        "reopened lexical scope view",
    )?;
    require_equal(&scoped_after.hits.len(), &1, "reopened lexical scope count")?;
    require_equal(
        &scoped_after
            .hits
            .first()
            .ok_or("reopened scoped hit")?
            .owner,
        &owner,
        "reopened lexical scope owner",
    )?;
    require(
        scoped_after.exhaustive,
        "reopened lexical universe not exhausted",
    )?;
    reopened.stop()?;
    stub.finish()?;
    Ok(())
}

fn check_product_daemon_publish_query_and_reopen(retention: bool) -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-config.json");
    let mut configuration = if retention {
        config_with_reclaim(&root, &socket)?
    } else {
        config(&root, &socket)
    };
    if retention {
        let object = configuration
            .as_object_mut()
            .ok_or("invalid product config fixture")?;
        let _previous = object.insert("retention".into(), serde_json::json!({
            "scopes": [{ "workspace": workspace(), "epoch": 1 }],
            "sequence_window": 2, "page_limit": 1,
            "interval_ms": 20, "round_timeout_ms": 2000, "head_probe_every": 2,
            "work_limits": {"reads": 1_000_000, "writes": 1_000_000, "bytes": 536_870_912, "mutations": 1_000_000}
        }));
    }
    fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
    let daemon = Daemon::start(&config_path, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("native workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_spec()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let sources = client.stage_sources(
        ingress,
        vec![SourceInput {
            path: "a.rs".into(),
            bytes: b"needle needle".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &budget(),
    )?;
    let [source] = sources.as_slice() else {
        return Err("one source expected".into());
    };
    let before = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: OwnerKey {
                producer: operation(2).producer,
                stable_id: StableOwnerId([1; 16]),
            },
            expected: None,
            next: Some(*source),
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: before.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(2).producer,
        stream: operation(2).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let view = published(&client.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let query = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 2,
        cursor: None,
    };
    let ranked = client.search_text(
        ViewSelection::Exact(view),
        text_spec().instance,
        query.clone(),
        &budget(),
    )?;
    require_equal(&ranked.view, &view, "native query view")?;
    require_equal(&ranked.matched, &1, "native query match count")?;
    let [hit] = ranked.hits.as_slice() else {
        return Err("native text query must return exactly one hit".into());
    };
    require_equal(&hit.path.as_str(), &"a.rs", "native query hit path")?;
    require_equal(&hit.source, source, "native query hit source")?;
    let workspace_id = format!(
        "{}{}{}{}",
        "01".repeat(16),
        "02".repeat(16),
        "03".repeat(16),
        "04".repeat(16)
    );
    let cli_view = native_cli(vec![
        "native".into(),
        "read-view".into(),
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "--workspace".into(),
        workspace_id.clone(),
        "--epoch".into(),
        "1".into(),
        "--output".into(),
        "json".into(),
    ])?;
    let cli_view: quanta_index_contract::index::CatalogView = serde_json::from_slice(&cli_view)?;
    require_equal(&cli_view.identity, &view, "CLI read-view identity")?;
    let cli_ranked = native_cli(vec![
        "native".into(),
        "search-text".into(),
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "--workspace".into(),
        workspace_id,
        "--epoch".into(),
        "1".into(),
        "--instance".into(),
        "47".repeat(16),
        "--query-text".into(),
        "needle".into(),
        "--top-k".into(),
        "2".into(),
        "--output".into(),
        "json".into(),
    ])?;
    let cli_ranked: NativeTextResult = serde_json::from_slice(&cli_ranked)?;
    require_equal(&cli_ranked, &ranked, "CLI native search result")?;
    if retention {
        // Observe the live daemon without opening a second Catalog writer or
        // running constructor recovery/maintenance against its owned root.
        let observer = rusqlite::Connection::open_with_flags(
            quanta_index_catalog::catalog_dir(&root).join(quanta_index_catalog::CATALOG_FILE_NAME),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        observer.busy_timeout(Duration::from_secs(2))?;
        let workspace_key = serde_json::to_vec(&workspace())?;
        let expected_floor = i64::try_from(view.sequence.saturating_sub(1))?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or("product retention deadline overflow")?;
        loop {
            let snapshot = observer.unchecked_transaction()?;
            let floor: i64 = snapshot.query_row(
                "SELECT COALESCE((SELECT retain_from_sequence FROM native_exact_history_floor_v1 WHERE workspace=?1 AND epoch=1),0)",
                rusqlite::params![&workspace_key], |row| row.get(0),
            )?;
            let pending: i64 = snapshot.query_row(
                "SELECT count(*) FROM native_terminal_custody_v1 WHERE workspace=?1 AND epoch=1",
                rusqlite::params![&workspace_key],
                |row| row.get(0),
            )?;
            snapshot.commit()?;
            if floor == expected_floor && pending == 0 {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "product retention did not settle: floor={floor}, expected={expected_floor}, pending_custody={pending}"
                ).into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        observer.close().map_err(|(_connection, error)| error)?;
        require(
            matches!(
                client.read_view(ViewSelection::Exact(genesis), &budget()),
                Err(NativeUdsError::Refused(error))
                    if error.code == NativeIndexErrorCode::SnapshotUnavailable
            ),
            "product retained new Exact admission below its automatic floor",
        )?;
    }
    daemon.stop()?;

    let reopened = Daemon::start(&config_path, &socket)?;
    let restored = NativeUdsClient::new(socket);
    let reopened_view = restored.read_view(ViewSelection::Exact(view), &budget())?;
    require_equal(&reopened_view.identity, &view, "reopened view identity")?;
    let after = restored.search_text(
        ViewSelection::Exact(view),
        text_spec().instance,
        query,
        &budget(),
    )?;
    require_equal(&after, &ranked, "reopened native search result")?;
    reopened.stop()?;
    Ok(())
}

#[test]
fn product_daemon_automatically_reclaims_discarded_input_and_recovers_retained_ingress()
-> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-reclaim.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&config_with_reclaim(&root, &socket)?)?,
    )?;
    let daemon = Daemon::start(&config_path, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    require(
        matches!(created, NativeIndexResponse::Created(_)),
        "native genesis failed",
    )?;
    let removed_body = b"process discarded source";
    let removed_ingress = client.begin_input(operation(801), &budget())?;
    let removed_record = client.stage_source(
        removed_ingress,
        "removed.rs".into(),
        removed_body,
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let kept_body = b"process retained source";
    let kept_ingress = client.begin_input(operation(802), &budget())?;
    let kept_record = client.stage_source(
        kept_ingress,
        "kept.rs".into(),
        kept_body,
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let identity = FileObjectStore::open(temp.path().join("identity-objects"))?;
    let limits = WorkLimits {
        reads: 1_000_000,
        writes: 1_000_000,
        bytes: 536_870_912,
        mutations: 1_000_000,
    };
    let removed_content = identity
        .identify(
            removed_body,
            &[],
            &mut WorkBudget::with_request(limits, budget()),
        )?
        .reference;
    let kept_content = identity
        .identify(
            kept_body,
            &[],
            &mut WorkBudget::with_request(limits, budget()),
        )?
        .reference;
    let removed_record_file = native_object_file(&root, removed_record)?;
    let removed_content_file = native_object_file(&root, removed_content)?;
    let kept_record_file = native_object_file(&root, kept_record)?;
    let kept_content_file = native_object_file(&root, kept_content)?;
    for path in [
        &removed_record_file,
        &removed_content_file,
        &kept_record_file,
        &kept_content_file,
    ] {
        require(
            path.is_file(),
            format!("staged object absent: {}", path.display()),
        )?;
    }
    client.discard_input(removed_ingress, &budget())?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(8))
        .ok_or("deadline overflow")?;
    while (removed_record_file.exists() || removed_content_file.exists())
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(20));
    }
    require(
        !removed_record_file.exists(),
        "process GC retained discarded record",
    )?;
    require(
        !removed_content_file.exists(),
        "process GC retained discarded body",
    )?;
    require(
        kept_record_file.is_file(),
        "process GC deleted retained record",
    )?;
    require(
        kept_content_file.is_file(),
        "process GC deleted retained body",
    )?;
    daemon.stop()?;

    let reopened = Daemon::start(&config_path, &socket)?;
    let restored = NativeUdsClient::new(socket);
    require(kept_record_file.is_file(), "reopen lost retained record")?;
    require(kept_content_file.is_file(), "reopen lost retained body")?;
    restored.discard_input(kept_ingress, &budget())?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(8))
        .ok_or("deadline overflow")?;
    while (kept_record_file.exists() || kept_content_file.exists()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    require(
        !kept_record_file.exists(),
        "reopen GC retained discarded record",
    )?;
    require(
        !kept_content_file.exists(),
        "reopen GC retained discarded body",
    )?;
    reopened.stop()
}

#[test]
fn native_reclaim_config_refuses_invalid_policy_and_capacity_before_state_creation() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    for case in [
        "zero_interval",
        "too_many_steps",
        "low_work",
        "missing_control_floor",
    ] {
        let root = temp.path().join(format!("state-{case}"));
        let socket = temp.path().join(format!("socket-{case}.sock"));
        let mut value = config_with_reclaim(&root, &socket)?;
        match case {
            "zero_interval" => {
                set_fixture_json(&mut value, "/maintenance/interval_ms", serde_json::json!(0))?;
            }
            "too_many_steps" => set_fixture_json(
                &mut value,
                "/maintenance/max_steps_per_round",
                serde_json::json!(9),
            )?,
            "low_work" => set_fixture_json(
                &mut value,
                "/maintenance/work_limits/bytes",
                serde_json::json!(1),
            )?,
            "missing_control_floor" => {
                set_fixture_json(
                    &mut value,
                    "/resources/execution/capacity/cpu_units",
                    serde_json::json!(4),
                )?;
                set_fixture_json(
                    &mut value,
                    "/resources/execution/capacity/io_bytes",
                    serde_json::json!(268_435_456_u64),
                )?;
                set_fixture_json(
                    &mut value,
                    "/resources/execution/capacity/memory_bytes",
                    serde_json::json!(268_435_456_u64),
                )?;
            }
            _ => return Err("unknown maintenance config refusal case".into()),
        }
        let path = temp.path().join(format!("config-{case}.json"));
        fs::write(&path, serde_json::to_vec(&value)?)?;
        let stderr_path = temp.path().join(format!("stderr-{case}.log"));
        let stderr_file = fs::File::create(&stderr_path)?;
        let mut daemon = Daemon(
            Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
                .arg("serve-native")
                .arg("--config")
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::from(stderr_file))
                .spawn()?,
            None,
        );
        let deadline = Instant::now()
            .checked_add(NATIVE_STARTUP_LIMIT)
            .ok_or("native maintenance refusal deadline overflow")?;
        let status = loop {
            if let Some(status) = daemon.0.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let stderr_bytes = stderr_prefix(&stderr_path)?;
                return Err(format!(
                    "{case} did not refuse native maintenance config within thirty seconds: {}",
                    String::from_utf8_lossy(&stderr_bytes)
                )
                .into());
            }
            thread::sleep(Duration::from_millis(10));
        };
        require(!status.success(), format!("{case} accepted"))?;
        let expected = match case {
            "zero_interval" => "native reclaim interval must be 10ms..=60s",
            "too_many_steps" => "native reclaim round must contain 1..=8 steps",
            "low_work" => "native reclaim WorkLimits are below the bounded round allowance",
            "missing_control_floor" => "native protected floors exceed service capacity",
            _ => return Err("unknown maintenance config refusal case".into()),
        };
        let stderr_bytes = stderr_prefix(&stderr_path)?;
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        require(
            stderr.contains(expected),
            format!("{case} failed for the wrong reason: {stderr}"),
        )?;
        require(!root.exists(), format!("{case} created state root"))?;
        require(!socket.exists(), format!("{case} created socket"))?;
    }
    Ok(())
}

#[test]
fn native_config_refuses_semantic_without_a_configured_provider() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let mut value = config_with_openai_semantic(&root, &socket)?;
    let semantic = value
        .pointer_mut("/semantic")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("native Semantic fixture must be a JSON object")?;
    let _removed = semantic
        .remove("provider")
        .ok_or("native Semantic provider fixture is absent")?;
    let config_path = temp.path().join("missing-semantic-provider.json");
    fs::write(&config_path, serde_json::to_vec(&value)?)?;
    expect_native_config_refusal(&root, &socket, &config_path, "missing field `provider`")
}

#[test]
fn native_semantic_provider_contract_refuses_before_state_creation() -> TestResult {
    for case in [
        "source_consent_missing",
        "endpoint_mismatch",
        "spoofed_loopback_host",
        "reader_without_provider_reservation",
        "unfunded_provider_fanout",
        "unfunded_response_bound",
        "unfunded_maximum_input",
        "missing_retained_headroom",
        "invalid_api_key_variable",
        "missing_api_key_variable",
        "unadmitted_local_model",
        "empty_authorized_scopes",
        "duplicate_authorized_scope",
        "cross_tenant_egress_scope",
        "mismatched_egress_tenant",
        "owned_scope_outside_egress_grant",
    ] {
        let temp = tempfile::tempdir()?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        let root = temp.path().join("state");
        let socket = temp.path().join("native.sock");
        let mut value = config_with_openai_semantic(&root, &socket)?;
        let expected = match case {
            "source_consent_missing" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/provider/egress/source_content_consent",
                    serde_json::json!(false),
                )?;
                "source content consent was not granted"
            }
            "endpoint_mismatch" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/provider/egress/endpoint",
                    serde_json::json!("https://other.invalid/v1/embeddings"),
                )?;
                "egress endpoint must match provider URL"
            }
            "spoofed_loopback_host" => {
                configure_semantic_endpoint(&mut value, "http://127.0.0.1:80@outside.invalid")?;
                "endpoint must be an HTTPS origin or explicit loopback HTTP origin"
            }
            "reader_without_provider_reservation" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/read/need/provider_units",
                    serde_json::json!(0),
                )?;
                "native semantic read policy exceeds bounded service limits"
            }
            "unfunded_provider_fanout" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/provider/concurrency",
                    serde_json::json!(2),
                )?;
                "owner execution requires one funded attempt slot"
            }
            "unfunded_response_bound" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/provider/max_response_bytes",
                    serde_json::json!(67_108_865_u64),
                )?;
                "response bound exceeds reader or Enrichment memory reservation"
            }
            "unfunded_maximum_input" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/max_source_bytes",
                    serde_json::json!(16_777_216_u64),
                )?;
                "maximum input envelope exceeds reader or Enrichment memory reservation"
            }
            "missing_retained_headroom" => {
                set_fixture_json(
                    &mut value,
                    "/resources/execution/capacity/memory_bytes",
                    serde_json::json!(268_435_456_u64),
                )?;
                "escaping buffers require headroom beyond protected execution floors"
            }
            "invalid_api_key_variable" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/provider/api_key_env",
                    serde_json::json!("bad-key-env"),
                )?;
                "invalid native OpenAI provider configuration"
            }
            "missing_api_key_variable" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/provider/api_key_env",
                    serde_json::json!("QIDX_NATIVE_INTENTIONALLY_MISSING_API_KEY_70491"),
                )?;
                "API key environment variable is missing"
            }
            "unadmitted_local_model" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/provider/kind",
                    serde_json::json!("potion_code"),
                )?;
                "unknown variant"
            }
            "empty_authorized_scopes" => {
                set_fixture_json(&mut value, "/semantic/scopes", serde_json::json!([]))?;
                "native Semantic needs 1..=1024 explicit scopes"
            }
            "duplicate_authorized_scope" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/scopes",
                    serde_json::json!([
                        {"workspace": workspace(), "epoch": 1},
                        {"workspace": workspace(), "epoch": 1}
                    ]),
                )?;
                "native Semantic scope is duplicated"
            }
            "cross_tenant_egress_scope" => {
                let outside = WorkspaceKey {
                    tenant: TenantId([9; 16]),
                    ..workspace()
                };
                set_fixture_json(
                    &mut value,
                    "/semantic/scopes",
                    serde_json::json!([
                        {"workspace": workspace(), "epoch": 1},
                        {"workspace": outside, "epoch": 1}
                    ]),
                )?;
                "egress grant cannot span workspace tenants"
            }
            "mismatched_egress_tenant" => {
                set_fixture_json(
                    &mut value,
                    "/semantic/provider/egress/tenant_id",
                    serde_json::json!(TenantId([9; 16])),
                )?;
                "native Semantic egress tenant differs from authorized scopes"
            }
            "owned_scope_outside_egress_grant" => {
                let outside = WorkspaceKey {
                    slot: SlotId([9; 16]),
                    ..workspace()
                };
                let owned_work = serde_json::json!({
                    "scopes": [{"workspace": outside, "epoch": 1}],
                    "batch_limit": 1, "max_in_flight": 1, "poll_interval_ms": 20,
                    "dispatch_limits": {"reads": 1_000_000, "writes": 1_000_000,
                                        "bytes": 536_870_912, "mutations": 1_000_000},
                    "max_dispatches": 2, "total_timeout_ms": 2000,
                    "lease_ms": 1000, "retry_backoff_ms": 20
                });
                let old = value
                    .as_object_mut()
                    .ok_or("native config fixture object")?
                    .insert("owned_work".into(), owned_work);
                require(old.is_none(), "owned work fixture field already exists")?;
                "owned work scope exceeds configured Semantic authorization"
            }
            _ => return Err("unknown native semantic refusal case".into()),
        };
        let path = temp.path().join(format!("native-{case}.json"));
        fs::write(&path, serde_json::to_vec(&value)?)?;
        expect_native_config_refusal(&root, &socket, &path, expected)?;
    }
    Ok(())
}

#[test]
fn native_effort_policy_and_signal_ttl_refuse_before_state_creation() -> TestResult {
    for case in [
        "inverted_pressure",
        "zero_idle",
        "zero_sample_ttl",
        "stale_worker_age",
    ] {
        let temp = tempfile::tempdir()?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        let root = temp.path().join("state");
        let socket = temp.path().join("native.sock");
        let mut value = config(&root, &socket);
        configure_effort(&mut value)?;
        let expected = match case {
            "inverted_pressure" => {
                set_fixture_json(
                    &mut value,
                    "/effort/thresholds/interactive_low_bps",
                    serde_json::json!(9000),
                )?;
                "invalid effort configuration"
            }
            "zero_idle" => {
                set_fixture_json(&mut value, "/effort/stable_idle_ms", serde_json::json!(0))?;
                "invalid effort configuration"
            }
            "zero_sample_ttl" => {
                set_fixture_json(&mut value, "/effort/sample_ttl_ms", serde_json::json!(0))?;
                "Auto signal age and TTL must be 1..=60000 ms"
            }
            "stale_worker_age" => {
                set_fixture_json(
                    &mut value,
                    "/effort/worker_lag_max_age_ms",
                    serde_json::json!(60_001),
                )?;
                "Auto signal age and TTL must be 1..=60000 ms"
            }
            _ => return Err("unknown native effort refusal case".into()),
        };
        let path = temp.path().join(format!("native-effort-{case}.json"));
        fs::write(&path, serde_json::to_vec(&value)?)?;
        expect_native_config_refusal(&root, &socket, &path, expected)?;
    }
    Ok(())
}

#[test]
fn native_retention_and_owned_work_config_refuse_before_creating_state() -> TestResult {
    for case in [
        "missing_reclaim",
        "zero_window",
        "duplicate_scope",
        "insufficient_retention_budget",
        "owned_work_without_semantic",
    ] {
        let temp = tempfile::tempdir()?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        let root = temp.path().join("state");
        let socket = temp.path().join("native.sock");
        let mut value = if case == "missing_reclaim" || case == "owned_work_without_semantic" {
            config(&root, &socket)
        } else {
            config_with_reclaim(&root, &socket)?
        };
        let scope = serde_json::json!({ "workspace": workspace(), "epoch": 1 });
        let mut retention = serde_json::json!({
            "scopes": [scope.clone()], "sequence_window": 2, "page_limit": 1,
            "interval_ms": 20, "round_timeout_ms": 2000, "head_probe_every": 2,
            "work_limits": { "reads": 1_000_000, "writes": 1_000_000, "bytes": 536_870_912, "mutations": 1_000_000 }
        });
        if case == "zero_window" {
            *retention
                .get_mut("sequence_window")
                .ok_or("missing retention window fixture")? = serde_json::json!(0);
        }
        if case == "duplicate_scope" {
            *retention
                .get_mut("scopes")
                .ok_or("missing retention scopes fixture")? =
                serde_json::json!([scope.clone(), scope.clone()]);
        }
        if case == "insufficient_retention_budget" {
            *retention
                .pointer_mut("/work_limits/bytes")
                .ok_or("missing retention byte allowance fixture")? = serde_json::json!(1);
        }
        let object = value.as_object_mut().ok_or("invalid config fixture")?;
        if case == "owned_work_without_semantic" {
            let _previous = object.insert("owned_work".into(), serde_json::json!({
                "scopes": [scope], "batch_limit": 1, "max_in_flight": 1, "poll_interval_ms": 20,
                "dispatch_limits": { "reads": 1_000_000, "writes": 1_000_000, "bytes": 536_870_912, "mutations": 1_000_000 },
                "max_dispatches": 2, "total_timeout_ms": 2000, "lease_ms": 1000, "retry_backoff_ms": 20
            }));
        } else {
            let _previous = object.insert("retention".into(), retention);
        }
        let path = temp.path().join("refused-native-config.json");
        fs::write(&path, serde_json::to_vec(&value)?)?;
        let stderr_path = temp.path().join("refused-native.stderr");
        let stderr_file = fs::File::create(&stderr_path)?;
        let mut daemon = Daemon(
            Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
                .arg("serve-native")
                .arg("--config")
                .arg(path)
                .stdout(Stdio::null())
                .stderr(Stdio::from(stderr_file))
                .spawn()?,
            None,
        );
        let deadline = Instant::now()
            .checked_add(NATIVE_STARTUP_LIMIT)
            .ok_or("retention config refusal deadline overflow")?;
        let status = loop {
            if let Some(status) = daemon.0.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(format!("{case} did not refuse before the startup deadline").into());
            }
            thread::sleep(Duration::from_millis(10));
        };
        require(!status.success(), format!("{case} accepted"))?;
        let expected = match case {
            "missing_reclaim" => "requires automatic reclaim",
            "zero_window" => "invalid native retention envelope",
            "duplicate_scope" => "duplicate native retention scope",
            "insufficient_retention_budget" => "below the bounded round allowance",
            "owned_work_without_semantic" => {
                "owned work requires a configured Semantic owner adapter"
            }
            _ => return Err("unknown native policy refusal case".into()),
        };
        let stderr = String::from_utf8(stderr_prefix(&stderr_path)?)?;
        require(
            stderr.contains(expected),
            format!("{case} failed for wrong reason: {stderr}"),
        )?;
        require(
            !root.exists() && !socket.exists(),
            format!("{case} created state or socket"),
        )?;
    }
    Ok(())
}

#[test]
fn invalid_native_config_has_no_state_root_or_socket_side_effects() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    for case in [
        "version",
        "relative_root",
        "duplicate_kind",
        "bm25",
        "unknown_field",
    ] {
        let root = temp.path().join(format!("state-{case}"));
        let socket = temp.path().join(format!("socket-{case}.sock"));
        let mut value = config(&root, &socket);
        match case {
            "version" => set_fixture_json(&mut value, "/version", serde_json::json!(2))?,
            "relative_root" => {
                set_fixture_json(
                    &mut value,
                    "/state_root",
                    serde_json::json!("relative-state"),
                )?;
            }
            "duplicate_kind" => {
                let kind = value
                    .pointer("/literal/kind")
                    .ok_or("missing literal kind in native config fixture")?
                    .clone();
                set_fixture_json(&mut value, "/file_map/kind", kind)?;
            }
            "bm25" => set_fixture_json(&mut value, "/text/bm25/k1", serde_json::json!(0.0))?,
            "unknown_field" => {
                let object = value
                    .as_object_mut()
                    .ok_or("native config fixture must be a JSON object")?;
                let previous =
                    object.insert("ignored_provider".into(), serde_json::json!("fallback"));
                require(
                    previous.is_none(),
                    "unknown field already exists in fixture",
                )?;
            }
            _ => return Err("unhandled native config case".into()),
        }
        let path = temp.path().join(format!("config-{case}.json"));
        fs::write(&path, serde_json::to_vec(&value)?)?;
        let output = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
            .arg("serve-native")
            .arg("--config")
            .arg(&path)
            .output()?;
        require(!output.status.success(), format!("{case} accepted"))?;
        require(!root.exists(), format!("{case} created state root"))?;
        require(!socket.exists(), format!("{case} created socket"))?;
    }
    Ok(())
}

#[test]
fn fifo_native_config_is_refused_without_blocking_or_creating_state() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let config_path = temp.path().join("config.fifo");
    let made = Command::new("mkfifo").arg(&config_path).status()?;
    if !made.success() {
        return Err("mkfifo fixture setup failed".into());
    }
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let stderr_path = temp.path().join("daemon.stderr");
    let stderr_file = fs::File::create(&stderr_path)?;
    let launch_started = Instant::now();
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
            .arg("serve-native")
            .arg("--config")
            .arg(&config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_file))
            .spawn()?,
        None,
    );
    let startup_deadline = launch_started
        .checked_add(NATIVE_STARTUP_LIMIT)
        .ok_or("FIFO startup deadline overflow")?;
    let mut preflight_deadline = None;
    loop {
        let now = Instant::now();
        let stderr_bytes = stderr_prefix(&stderr_path)?;
        let stderr = String::from_utf8_lossy(&stderr_bytes);
        if preflight_deadline.is_none() && now >= startup_deadline {
            let child_pid = daemon.0.id();
            let preview_len = stderr_bytes.len().min(512);
            let preview = String::from_utf8_lossy(
                stderr_bytes
                    .get(..preview_len)
                    .ok_or("stderr preview bounds changed")?,
            );
            return Err(format!(
                "FIFO config daemon did not reach config preflight within thirty seconds: binary={}, pid={child_pid}, stderr_bytes={}, stderr_preview={preview:?}",
                env!("CARGO_BIN_EXE_quanta-index-searchd"),
                stderr_bytes.len()
            )
            .into());
        }
        if preflight_deadline.is_none() && stderr.contains(NATIVE_CONFIG_PREFLIGHT_STAGE) {
            preflight_deadline = Some(
                now.checked_add(Duration::from_secs(3))
                    .ok_or("FIFO refusal deadline overflow")?,
            );
        }
        if let Some(status) = daemon.0.try_wait()? {
            require(!status.success(), "FIFO config unexpectedly accepted")?;
            require(
                preflight_deadline.is_some(),
                format!("FIFO refusal lacked config-preflight stage: {stderr}"),
            )?;
            require(
                stderr.contains("regular file"),
                format!("FIFO refusal lacked regular-file diagnosis: {stderr}"),
            )?;
            require(!root.exists(), "FIFO config created a state root")?;
            require(!socket.exists(), "FIFO config created a socket")?;
            return Ok(());
        }
        if preflight_deadline.is_some_and(|deadline| now >= deadline) {
            let child_pid = daemon.0.id();
            let preview_len = stderr_bytes.len().min(512);
            let preview = String::from_utf8_lossy(
                stderr_bytes
                    .get(..preview_len)
                    .ok_or("stderr preview bounds changed")?,
            );
            return Err(format!(
                "FIFO config blocked after config preflight for three seconds: binary={}, pid={child_pid}, stderr_bytes={}, stderr_preview={preview:?}",
                env!("CARGO_BIN_EXE_quanta-index-searchd"),
                stderr_bytes.len()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn occupied_native_socket_is_preserved_and_initialized_root_reopens() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.path().join("native.sock");
    let foreign = UnixListener::bind(&socket)?;
    let foreign_inode = fs::metadata(&socket)?.ino();
    let config_path = temp.path().join("native-config.json");
    fs::write(&config_path, serde_json::to_vec(&config(&root, &socket))?)?;
    let refused = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
        .arg("serve-native")
        .arg("--config")
        .arg(&config_path)
        .output()?;
    require(!refused.status.success(), "occupied socket was accepted")?;
    let stderr = String::from_utf8(refused.stderr)?;
    require(
        stderr.contains("SOCKET_IN_USE"),
        format!("occupied socket refusal lacked code: {stderr}"),
    )?;
    require_equal(
        &fs::metadata(&socket)?.ino(),
        &foreign_inode,
        "foreign socket inode",
    )?;
    let foreign_connection = UnixStream::connect(&socket)?;
    drop(foreign_connection);
    require(
        fs::read(root.join(".native-index-v18"))? == b"quanta-index-native-state-v18\n",
        "bind failure did not leave the current initialized format",
    )?;
    drop(foreign);

    let reopened = Daemon::start(&config_path, &socket)?;
    reopened.stop()?;
    Ok(())
}
