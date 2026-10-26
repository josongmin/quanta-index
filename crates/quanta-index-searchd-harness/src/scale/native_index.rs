//! Index-native scale adapter. This is the same scale rail and artifact owner,
//! with a separate, explicit producer/daemon backend from the legacy `E2eRuntime`.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::{
    fs::{OpenOptionsExt as _, PermissionsExt as _},
    net::UnixStream,
};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use quanta_index_contract::index::{
    CollectionId, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile, InputDomain,
    InstanceSpec, OperationKey, OwnerKey, OwnerMutation, ProducerId, PublicationGroupId, SchemaId,
    SlotId, SourceEvent, SourceInput, SourceTextAdmission, SourceTransition, StableOwnerId,
    StorageFormatId, StreamId, TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection,
    WorkspaceKey, WorktreeId,
};
use quanta_index_contract::native_protocol::{
    NativeIndexRequest, NativeIndexResponse, NativeReadSessionStatus,
};
use quanta_index_contract::{QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax};
use quanta_index_core::RequestBudgetV1;
use quanta_index_lexical::native::TextRecipe;
use quanta_index_sdk::native_uds::{MAX_NATIVE_STAGE_SOURCES_INPUTS, NativeUdsClient};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

#[path = "native_index/observation.rs"]
mod observation;
use observation::{NativeObservation, NativeSampler, ObservationMode};
#[path = "native_index/workload.rs"]
mod workload;
use workload::{NativeForegroundWorkload, selected_workload_from_env};
#[path = "native_index/trace.rs"]
mod trace;
use trace::NativeTrace;
#[path = "native_index/executable.rs"]
mod executable;
use executable::NativeExecutable;
#[path = "native_index/capture_artifacts.rs"]
mod capture_artifacts;
use capture_artifacts::ChildTraceArtifacts;
#[path = "native_index/publication.rs"]
mod publication;
use publication::{PublicationAudit, PublicationLabel};
#[path = "native_index/child_trace.rs"]
mod child_trace;
#[path = "native_index/daemon_diagnostic.rs"]
mod daemon_diagnostic;
#[path = "native_index/mixed_load.rs"]
mod mixed_load;
#[path = "native_index/retained_read.rs"]
mod retained_read;

use super::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily,
    ScaleSourceBinding, ScaleTier, config_digest, directory_bytes, file_query_token,
    generate_scoped_corpus, params_for, scoped_corpus_digest, stage_or_preserve,
    tier_manifest_json,
};

const DIMENSION: &str = "scale-index-native";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const START_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const FIRST_PATH: &str = "repo0/src/file_0.rs";
const DELTA_TOKEN: &str = "scalechangedneedle";
#[derive(Clone, Copy, Debug)]
pub struct NativePhase {
    pub wall_ms: f64,
    pub root_bytes_before: Option<u64>,
    pub root_bytes_after: Option<u64>,
    pub observer_ms: f64,
    pub completed: bool,
}

#[derive(Debug)]
pub(super) struct NativeFailurePrefix {
    pub(super) phases: Value,
}

impl std::fmt::Display for NativeFailurePrefix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("index-native measurement stopped after partial phases")
    }
}

impl std::error::Error for NativeFailurePrefix {}

fn with_prefix(
    phases: &BTreeMap<&'static str, NativePhase>,
    observation: &NativeObservation,
    churn: &NativeChurnProgress,
    publication: &PublicationAudit,
    workload: NativeForegroundWorkload,
    error: anyhow::Error,
) -> anyhow::Error {
    let mut prefix = phase_json(phases);
    if let Some(object) = prefix.as_object_mut() {
        let _previous = object.insert("foreground_workload".into(), workload.metadata());
        let _previous = object.insert("publication_outcomes".into(), publication.detail());
        if let Some(observed) = observation.detail() {
            let _previous = object.insert("native_resource_observation".into(), observed);
        }
        if churn.requested > 0 {
            let _previous = object.insert("churn_progress".into(), churn.detail());
        }
        if let Some(mixed) = error.downcast_ref::<mixed_load::MixedFailurePrefix>() {
            let _previous = object.insert("mixed_load".into(), mixed.detail.clone());
        }
    }
    error.context(NativeFailurePrefix { phases: prefix })
}

/// Observations are deliberately scoped to Index-native source intake and
/// text search. Parent RSS does not account for the child daemon.
#[derive(Debug)]
pub struct NativeIndexMeasurement {
    pub tier: ScaleTier,
    pub seed: u64,
    pub corpus_digest: String,
    pub daemon_sha256: String,
    pub native_config_sha256: String,
    pub foreground_workload: Value,
    pub publication_outcomes: Value,
    pub file_count: usize,
    pub source_bytes: u64,
    pub phases: BTreeMap<&'static str, NativePhase>,
    pub first_query_ms: f64,
    pub first_query_matched: u64,
    pub total_ms: f64,
    pub resource_observation: Option<Value>,
    pub churn_cycles: u32,
    pub churn: Option<Value>,
    pub child_trace_verified: bool,
    pub child_object_io: Option<Value>,
    pub mixed_load: Option<Value>,
    child_traces: Option<ChildTraceArtifacts>,
}

#[derive(Default)]
struct NativeChurnProgress {
    requested: u32,
    completed: u32,
    last_attempt: Option<u32>,
    ack_wall_ms: f64,
    query_wall_ms: f64,
    max_cycle_wall_ms: f64,
}

impl NativeChurnProgress {
    fn detail(&self) -> Value {
        json!({
            "requested_cycles":self.requested,
            "completed_cycles":self.completed,
            "last_attempt":self.last_attempt,
            "ack_wall_ms":self.ack_wall_ms,
            "query_wall_ms":self.query_wall_ms,
            "max_cycle_wall_ms":self.max_cycle_wall_ms,
            "scope":"finite single-owner CAS churn; not sustained steady-state or P95 qualification"
        })
    }
}

fn configured_churn_cycles() -> Result<u32> {
    match std::env::var_os("QUANTA_INDEX_NATIVE_CHURN_CYCLES") {
        None => Ok(0),
        Some(raw) => {
            let value = raw.to_str().context("native churn cycles must be UTF-8")?;
            let count = value
                .parse::<u32>()
                .context("native churn cycles must be an integer")?;
            anyhow::ensure!(
                (1..=256).contains(&count),
                "native churn cycles must be 1..=256"
            );
            Ok(count)
        }
    }
}

fn configured_mixed_load(
    tier: ScaleTier,
    seed: u64,
    workload: NativeForegroundWorkload,
) -> Result<bool> {
    let selected = std::env::var_os("QUANTA_INDEX_NATIVE_MIXED_LOAD_V1");
    let sampling = std::env::var_os("QUANTA_INDEX_NATIVE_RESOURCE_SAMPLING_V1");
    mixed_load_selection(
        selected.as_deref(),
        sampling.as_deref(),
        tier,
        seed,
        workload,
    )
}

fn mixed_load_selection(
    selected: Option<&std::ffi::OsStr>,
    sampling: Option<&std::ffi::OsStr>,
    tier: ScaleTier,
    seed: u64,
    workload: NativeForegroundWorkload,
) -> Result<bool> {
    let Some(raw) = selected else {
        return Ok(false);
    };
    anyhow::ensure!(raw == "1", "native mixed load must be exactly 1");
    anyhow::ensure!(
        tier == ScaleTier::Large
            && seed == 42
            && workload.mode == workload::NativeWorkloadMode::Explicit
            && workload.limits.reads == 67_108_864
            && workload.limits.writes == 67_108_864
            && workload.limits.bytes == 549_755_813_888
            && workload.limits.mutations == 67_108_864,
        "native mixed load requires the declared Large seed42 explicit work profile"
    );
    anyhow::ensure!(
        sampling.is_some_and(|raw| raw == "1"),
        "native mixed load requires resource sampling=1"
    );
    Ok(true)
}

struct NativeDaemon<'a> {
    child: Child,
    executable: &'a NativeExecutable,
}

impl<'a> NativeDaemon<'a> {
    fn start(
        binary: &'a NativeExecutable,
        config: &Path,
        socket: &Path,
        trace: Option<&Path>,
        trace_max_output_bytes: u64,
        stderr_path: &Path,
    ) -> Result<Self> {
        binary
            .verify()
            .context("native executable changed before startup")?;
        let stderr = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(stderr_path)
            .context("create exclusive native daemon stderr")?;
        let mut command = Command::new(binary.path());
        let _arguments = command.arg("serve-native").arg("--config").arg(config);
        if let Some(trace) = trace {
            let _trace_argument = command
                .arg("--diagnostic-trace")
                .arg(trace)
                .arg("--trace-max-output-bytes")
                .arg(trace_max_output_bytes.to_string());
        }
        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("start native daemon {}", binary.path().display()))?;
        let mut daemon = Self {
            child,
            executable: binary,
        };
        let deadline = Instant::now()
            .checked_add(START_TIMEOUT)
            .context("native startup deadline overflow")?;
        loop {
            if UnixStream::connect(socket).is_ok() {
                binary
                    .verify()
                    .context("native executable changed during startup")?;
                return Ok(daemon);
            }
            if let Some(status) = daemon.child.try_wait()? {
                return Err(daemon_diagnostic::attach(
                    anyhow::anyhow!("native daemon exited {status}"),
                    stderr_path,
                ));
            }
            if Instant::now() >= deadline {
                anyhow::bail!("native daemon did not bind UDS within {START_TIMEOUT:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(mut self, sampler: &mut Option<NativeSampler>) -> Result<()> {
        // Preserve the original Child as an unreaped PID until the observer
        // has joined, and finish observations before signaling shutdown.
        // A reused PID must never be reported as this daemon.
        if let Some(observer) = sampler.take() {
            observer.stop();
        }
        let status = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status()?;
        if !status.success() {
            anyhow::bail!("failed to signal native daemon");
        }
        let deadline = Instant::now()
            .checked_add(STOP_TIMEOUT)
            .context("native shutdown deadline overflow")?;
        loop {
            if let Some(status) = self.child.try_wait()? {
                if status.success() {
                    return self
                        .executable
                        .verify()
                        .context("native executable changed during execution");
                }
                anyhow::bail!("native daemon exited {status} while draining");
            }
            if Instant::now() >= deadline {
                anyhow::bail!("native daemon did not drain within {STOP_TIMEOUT:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for NativeDaemon<'_> {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
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

fn owner(index: usize) -> Result<OwnerKey> {
    Ok(OwnerKey {
        producer: operation(1).producer,
        stable_id: StableOwnerId(
            u128::try_from(
                index
                    .checked_add(1)
                    .context("native owner ordinal overflow")?,
            )?
            .to_be_bytes(),
        ),
    })
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
    RequestBudgetV1::for_duration(REQUEST_TIMEOUT)
}

fn native_config(root: &Path, socket: &Path, workload: NativeForegroundWorkload) -> Value {
    // Matches the configured product daemon's opt-in text profile. Explicit
    // limits are fixture inputs and must be emitted with the benchmark.
    let unit = json!({"cpu_units":1,"io_bytes":67_108_864,
        "memory_bytes":67_108_864,"provider_units":0});
    json!({
        "version":1,
        "state_root":root,
        "socket_path":socket,
        "work_limits":{"reads":workload.limits.reads,
            "writes":workload.limits.writes,
            "bytes":workload.limits.bytes,
            "mutations":workload.limits.mutations},
        "resources":{
            "physical":{
                "inventory":{"max_bytes":68_719_476_736_u64,"max_files":200_000,
                    "max_pending":4096,"max_entries":500_000,"max_depth":8,
                    "max_pending_frames":1024},
                "max_reader_handles":64,"max_tracked_entries":500_000,
                "max_process_roots":256,"global_max_sqlite_handles":512,
                "maintenance_reserved_bytes":524_288,"maintenance_reserved_files":8},
            "scratch_reserved_bytes":17_179_869_184_u64,
            "read_ticket_lease_ms":30_000,
            "execution":{"capacity":{"cpu_units":5,"io_bytes":335_544_320,
                "memory_bytes":402_653_184,"provider_units":0},
                "control":unit,"interactive":unit,"core":unit,"enrichment":unit}
        },
        "literal":{"kind":vec![8_u8;16],"schema":vec![10_u8;16],"format":vec![12_u8;16]},
        "file_map":{"kind":vec![18_u8;16],"schema":vec![10_u8;16],"format":vec![19_u8;16]},
        "text":{"descriptor":{"kind":vec![72_u8;16],"schema":vec![74_u8;16],
            "format":vec![75_u8;16]},"admission_policy":vec![31_u8;32],
            "bm25":{"k1":1.2,"b":0.75},"read_need":unit},
        "maintenance":{"interval_ms":20,"max_steps_per_round":1,"child_limit":32,
            "work_limits":{"reads":1_000_000,"writes":1_000_000,
                "bytes":536_870_912,"mutations":1_000_000},
            "round_timeout_ms":2_000,"head_probe_every":4}
    })
}

fn mixed_native_config(
    root: &Path,
    socket: &Path,
    workload: NativeForegroundWorkload,
) -> Result<Value> {
    let mut config = native_config(root, socket, workload);
    let object = config
        .as_object_mut()
        .context("fixed Native config is not an object")?;
    let _previous = object.insert(
        "retention".into(),
        json!({
            "scopes":[{"workspace":workspace(),"epoch":1}],
            "sequence_window":1,"page_limit":1,"interval_ms":20,
            "round_timeout_ms":2_000,"head_probe_every":2,
            "work_limits":{"reads":1_000_000,"writes":1_000_000,
                "bytes":536_870_912,"mutations":1_000_000}
        }),
    );
    let _previous = object.insert(
        "read_sessions".into(),
        json!({"global_limit":128,"per_uid_limit":128,
            "terminal_ttl_millis":60_000,"sweep_limit":16}),
    );
    *config
        .pointer_mut("/resources/read_ticket_lease_ms")
        .context("fixed Native config lacks read-ticket lease")? = json!(300_000);
    Ok(config)
}

fn root_bytes(root: &Path) -> Result<u64> {
    if root.exists() {
        Ok(directory_bytes(root)?)
    } else {
        Ok(0)
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

/// Recompute only the deterministic source identity for a failed Native run.
pub fn native_source_binding_for_failure(tier: ScaleTier, seed: u64) -> Result<ScaleSourceBinding> {
    let files = generate_scoped_corpus(tier, seed)?;
    let corpus_bytes = files.iter().try_fold(0_u64, |sum, file| {
        sum.checked_add(u64::try_from(file.content.len())?)
            .context("native refusal source byte sum overflow")
    })?;
    Ok(ScaleSourceBinding {
        tier,
        seed,
        serving_owner_count: 1,
        source_repo_count: params_for(tier).repo_count,
        file_count: files.len(),
        corpus_bytes,
        source_bytes: corpus_bytes,
        corpus_digest: scoped_corpus_digest(DIMENSION, &files),
    })
}

fn timed<T>(
    name: &'static str,
    root: &Path,
    phases: &mut BTreeMap<&'static str, NativePhase>,
    trace: &NativeTrace,
    work: impl FnOnce() -> Result<T>,
) -> Result<T> {
    // Full root census is O(objects). Restrict it to lifecycle boundaries so
    // a 32k-file query is not charged for an unrelated tree walk.
    let census = matches!(
        name,
        "full_event_ack" | "delta_event_ack" | "delete_event_ack" | "final_drain"
    );
    let observed_before = Instant::now();
    let before = census
        .then(|| root_bytes(root))
        .transpose()
        .map_err(|error| stage_or_preserve(name, error))?;
    let observer_before_ms = observed_before.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let span = trace.begin(name);
    let output = work().map_err(|error| stage_or_preserve(name, error));
    // Phase spans describe product work, matching wall_ms. Boundary census
    // is separately charged to observer_ms; its errors still fail the enclosing
    // native operation and completed flag below.
    NativeTrace::finish_phase(span, &output);
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    let observed_after = Instant::now();
    let after = census
        .then(|| root_bytes(root))
        .transpose()
        .map_err(|error| stage_or_preserve(name, error));
    let observer_ms = observed_after
        .elapsed()
        .as_secs_f64()
        .mul_add(1000.0, observer_before_ms);
    let _previous = phases.insert(
        name,
        NativePhase {
            wall_ms: elapsed,
            root_bytes_before: before,
            // Retain and return the exact observation error below.
            root_bytes_after: after.as_ref().map_or(None, |value| *value),
            observer_ms,
            completed: output.is_ok() && after.is_ok(),
        },
    );
    match (output, after) {
        (Ok(value), Ok(_)) => Ok(value),
        (Err(primary), Ok(_)) => Err(primary),
        (Ok(_), Err(observation)) => Err(observation),
        (Err(primary), Err(observation)) => Err(primary.context(format!(
            "disk census after failed {name} also failed: {observation:#}"
        ))),
    }
}

fn advance(
    client: &NativeUdsClient,
    key: OperationKey,
    expected: ViewRef,
    mutations: impl IntoIterator<Item = OwnerMutation>,
    event_sequence: u64,
    publication: &mut PublicationAudit,
) -> Result<ViewRef> {
    let ingress = client.begin_input(key, &budget())?;
    advance_staged(
        client,
        ingress,
        expected,
        mutations,
        event_sequence,
        PublicationLabel {
            phase: "delete_event_ack",
            cycle: None,
        },
        publication,
    )
}

fn advance_staged(
    client: &NativeUdsClient,
    ingress: quanta_index_contract::index::IngressRef,
    expected: ViewRef,
    mutations: impl IntoIterator<Item = OwnerMutation>,
    event_sequence: u64,
    label: PublicationLabel,
    publication: &mut PublicationAudit,
) -> Result<ViewRef> {
    let prior = client.read_view(ViewSelection::Exact(expected), &budget())?;
    let owners = client.stage_mutations(ingress, mutations, &budget())?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::with_capacity(1),
    };
    transition.events.push(SourceEvent {
        producer: ingress.operation.producer,
        stream: ingress.operation.stream,
        stream_epoch: ingress.operation.stream_epoch,
        predecessor: event_sequence
            .checked_sub(1)
            .context("event sequence underflow")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    publication::publish(
        client,
        UpdateRequest {
            operation: ingress.operation,
            expected_view: expected,
            kind: UpdateKind::SourceAdvance(transition),
        },
        label,
        publication,
    )
}

fn advance_frontier_only(
    client: &NativeUdsClient,
    expected: ViewRef,
    publication: &mut PublicationAudit,
) -> Result<ViewRef> {
    let key = operation(4);
    let ingress = client.begin_input(key, &budget())?;
    let prior = client.read_view(ViewSelection::Exact(expected), &budget())?;
    let owners = client.stage_mutations(ingress, std::iter::empty::<OwnerMutation>(), &budget())?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::with_capacity(1),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: 2,
        sequence: 3,
        payload: transition.owner_digest()?,
    });
    let next = publication::publish(
        client,
        UpdateRequest {
            operation: key,
            expected_view: expected,
            kind: UpdateKind::SourceAdvance(transition),
        },
        PublicationLabel {
            phase: "noop_event_ack",
            cycle: None,
        },
        publication,
    )?;
    let after = client.read_view(ViewSelection::Exact(next), &budget())?;
    anyhow::ensure!(
        after.source.owners == prior.source.owners,
        "frontier-only update changed owners"
    );
    anyhow::ensure!(
        after.source.paths == prior.source.paths,
        "frontier-only update changed source paths"
    );
    anyhow::ensure!(
        after.source.frontiers != prior.source.frontiers,
        "frontier-only update did not advance the source frontier"
    );
    anyhow::ensure!(
        after.profile == prior.profile && after.projections == prior.projections,
        "frontier-only update changed unrelated profile/projection authority"
    );
    anyhow::ensure!(
        next.sequence > expected.sequence
            && next.workspace == expected.workspace
            && next.epoch == expected.epoch,
        "frontier-only ACK did not advance the same workspace/epoch"
    );
    Ok(next)
}

fn query_exact(
    client: &NativeUdsClient,
    view: ViewRef,
    token: &str,
    expected_path: Option<&str>,
) -> Result<(f64, u64)> {
    let started = Instant::now();
    let page = client.search_text(
        ViewSelection::Exact(view),
        text_spec().instance,
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: token.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        },
        &budget(),
    )?;
    verify_exact_query(&page, view, token, expected_path, started)
}

fn query_held_exact(
    client: &NativeUdsClient,
    status: &NativeReadSessionStatus,
    token: &str,
    expected_path: Option<&str>,
) -> Result<(f64, u64)> {
    let started = Instant::now();
    let page = client.search_text_in_read_session(
        status,
        text_spec().instance,
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: token.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        },
        &budget(),
    )?;
    verify_exact_query(&page, status.view, token, expected_path, started)
}

fn verify_exact_query(
    page: &quanta_index_contract::native_protocol::NativeTextResult,
    view: ViewRef,
    token: &str,
    expected_path: Option<&str>,
    started: Instant,
) -> Result<(f64, u64)> {
    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    if page.view != view || !page.exhaustive {
        anyhow::bail!("native query returned wrong view or non-exhaustive coverage");
    }
    match expected_path {
        Some(path)
            if page.matched == 1
                && page.hits.len() == 1
                && page.hits.first().is_some_and(|hit| hit.path == path) => {}
        None if page.matched == 0 && page.hits.is_empty() => {}
        _ => anyhow::bail!(
            "native searchability oracle failed for {token}: matched={} hits={:?}",
            page.matched,
            page.hits
        ),
    }
    Ok((wall_ms, page.matched))
}

struct FullViewReadGuard<'a> {
    client: &'a NativeUdsClient,
    status: Option<NativeReadSessionStatus>,
    last_renew: Instant,
}

impl<'a> FullViewReadGuard<'a> {
    fn open(client: &'a NativeUdsClient, view: ViewRef) -> Result<Self> {
        let status = client.open_read_session(
            ViewSelection::Exact(view),
            retained_read::session_key(view, b"pre-mixed-full-view"),
            &RequestBudgetV1::for_duration(Duration::from_secs(2)),
        )?;
        anyhow::ensure!(
            status.view == view,
            "pre-mixed full-view read ticket differs"
        );
        Ok(Self {
            client,
            status: Some(status),
            last_renew: Instant::now(),
        })
    }

    fn status(&self) -> Result<&NativeReadSessionStatus> {
        self.status
            .as_ref()
            .context("pre-mixed full-view read ticket absent")
    }

    fn renew_between_stages(&mut self) -> Result<()> {
        let gap = self.last_renew.elapsed();
        anyhow::ensure!(
            gap <= Duration::from_secs(180),
            "pre-mixed full-view renewal gap exceeded 180 seconds"
        );
        if gap >= Duration::from_secs(120) {
            let current = *self.status()?;
            let renewed = self.client.renew_read_session(
                &current,
                &RequestBudgetV1::for_duration(Duration::from_secs(2)),
            )?;
            anyhow::ensure!(
                renewed.expires_at_ms > current.expires_at_ms,
                "pre-mixed full-view renewal did not extend lease"
            );
            self.status = Some(renewed);
            self.last_renew = Instant::now();
        }
        Ok(())
    }

    fn release(&mut self) -> Result<()> {
        if let Some(status) = self.status {
            let (released, remaining) = self.client.release_read_session(
                &status,
                &RequestBudgetV1::for_duration(Duration::from_secs(2)),
            )?;
            self.status = None;
            anyhow::ensure!(
                released.view == status.view && !remaining,
                "pre-mixed full-view ticket release did not close last pin"
            );
        }
        Ok(())
    }
}

impl Drop for FullViewReadGuard<'_> {
    fn drop(&mut self) {
        if self.status.is_some() {
            let _cleanup = self.release();
        }
    }
}

/// Executes the configured Native daemon via the real UDS client. The result
/// covers Index intake/ACK/search only; Semantica capture and Original analysis
/// are not part of this fixture.
pub fn measure_index_native_tier(
    tier: ScaleTier,
    seed: u64,
    daemon_bin: &Path,
    capture: Option<quanta_index_searchd::DiagnosticCaptureLeaseV1>,
    child_trace_dir: Option<&Path>,
    trace_max_output_bytes: u64,
) -> Result<NativeIndexMeasurement> {
    let _validated_limits = quanta_index_searchd::DiagnosticCaptureLimitsV1::default()
        .with_max_output_bytes(trace_max_output_bytes)?;
    let trace = NativeTrace::new(capture);
    let mut child_traces = None;
    let result = (|| {
        if trace.enabled() {
            child_traces = Some(ChildTraceArtifacts::create(
                child_trace_dir.context("native Record capture output directory absent")?,
                trace_max_output_bytes,
            )?);
        }
        let workload = selected_workload_from_env(tier)?;
        measure_native_inner(
            tier,
            seed,
            daemon_bin,
            &trace,
            workload,
            child_traces.as_mut(),
            trace_max_output_bytes,
        )
        .with_context(|| format!("selected native foreground funding={}", workload.metadata()))
    })();
    let result = match result {
        Ok(mut measurement) => {
            measurement.child_traces = child_traces;
            Ok(measurement)
        }
        Err(error) => Err(match child_traces {
            Some(captures) => error.context(captures),
            None => error,
        }),
    };
    trace.finish(&result);
    result
}

pub(super) fn failure_capture_detail(error: &anyhow::Error) -> Option<Value> {
    error
        .downcast_ref::<ChildTraceArtifacts>()
        .map(ChildTraceArtifacts::failure_detail)
}

fn measure_native_inner(
    tier: ScaleTier,
    seed: u64,
    daemon_bin: &Path,
    trace: &NativeTrace,
    workload: NativeForegroundWorkload,
    mut child_traces: Option<&mut ChildTraceArtifacts>,
    trace_max_output_bytes: u64,
) -> Result<NativeIndexMeasurement> {
    let mixed_enabled = configured_mixed_load(tier, seed, workload)?;
    anyhow::ensure!(
        !mixed_enabled || !trace.enabled(),
        "native mixed load uses Off mode; full Record retention has no declared bounded capture profile"
    );
    let mut mixed_detail = None;
    let mut final_first_token = None;
    let mut final_first_ref = None;
    let overall = Instant::now();
    let observation = NativeObservation::new(ObservationMode::from_env()?);
    let mut churn = NativeChurnProgress {
        requested: configured_churn_cycles()?,
        ..NativeChurnProgress::default()
    };
    let mut publication = PublicationAudit::default();
    let mut child_object_io = BTreeMap::new();
    let mut phases = BTreeMap::new();
    let temp = TempDir::new()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config = temp.path().join("native.json");
    let first_trace = child_traces
        .as_ref()
        .map(|captures| captures.path("first"))
        .transpose()?;
    let reopen_trace = child_traces
        .as_ref()
        .map(|captures| captures.path("reopen"))
        .transpose()?;
    let first_stderr = child_traces
        .as_ref()
        .map(|captures| captures.stderr_path("first"))
        .transpose()?
        .unwrap_or_else(|| temp.path().join("first.stderr"));
    let reopen_stderr = child_traces
        .as_ref()
        .map(|captures| captures.stderr_path("reopen"))
        .transpose()?
        .unwrap_or_else(|| temp.path().join("reopen.stderr"));
    let corpus = timed("corpus_generation", &root, &mut phases, trace, || {
        generate_scoped_corpus(tier, seed)
    })
    .map_err(|error| with_prefix(&phases, &observation, &churn, &publication, workload, error))?;
    let (corpus_digest, source_bytes, native_config_sha256, daemon_executable) =
        timed("config_setup", &root, &mut phases, trace, || {
            let corpus_digest = scoped_corpus_digest(DIMENSION, &corpus);
            let source_bytes = corpus.iter().try_fold(0_u64, |sum, file| {
                sum.checked_add(u64::try_from(file.content.len())?)
                    .context("source byte sum overflow")
            })?;
            let configured = if mixed_enabled {
                mixed_native_config(&root, &socket, workload)?
            } else {
                native_config(&root, &socket, workload)
            };
            let canonical = if mixed_enabled {
                mixed_native_config(
                    Path::new("/fixture-state"),
                    Path::new("/fixture.sock"),
                    workload,
                )?
            } else {
                native_config(
                    Path::new("/fixture-state"),
                    Path::new("/fixture.sock"),
                    workload,
                )
            };
            let native_config_sha256 = hex_sha256(&serde_json::to_vec(&canonical)?);
            let daemon_executable = NativeExecutable::capture(daemon_bin)?;
            fs::write(&config, serde_json::to_vec(&configured)?)?;
            fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;
            Ok((
                corpus_digest,
                source_bytes,
                native_config_sha256,
                daemon_executable,
            ))
        })
        .map_err(|error| {
            with_prefix(&phases, &observation, &churn, &publication, workload, error)
        })?;
    let daemon = timed("startup", &root, &mut phases, trace, || {
        NativeDaemon::start(
            &daemon_executable,
            &config,
            &socket,
            first_trace.as_deref(),
            trace_max_output_bytes,
            &first_stderr,
        )
    })
    .map_err(|error| with_prefix(&phases, &observation, &churn, &publication, workload, error))?;
    let mut sampler = observation.start("first_daemon", daemon.child.id(), &root);
    let client = NativeUdsClient::new(socket.clone());
    let run = (|| -> Result<(NativeIndexMeasurement, ViewRef)> {
        let genesis = timed("workspace_create", &root, &mut phases, trace, || {
            let response = client.request(
                NativeIndexRequest::CreateWorkspace {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )?;
            let NativeIndexResponse::Created(view) = response else {
                anyhow::bail!("native workspace creation reply: {response:?}");
            };
            Ok(view)
        })?;
        let configured_view = timed("profile_configure", &root, &mut phases, trace, || {
            let ingress = client.begin_input(operation(1), &budget())?;
            let profile = client.stage_profile(
                ingress,
                IndexProfile {
                    instances: vec![text_spec()],
                },
                &budget(),
            )?;
            publication::publish(
                &client,
                UpdateRequest {
                    operation: operation(1),
                    expected_view: genesis,
                    kind: UpdateKind::Reconfigure { profile },
                },
                PublicationLabel {
                    phase: "profile_configure",
                    cycle: None,
                },
                &mut publication,
            )
        })?;
        let ingress = timed("full_begin_input", &root, &mut phases, trace, || {
            Ok(client.begin_input(operation(2), &budget())?)
        })?;
        let refs = timed("full_stage_sources", &root, &mut phases, trace, || {
            let mut refs = Vec::with_capacity(corpus.len());
            for page in corpus.chunks(MAX_NATIVE_STAGE_SOURCES_INPUTS) {
                let staged = client.stage_sources(
                    ingress,
                    page.iter()
                        .map(|file| SourceInput {
                            path: format!("{}/{}", file.source_repo_id, file.repo_relative_path),
                            bytes: file.content.as_bytes().to_vec(),
                            text: SourceTextAdmission::Utf8 {
                                policy: Digest([31; 32]),
                            },
                        })
                        .collect(),
                    &budget(),
                )?;
                if staged.len() != page.len() {
                    anyhow::bail!("native source ACK omitted refs");
                }
                refs.extend(staged);
            }
            Ok(refs)
        })?;
        let full_view = timed("full_event_ack", &root, &mut phases, trace, || {
            let mutations = refs
                .iter()
                .enumerate()
                .map(|(index, reference)| {
                    Ok(OwnerMutation {
                        owner: owner(index)?,
                        expected: None,
                        next: Some(*reference),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            advance_staged(
                &client,
                ingress,
                configured_view,
                mutations,
                1,
                PublicationLabel {
                    phase: "full_event_ack",
                    cycle: None,
                },
                &mut publication,
            )
        })?;
        let first_token = file_query_token(0, 0);
        let (first_query_ms, first_query_matched) =
            timed("first_search", &root, &mut phases, trace, || {
                query_exact(&client, full_view, &first_token, Some(FIRST_PATH))
            })?;
        let mut held_full = if mixed_enabled {
            Some(FullViewReadGuard::open(&client, full_view)?)
        } else {
            None
        };
        let delta_ref = timed("delta_stage_source", &root, &mut phases, trace, || {
            let ingress = client.begin_input(operation(3), &budget())?;
            let content = format!(
                "{}\n// {DELTA_TOKEN}\n",
                corpus.first().context("native scale corpus empty")?.content
            );
            let reference = client.stage_source(
                ingress,
                FIRST_PATH.into(),
                content.as_bytes(),
                SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                &budget(),
            )?;
            Ok((ingress, reference))
        })?;
        if let Some(held) = held_full.as_mut() {
            held.renew_between_stages()?;
        }
        let delta_view = timed("delta_event_ack", &root, &mut phases, trace, || {
            advance_staged(
                &client,
                delta_ref.0,
                full_view,
                [OwnerMutation {
                    owner: owner(0)?,
                    expected: Some(*refs.first().context("native full source ACK empty")?),
                    next: Some(delta_ref.1),
                }],
                2,
                PublicationLabel {
                    phase: "delta_event_ack",
                    cycle: None,
                },
                &mut publication,
            )
        })?;
        if let Some(held) = held_full.as_mut() {
            held.renew_between_stages()?;
        }
        let _observed = timed("delta_search", &root, &mut phases, trace, || {
            query_exact(&client, delta_view, DELTA_TOKEN, Some(FIRST_PATH))
        })?;
        let noop_view = timed("noop_event_ack", &root, &mut phases, trace, || {
            advance_frontier_only(&client, delta_view, &mut publication)
        })?;
        if let Some(held) = held_full.as_mut() {
            held.renew_between_stages()?;
        }
        let _observed = timed("noop_stable_search", &root, &mut phases, trace, || {
            query_exact(&client, noop_view, DELTA_TOKEN, Some(FIRST_PATH))
        })?;
        let deleted_view = timed("delete_event_ack", &root, &mut phases, trace, || {
            advance(
                &client,
                operation(5),
                noop_view,
                [OwnerMutation {
                    owner: owner(0)?,
                    expected: Some(delta_ref.1),
                    next: None,
                }],
                4,
                &mut publication,
            )
        })?;
        if let Some(held) = held_full.as_mut() {
            held.renew_between_stages()?;
        }
        let _observed = timed("deleted_search", &root, &mut phases, trace, || {
            query_exact(&client, deleted_view, DELTA_TOKEN, None)
        })?;
        let _observed = timed("old_pin_search", &root, &mut phases, trace, || {
            if let Some(held) = held_full.as_mut() {
                held.renew_between_stages()?;
                let _absent = query_held_exact(&client, held.status()?, DELTA_TOKEN, None)?;
                query_held_exact(&client, held.status()?, &first_token, Some(FIRST_PATH))
            } else {
                let _absent = query_exact(&client, full_view, DELTA_TOKEN, None)?;
                query_exact(&client, full_view, &first_token, Some(FIRST_PATH))
            }
        })?;
        let mut final_view = if churn.requested == 0 {
            deleted_view
        } else {
            timed("churn_cycles", &root, &mut phases, trace, || {
                let mut previous_ref = None;
                let mut previous_token: Option<String> = None;
                let mut current_view = deleted_view;
                let original = &corpus.first().context("native scale corpus empty")?.content;
                for cycle in 0..churn.requested {
                    if let Some(held) = held_full.as_mut() {
                        held.renew_between_stages()?;
                    }
                    let attempt = cycle.checked_add(1).context("churn attempt overflow")?;
                    churn.last_attempt = Some(attempt);
                    let started = Instant::now();
                    let token = format!("qidxchurnneedle{attempt}");
                    let key = operation(
                        u64::from(cycle)
                            .checked_add(6)
                            .context("churn operation overflow")?,
                    );
                    let ingress = client.begin_input(key, &budget())?;
                    let content = format!("{original}\n// {token}\n");
                    let reference = client.stage_source(
                        ingress,
                        FIRST_PATH.into(),
                        content.as_bytes(),
                        SourceTextAdmission::Utf8 {
                            policy: Digest([31; 32]),
                        },
                        &budget(),
                    )?;
                    current_view = advance_staged(
                        &client,
                        ingress,
                        current_view,
                        [OwnerMutation {
                            owner: owner(0)?,
                            expected: previous_ref,
                            next: Some(reference),
                        }],
                        u64::from(cycle)
                            .checked_add(5)
                            .context("churn event overflow")?,
                        PublicationLabel {
                            phase: "churn_cycles",
                            cycle: Some(attempt),
                        },
                        &mut publication,
                    )?;
                    if let Some(held) = held_full.as_mut() {
                        held.renew_between_stages()?;
                    }
                    churn.ack_wall_ms += started.elapsed().as_secs_f64() * 1_000.0;
                    let queries_started = Instant::now();
                    let _hit = query_exact(&client, current_view, &token, Some(FIRST_PATH))?;
                    if let Some(prior) = previous_token.as_deref() {
                        let _absent = query_exact(&client, current_view, prior, None)?;
                    }
                    churn.query_wall_ms += queries_started.elapsed().as_secs_f64() * 1_000.0;
                    churn.max_cycle_wall_ms = churn
                        .max_cycle_wall_ms
                        .max(started.elapsed().as_secs_f64() * 1_000.0);
                    churn.completed = attempt;
                    previous_ref = Some(reference);
                    previous_token = Some(token);
                }
                final_first_ref = previous_ref;
                final_first_token = previous_token;
                Ok(current_view)
            })?
        };
        if churn.requested > 0 {
            let _observed = timed("churn_old_view_search", &root, &mut phases, trace, || {
                let latest_token = format!("qidxchurnneedle{}", churn.requested);
                if let Some(held) = held_full.as_mut() {
                    held.renew_between_stages()?;
                    let _absent = query_held_exact(&client, held.status()?, &latest_token, None)?;
                    query_held_exact(&client, held.status()?, &first_token, Some(FIRST_PATH))
                } else {
                    let _absent = query_exact(&client, full_view, &latest_token, None)?;
                    query_exact(&client, full_view, &first_token, Some(FIRST_PATH))
                }
            })?;
        }
        if let Some(held) = held_full.as_mut() {
            held.release()?;
        }
        if mixed_enabled {
            // Preserve the finite-churn old-view gold before the long run.
            // Holding a ViewRef across this workload would not acquire a pin.
            let mixed = mixed_load::run(
                &socket,
                tier,
                seed,
                &corpus,
                &refs,
                mixed_load::MixedStart {
                    view: final_view,
                    first_ref: final_first_ref,
                    first_token: final_first_token.take(),
                    next_operation_sequence: u64::from(churn.requested)
                        .checked_add(6)
                        .context("mixed operation sequence overflow")?,
                    next_event_sequence: u64::from(churn.requested)
                        .checked_add(5)
                        .context("mixed event sequence overflow")?,
                    transition_step: 0,
                },
            )?;
            anyhow::ensure!(
                mixed.first_ref.is_some() == mixed.first_token.is_some(),
                "mixed final first-owner witness is inconsistent"
            );
            final_view = mixed.final_view;
            final_first_token = mixed.first_token;
            mixed_detail = Some(mixed.detail);
        }
        // The daemon owns mandatory upkeep; its cooperative shutdown is part
        // of the observed run and must complete before reopen is timed.
        Ok((
            NativeIndexMeasurement {
                tier,
                seed,
                corpus_digest,
                daemon_sha256: daemon_executable.sha256().to_owned(),
                native_config_sha256,
                foreground_workload: workload.metadata(),
                publication_outcomes: publication.detail(),
                file_count: corpus.len(),
                source_bytes,
                phases: BTreeMap::new(),
                first_query_ms,
                first_query_matched,
                total_ms: 0.0,
                resource_observation: None,
                churn_cycles: 0,
                churn: None,
                child_trace_verified: false,
                child_traces: None,
                child_object_io: None,
                mixed_load: None,
            },
            final_view,
        ))
    })();
    let stopped = timed("drain", &root, &mut phases, trace, || {
        daemon.stop(&mut sampler)
    });
    let stopped = verify_child_trace(
        stopped,
        ChildTraceEpoch {
            path: first_trace.as_deref(),
            publication: run.is_ok(),
            phase: "drain",
            stderr: &first_stderr,
            captures: child_traces.as_deref_mut(),
            role: "first",
        },
        &mut phases,
        &mut child_object_io,
    );
    let (mut measurement, terminal_ack_view) = match (run, stopped) {
        (Ok(result), Ok(())) => result,
        (Err(error), Ok(())) | (Ok(_), Err(error)) => {
            return Err(with_prefix(
                &phases,
                &observation,
                &churn,
                &publication,
                workload,
                daemon_diagnostic::attach(error, &first_stderr),
            ));
        }
        (Err(primary), Err(cleanup)) => {
            return Err(with_prefix(
                &phases,
                &observation,
                &churn,
                &publication,
                workload,
                daemon_diagnostic::attach(
                    primary.context(format!("native drain also failed: {cleanup:#}")),
                    &first_stderr,
                ),
            ));
        }
    };
    let reopened = timed("reopen", &root, &mut phases, trace, || {
        NativeDaemon::start(
            &daemon_executable,
            &config,
            &socket,
            reopen_trace.as_deref(),
            trace_max_output_bytes,
            &reopen_stderr,
        )
    })
    .map_err(|error| with_prefix(&phases, &observation, &churn, &publication, workload, error))?;
    let mut reopen_sampler = observation.start("reopened_daemon", reopened.child.id(), &root);
    let reopened_client = NativeUdsClient::new(socket);
    let reopen_run = (|| -> Result<()> {
        let last = corpus.last().context("native scale corpus empty")?;
        let last_repo = last
            .source_repo_id
            .strip_prefix("repo")
            .context("invalid last repo ID")?
            .parse::<u32>()?;
        let last_file = last
            .repo_relative_path
            .strip_prefix("src/file_")
            .and_then(|value| value.strip_suffix(".rs"))
            .context("invalid last file path")?
            .parse::<u32>()?;
        // A new process must read the durable head rather than substitute a
        // request-local generation for the old view identity.
        let head = timed("reopen_read_head", &root, &mut phases, trace, || {
            let reopened_head = reopened_client
                .read_view(
                    ViewSelection::Latest {
                        workspace: workspace(),
                        epoch: 1,
                    },
                    &budget(),
                )?
                .identity;
            if reopened_head != terminal_ack_view {
                anyhow::bail!(
                    "reopened head differs from final ACK: expected {terminal_ack_view:?}, got {reopened_head:?}"
                );
            }
            Ok(reopened_head)
        })?;
        let _observed = timed("reopen_deleted_search", &root, &mut phases, trace, || {
            query_exact(&reopened_client, head, DELTA_TOKEN, None)
        })?;
        if let Some(last_token) = final_first_token.as_deref() {
            let _observed = timed("reopen_churn_search", &root, &mut phases, trace, || {
                query_exact(&reopened_client, head, last_token, Some(FIRST_PATH))
            })?;
        } else if mixed_enabled {
            let _observed = timed("reopen_churn_search", &root, &mut phases, trace, || {
                query_exact(&reopened_client, head, &file_query_token(0, 0), None)
            })?;
        }
        let last_path = format!("{}/{}", last.source_repo_id, last.repo_relative_path);
        let _observed = timed("reopen_search", &root, &mut phases, trace, || {
            query_exact(
                &reopened_client,
                head,
                &file_query_token(last_repo, last_file),
                Some(&last_path),
            )
        })?;
        Ok(())
    })();
    let final_drain = timed("final_drain", &root, &mut phases, trace, || {
        reopened.stop(&mut reopen_sampler)
    });
    let final_drain = verify_child_trace(
        final_drain,
        ChildTraceEpoch {
            path: reopen_trace.as_deref(),
            publication: false,
            phase: "final_drain",
            stderr: &reopen_stderr,
            captures: child_traces,
            role: "reopen",
        },
        &mut phases,
        &mut child_object_io,
    );
    match (reopen_run, final_drain) {
        (Ok(()), Ok(())) => {}
        (Err(error), Ok(())) | (Ok(()), Err(error)) => {
            return Err(with_prefix(
                &phases,
                &observation,
                &churn,
                &publication,
                workload,
                daemon_diagnostic::attach(error, &reopen_stderr),
            ));
        }
        (Err(primary), Err(cleanup)) => {
            return Err(with_prefix(
                &phases,
                &observation,
                &churn,
                &publication,
                workload,
                daemon_diagnostic::attach(
                    primary.context(format!("native final drain also failed: {cleanup:#}")),
                    &reopen_stderr,
                ),
            ));
        }
    }
    measurement.phases = phases;
    measurement.total_ms = overall.elapsed().as_secs_f64() * 1000.0;
    measurement.resource_observation = observation.detail();
    measurement.churn_cycles = churn.requested;
    measurement.churn = (churn.requested > 0).then(|| churn.detail());
    measurement.mixed_load = mixed_detail;
    measurement.child_trace_verified = trace.enabled();
    measurement.child_object_io = trace.enabled().then(|| json!({
        "scope":"quiescent cumulative object-install IO; excludes constructor, GC, Catalog/WAL and device media traffic; elapsed call sums may overlap across workers",
        "epochs":child_object_io,
    }));
    Ok(measurement)
}

struct ChildTraceEpoch<'a> {
    path: Option<&'a Path>,
    publication: bool,
    phase: &'static str,
    stderr: &'a Path,
    captures: Option<&'a mut ChildTraceArtifacts>,
    role: &'static str,
}

fn verify_child_trace(
    stopped: Result<()>,
    epoch: ChildTraceEpoch<'_>,
    phases: &mut BTreeMap<&'static str, NativePhase>,
    observations: &mut BTreeMap<&'static str, Value>,
) -> Result<()> {
    stopped?;
    let ChildTraceEpoch {
        path,
        publication,
        phase,
        stderr,
        captures,
        role,
    } = epoch;
    let Some(path) = path else {
        return Ok(());
    };
    let start = Instant::now();
    let captures = captures.context("child trace output custody absent")?;
    let result = child_trace::verify(path, publication, captures.max_bytes()).and_then(|capture| {
        let diagnostic = daemon_diagnostic::verified_object_io(stderr, &capture)?;
        captures.remember(role, publication, &capture, &diagnostic)?;
        let _previous = observations.insert(phase, diagnostic.summary().clone());
        Ok(())
    });
    phases
        .get_mut(phase)
        .context("child capture drain phase absent")?
        .observer_ms += start.elapsed().as_secs_f64() * 1_000.0;
    result
}

fn phase_json(phases: &BTreeMap<&'static str, NativePhase>) -> Value {
    Value::Object(
        phases
            .iter()
            .map(|(name, phase)| {
                (
                    (*name).to_owned(),
                    json!({
                        "wall_ms":phase.wall_ms,
                        "root_bytes_before":phase.root_bytes_before,
                        "root_bytes_after":phase.root_bytes_after,
                        "observer_ms":phase.observer_ms,
                        "completed":phase.completed,
                        "root_net_bytes":phase.root_bytes_before.zip(phase.root_bytes_after)
                            .and_then(|(before, after)| i128::from(after).checked_sub(i128::from(before)))
                    }),
                )
            })
            .collect(),
    )
}

impl NativeIndexMeasurement {
    /// Keep retained raw paths discoverable when artifact writing or its trace fails.
    #[must_use]
    pub fn with_failure_capture_paths(&self, error: anyhow::Error) -> anyhow::Error {
        match &self.child_traces {
            Some(captures) if error.downcast_ref::<ChildTraceArtifacts>().is_none() => {
                error.context(captures.clone())
            }
            _ => error,
        }
    }
}

pub fn write_index_native_artifacts(
    measurement: &NativeIndexMeasurement,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> Result<()> {
    write_index_native_artifacts_inner(measurement, dir, git_head, host)
        .map_err(|error| measurement.with_failure_capture_paths(error))
}

fn write_index_native_artifacts_inner(
    measurement: &NativeIndexMeasurement,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> Result<()> {
    let child_trace_files = measurement
        .child_traces
        .as_ref()
        .map(ChildTraceArtifacts::success_detail)
        .transpose()?;
    anyhow::ensure!(
        measurement.child_trace_verified == child_trace_files.is_some(),
        "native trace verification has no retained raw capture binding"
    );
    let params = params_for(measurement.tier);
    let full = measurement
        .phases
        .get("full_event_ack")
        .context("native full ACK phase absent")?;
    let delta = measurement
        .phases
        .get("delta_event_ack")
        .context("native delta ACK phase absent")?;
    let full_stage = measurement
        .phases
        .get("full_stage_sources")
        .context("native full source phase absent")?;
    let full_begin = measurement
        .phases
        .get("full_begin_input")
        .context("native full begin phase absent")?;
    let delta_stage = measurement
        .phases
        .get("delta_stage_source")
        .context("native delta source phase absent")?;
    let mut digest_inputs = vec![
        ("tier", params.tier.as_str().into()),
        ("seed", measurement.seed.to_string()),
        ("repo_count", params.repo_count.to_string()),
        ("files_per_repo", params.files_per_repo.to_string()),
        ("backend", "configured-native-daemon-uds-v1".into()),
        (
            "foreground_workload",
            measurement.foreground_workload.to_string(),
        ),
        (
            "publication_completion_method",
            "single_submit_same_key_status_original_budget_v1".into(),
        ),
        ("daemon_sha256", measurement.daemon_sha256.clone()),
        (
            "native_config_sha256",
            measurement.native_config_sha256.clone(),
        ),
    ];
    if measurement.resource_observation.is_some() {
        digest_inputs.push(("resource_observation", "opt_in_diagnostic_v2".into()));
    }
    if measurement.churn_cycles > 0 {
        digest_inputs.push(("churn_cycles", measurement.churn_cycles.to_string()));
    }
    if measurement.mixed_load.is_some() {
        digest_inputs.push((
            "mixed_load",
            "large-seed42-60s-warmup-900s-50qps-held-v2".into(),
        ));
    }
    let mut detail = json!({
        "backend":"index-native-configured-daemon-uds",
        "scope":"index-only-source-event-to-ACK-to-searchability",
        "trace_phase_scope":if measurement.child_trace_verified {
            "harness work body only; excludes boundary census observer_ms; operation terminal includes observation and cleanup errors; child capture independently verifies lifecycle, IPC, build, publish and quiescent install sync"
        } else {
            "harness work body only; excludes boundary census observer_ms; operation terminal includes observation and cleanup errors; no canonical child capture verified"
        },
        "tier":measurement.tier.as_str(), "seed":measurement.seed,
        "daemon_sha256":measurement.daemon_sha256,
        "daemon_binding_scope":"initial digest and path/content epoch rechecked before startup, after socket-ready and after each drain; excludes kernel exec identity and source/build attestation; startup/reopen/drain wall includes these verification reads",
        "native_config_sha256":measurement.native_config_sha256,
        "foreground_workload":measurement.foreground_workload,
        "child_trace_verified":measurement.child_trace_verified,
        "child_trace_files":child_trace_files,
        "child_object_io":measurement.child_object_io,
        "mixed_load":measurement.mixed_load,
        "child_trace_clock_scope":if measurement.child_trace_verified {
            "independent child process captures; no harness/child clock alignment inferred"
        } else {
            "no verified child process captures; no harness/child clock alignment inferred"
        },
        "publication_completion_scope":"immediate_ack means the original submit returned Published; reconciled_status means the same original key was observed Published via read-only Status without replay",
        "publication_outcomes":measurement.publication_outcomes,
        "file_count":measurement.file_count, "source_bytes":measurement.source_bytes,
        "total_ms":measurement.total_ms, "phases":phase_json(&measurement.phases),
        "source_mutation_count":{"full":measurement.file_count,"delta":1,"noop":0,"delete":1},
        "derived_fanout":"unobserved; no projection producer in Index-only fixture",
        "included":["corpus_generation","daemon_startup","workspace_setup",
            "source_stage","mutation_stage","event_ack","query","drain","reopen"],
        "excluded":["semantica_capture","semantica_analysis","Original_budget",
            "semantic_materialization","cross_process_provider"],
        "resource_limits":{"rss":if measurement.resource_observation.is_some() {
                "parent_resource_field; child and Linux harness/daemon/observed-descendant RSS sampled separately in detail; not a continuous peak"
            } else { "parent_process_only; child_daemon_not_sampled" },
            "io":if measurement.resource_observation.is_some() {
                "state_root and native_scratch allocated_blocks_sampled separately; Linux child process IO sampled where supported; physical media and whole lifetime IO not measured"
            } else { "state_root_logical_size_boundaries_only; physical_write_bytes_not_sampled" },
            "diagnostic_output":"retained child trace files are outside the measured state/fixture disk census",
            "gc":"mandatory_daemon_drain_included; GC time not separately observed"},
        "noop":"frontier-only source event publishes and ACKs; owner/path roots unchanged and new frontier view re-queried; not zero total update cost",
        "catch_up":"not_applicable_to_core_required_text; async producer absent",
    });
    if let (Some(object), Some(observation)) = (
        detail.as_object_mut(),
        measurement.resource_observation.as_ref(),
    ) {
        let _previous = object.insert("native_resource_observation".into(), observation.clone());
    }
    if let (Some(object), Some(churn)) = (detail.as_object_mut(), measurement.churn.as_ref()) {
        let _previous = object.insert("finite_churn".into(), churn.clone());
    }
    if measurement.churn_cycles > 0
        && let Some(counts) = detail
            .get_mut("source_mutation_count")
            .and_then(Value::as_object_mut)
    {
        let _previous = counts.insert("churn".into(), json!(measurement.churn_cycles));
    }
    let artifact = BenchArtifactV1 {
        dimension: DIMENSION.into(),
        mode: BenchMode::Cold,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: measurement.corpus_digest.clone(),
            config_digest: config_digest(DIMENSION, &digest_inputs),
            model_revision: None,
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1 {
            build_ms: Some(full_begin.wall_ms + full_stage.wall_ms + full.wall_ms),
            update_ms: Some(delta_stage.wall_ms + delta.wall_ms),
            gc_ms: None,
        },
        // Boundary root-size changes are not physical write-byte counters.
        disk_amplification: None,
        rows: vec![BenchRowV1 {
            scenario_id: format!("index-native-first-search-{}", measurement.tier.as_str()),
            route_family: RouteFamily::Lexical,
            syntax: BenchSyntax::Native,
            result_shape: ResultShape::Candidates,
            latency: Some(LatencySummary {
                p50_ms: measurement.first_query_ms,
                p95_ms: measurement.first_query_ms,
                p99_ms: measurement.first_query_ms,
                samples: 1,
            }),
            qps: None,
            error_count: 0,
            timeout_count: 0,
            result_count: Some(measurement.first_query_matched),
            typed_error_code: None,
            engine_touched: vec!["native_text".into()],
            early_stop_reason: None,
        }],
        detail,
    };
    crate::artifact::write_json_pretty(&dir.join("tier_manifest.json"), &tier_manifest_json())?;
    artifact.write_to(&dir.join("summary.json"))?;
    Ok(())
}

#[cfg(test)]
mod product_tests {
    use super::*;

    pub(super) fn required_field<'a>(value: &'a Value, path: &str) -> Result<&'a Value> {
        value
            .pointer(path)
            .with_context(|| format!("native diagnostic field absent: {path}"))
    }

    #[test]
    fn replaced_executable_is_refused_before_startup_side_effects() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("daemon");
        fs::write(&path, b"original daemon bytes")?;
        let executable = NativeExecutable::capture(&path)?;
        let replacement = directory.path().join("replacement");
        fs::write(&replacement, b"replacement daemon bytes")?;
        fs::rename(replacement, &path)?;
        let config = directory.path().join("config.json");
        let socket = directory.path().join("daemon.sock");

        let stderr = directory.path().join("first.stderr");
        let result = NativeDaemon::start(&executable, &config, &socket, None, 67_108_864, &stderr);
        anyhow::ensure!(result.is_err(), "replacement reached daemon startup");
        anyhow::ensure!(
            !stderr.exists() && !socket.exists(),
            "replacement rejection happened after startup side effects"
        );
        Ok(())
    }

    #[test]
    fn startup_failure_keeps_distinct_private_stderr_epochs_without_clobber() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let binary = directory.path().join("failed-daemon");
        fs::write(&binary, b"#!/bin/sh\nprintf '%s\\n' \"$3\" >&2\nexit 23\n")?;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
        let executable = NativeExecutable::capture(&binary)?;
        let socket = directory.path().join("daemon.sock");
        let first_config = directory.path().join("first.json");
        let reopen_config = directory.path().join("reopen.json");
        let first = directory.path().join("first.stderr");
        let reopen = directory.path().join("reopen.stderr");
        for (config, stderr) in [(&first_config, &first), (&reopen_config, &reopen)] {
            let result =
                NativeDaemon::start(&executable, config, &socket, None, 67_108_864, stderr);
            let error = result
                .err()
                .context("failing fixture daemon reached readiness")?;
            anyhow::ensure!(
                format!("{error:#}").contains(config.to_string_lossy().as_ref()),
                "startup refusal lost this epoch's raw diagnostic"
            );
            anyhow::ensure!(fs::metadata(stderr)?.permissions().mode() & 0o777 == 0o600);
        }
        let expected_first = format!("{}\n", first_config.display());
        let expected_reopen = format!("{}\n", reopen_config.display());
        anyhow::ensure!(fs::read_to_string(&first)? == expected_first);
        anyhow::ensure!(fs::read_to_string(&reopen)? == expected_reopen);
        let duplicate = NativeDaemon::start(
            &executable,
            &reopen_config,
            &socket,
            None,
            67_108_864,
            &first,
        );
        let error = duplicate.err().context("existing stderr was reused")?;
        anyhow::ensure!(format!("{error:#}").contains("create exclusive native daemon stderr"));
        anyhow::ensure!(
            fs::read_to_string(first)? == expected_first,
            "later startup truncated an earlier epoch's raw stderr"
        );
        Ok(())
    }

    #[test]
    fn native_record_child_receives_selected_output_cap() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let binary = directory.path().join("argument-daemon");
        fs::write(&binary, b"#!/bin/sh\nprintf '%s\\n' \"$@\" >&2\nexit 23\n")?;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
        let executable = NativeExecutable::capture(&binary)?;
        let config = directory.path().join("native.json");
        let socket = directory.path().join("native.sock");
        let trace = directory.path().join("native.jsonl");
        let stderr = directory.path().join("native.stderr");
        anyhow::ensure!(
            NativeDaemon::start(
                &executable,
                &config,
                &socket,
                Some(&trace),
                1_073_741_824,
                &stderr,
            )
            .is_err(),
            "argument fixture unexpectedly served"
        );
        let arguments = fs::read_to_string(stderr)?;
        anyhow::ensure!(
            arguments.contains("--diagnostic-trace\n")
                && arguments.contains(&format!("{}\n", trace.display()))
                && arguments.contains("--trace-max-output-bytes\n1073741824\n"),
            "native child lost the selected trace output cap: {arguments}"
        );
        Ok(())
    }

    #[test]
    fn actual_refusal_writer_preserves_child_raw_paths_and_original_failure() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut captures = ChildTraceArtifacts::create(
            &directory.path().join("children"),
            child_trace::MAX_BYTES,
        )?;
        let first = captures.path("first")?;
        child_trace::write_balanced_fixture(&first)?;
        let capture = child_trace::verify(&first, false, child_trace::MAX_BYTES)?;
        let first_stderr = captures.stderr_path("first")?;
        daemon_diagnostic::write_balanced_stderr_fixture(&first_stderr, &capture)?;
        let diagnostic = daemon_diagnostic::verified_object_io(&first_stderr, &capture)?;
        captures.remember("first", false, &capture, &diagnostic)?;
        let error = anyhow::anyhow!("independent original product failure").context(captures);
        let binding = native_source_binding_for_failure(ScaleTier::Small, 42)?;
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567")?;
        let host = HostV1 {
            os: "linux".into(),
            arch: "aarch64".into(),
            cpu_count: 2,
            mem_bytes: 6_442_450_944,
            hostname_hash: "fixed-test-host".into(),
        };
        super::super::write_refusal_artifact_with_context(
            &binding,
            directory.path(),
            &head,
            &host,
            &error,
            DIMENSION,
            None,
        )?;
        let refusal: Value =
            serde_json::from_slice(&fs::read(directory.path().join("refusal.json"))?)?;
        anyhow::ensure!(refusal.get("status") == Some(&json!("failed")));
        anyhow::ensure!(
            refusal
                .pointer("/failure/message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("independent original product failure")),
            "capture custody replaced the original failure"
        );
        anyhow::ensure!(
            refusal.pointer("/failure/child_trace_files/captures/0/path") == Some(&json!(first)),
            "actual writer lost the retained raw capture path"
        );
        anyhow::ensure!(
            refusal.pointer("/failure/child_trace_files/captures/0/current_integrity")
                == Some(&json!("unverified")),
            "failure raw capture became current verified evidence"
        );
        anyhow::ensure!(
            refusal.pointer("/failure/child_trace_files/captures/1/verified_at_drain")
                == Some(&json!(false)),
            "missing reopen capture became verified"
        );
        anyhow::ensure!(
            refusal.pointer("/failure/child_trace_files/captures/0/stderr_tail/path")
                == Some(&json!(first_stderr)),
            "actual refusal writer lost the first raw stderr path"
        );
        anyhow::ensure!(
            refusal.pointer("/failure/child_trace_files/captures/0/stderr_tail/current_integrity")
                == Some(&json!("unverified")),
            "failed stderr became current verified evidence"
        );
        anyhow::ensure!(
            refusal.pointer("/failure/child_trace_files/captures/1/stderr_tail/verified_at_drain")
                == Some(&json!(false)),
            "missing reopened stderr became verified"
        );
        anyhow::ensure!(
            first.is_file()
                && first_stderr.is_file()
                && !directory.path().join("summary.json").exists()
        );
        Ok(())
    }

    #[test]
    fn late_raw_revalidation_error_reaches_actual_refusal_without_summary() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut captures = ChildTraceArtifacts::create(
            &directory.path().join("children"),
            child_trace::MAX_BYTES,
        )?;
        for (role, publication) in [("first", true), ("reopen", false)] {
            let path = captures.path(role)?;
            child_trace::write_balanced_fixture(&path)?;
            if role == "reopen" {
                let raw = fs::read_to_string(&path)?;
                fs::write(&path, raw.replace(&"11".repeat(16), &"22".repeat(16)))?;
            }
            let capture = child_trace::verify(&path, publication, child_trace::MAX_BYTES)?;
            let stderr = captures.stderr_path(role)?;
            daemon_diagnostic::write_balanced_stderr_fixture(&stderr, &capture)?;
            let diagnostic = daemon_diagnostic::verified_object_io(&stderr, &capture)?;
            captures.remember(role, publication, &capture, &diagnostic)?;
        }
        let _valid_before_mutation = captures.success_detail()?;
        let first_stderr = captures.stderr_path("first")?;
        let reopened_stderr = captures.stderr_path("reopen")?;
        fs::remove_file(&first_stderr)?;
        let measurement = NativeIndexMeasurement {
            tier: ScaleTier::Small,
            seed: 42,
            corpus_digest: "fixed fixture".into(),
            daemon_sha256: "00".repeat(32),
            native_config_sha256: "11".repeat(32),
            foreground_workload: Value::Null,
            publication_outcomes: Value::Null,
            file_count: 1,
            source_bytes: 1,
            phases: BTreeMap::new(),
            first_query_ms: 0.0,
            first_query_matched: 1,
            total_ms: 0.0,
            resource_observation: None,
            churn_cycles: 0,
            churn: None,
            child_trace_verified: true,
            child_traces: Some(captures),
            child_object_io: None,
            mixed_load: None,
        };
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567")?;
        let host = HostV1 {
            os: "linux".into(),
            arch: "aarch64".into(),
            cpu_count: 2,
            mem_bytes: 6_442_450_944,
            hostname_hash: "fixed-test-host".into(),
        };
        let error = write_index_native_artifacts(
            &measurement,
            directory.path(),
            head.clone(),
            host.clone(),
        )
        .err()
        .context("missing stderr produced a successful summary")?;
        anyhow::ensure!(
            format!("{error:#}").contains("revalidate first Native child stderr"),
            "writer did not fail at the expected raw revalidation boundary"
        );
        // Also exercise the outer trace-error binder used by the CLI: it must
        // preserve existing capture context rather than replace the writer error.
        let error = measurement.with_failure_capture_paths(error);
        let binding = native_source_binding_for_failure(ScaleTier::Small, 42)?;
        super::super::write_refusal_artifact_with_context(
            &binding,
            directory.path(),
            &head,
            &host,
            &error,
            DIMENSION,
            None,
        )?;
        let refusal: Value =
            serde_json::from_slice(&fs::read(directory.path().join("refusal.json"))?)?;
        anyhow::ensure!(
            refusal
                .pointer("/failure/message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("revalidate first Native child stderr"))
        );
        for (index, path) in [(0, first_stderr), (1, reopened_stderr)] {
            anyhow::ensure!(
                refusal.pointer(&format!(
                    "/failure/child_trace_files/captures/{index}/stderr_tail/path"
                )) == Some(&json!(path))
            );
            anyhow::ensure!(
                refusal.pointer(&format!(
                    "/failure/child_trace_files/captures/{index}/stderr_tail/current_integrity"
                )) == Some(&json!("unverified"))
            );
        }
        anyhow::ensure!(
            !directory.path().join("summary.json").exists()
                && !directory.path().join("tier_manifest.json").exists(),
            "revalidation refusal wrote a success artifact first"
        );
        Ok(())
    }

    #[test]
    fn explicit_workload_changes_only_foreground_funding() -> Result<()> {
        let fixed = workload::selected_workload_from_json(ScaleTier::Small, None)?;
        let explicit = workload::selected_workload_from_json(
            ScaleTier::Large,
            Some(
                r#"{"reads":67000000,"writes":67000000,"bytes":549755813888,"mutations":67000000}"#,
            ),
        )?;
        let mut baseline = native_config(
            Path::new("/fixture-state"),
            Path::new("/fixture.sock"),
            fixed,
        );
        let mut configured = native_config(
            Path::new("/fixture-state"),
            Path::new("/fixture.sock"),
            explicit,
        );
        anyhow::ensure!(
            required_field(&baseline, "/work_limits")?
                == &json!({"reads":1_000_000,"writes":1_000_000,"bytes":536_870_912,"mutations":1_000_000})
        );
        anyhow::ensure!(
            required_field(&configured, "/work_limits")?
                == required_field(&explicit.metadata(), "/work_limits")?
        );
        anyhow::ensure!(
            hex_sha256(&serde_json::to_vec(&baseline)?)
                != hex_sha256(&serde_json::to_vec(&configured)?)
        );
        let _fixed_limits = baseline
            .as_object_mut()
            .context("config object absent")?
            .remove("work_limits");
        let _explicit_limits = configured
            .as_object_mut()
            .context("config object absent")?
            .remove("work_limits");
        anyhow::ensure!(
            baseline == configured,
            "explicit work funding changed physical, execution, maintenance or lease limits"
        );
        Ok(())
    }

    #[test]
    fn mixed_load_requires_its_explicit_profile_before_fixture_setup() -> Result<()> {
        use std::ffi::OsStr;
        // Fixed limits are tier-independent; construct them through valid admission.
        let fixed = workload::selected_workload_from_json(ScaleTier::Small, None)?;
        let explicit = workload::selected_workload_from_json(
            ScaleTier::Large,
            Some(
                r#"{"reads":67108864,"writes":67108864,"bytes":549755813888,"mutations":67108864}"#,
            ),
        )?;
        anyhow::ensure!(!mixed_load_selection(
            None,
            None,
            ScaleTier::Small,
            1,
            fixed
        )?);
        anyhow::ensure!(mixed_load_selection(
            Some(OsStr::new("1")),
            Some(OsStr::new("1")),
            ScaleTier::Large,
            42,
            explicit
        )?);
        for (flag, sampling, tier, seed, funding) in [
            ("0", Some("1"), ScaleTier::Large, 42, explicit),
            ("1", None, ScaleTier::Large, 42, explicit),
            ("1", Some("1"), ScaleTier::Small, 42, explicit),
            ("1", Some("1"), ScaleTier::Large, 43, explicit),
            ("1", Some("1"), ScaleTier::Large, 42, fixed),
        ] {
            anyhow::ensure!(
                mixed_load_selection(
                    Some(OsStr::new(flag)),
                    sampling.map(OsStr::new),
                    tier,
                    seed,
                    funding
                )
                .is_err(),
                "unbound mixed profile was accepted"
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires an explicitly built daemon, native sampling=1 and churn cycles=20"]
    fn real_daemon_churn_and_resource_observation_preserve_product_gold() -> Result<()> {
        let _measured = functional_profile(ScaleTier::Small, 20, None, None)?;
        Ok(())
    }

    #[test]
    #[ignore = "requires an explicit daemon, sampling=1, functional tier and churn cycles"]
    fn real_daemon_selected_scale_and_churn_preserve_product_gold() -> Result<()> {
        use quanta_index_searchd::{
            DiagnosticCaptureLimitsV1, DiagnosticCaptureStatusV1, DiagnosticCaptureV1,
        };

        let raw = std::env::var("QUANTA_INDEX_NATIVE_FUNCTIONAL_TIER")
            .context("explicit QUANTA_INDEX_NATIVE_FUNCTIONAL_TIER is required")?;
        let tier = match raw.as_str() {
            "small" => ScaleTier::Small,
            "medium" => ScaleTier::Medium,
            "large" => ScaleTier::Large,
            "xlarge" => ScaleTier::Xlarge,
            _ => anyhow::bail!("functional tier must be small, medium, large or xlarge"),
        };
        let cycles = configured_churn_cycles()?;
        anyhow::ensure!(cycles > 0, "explicit functional churn cycles are required");
        match std::env::var_os("QUANTA_INDEX_NATIVE_RECORD_CAPTURE_V1") {
            None => {
                let _measured = functional_profile(tier, cycles, None, None)?;
            }
            Some(value) => {
                anyhow::ensure!(value == "1", "native record capture must be exactly 1");
                let output = tempfile::tempdir()?;
                let mut capture = DiagnosticCaptureV1::start(
                    output.path().join("record.jsonl"),
                    DiagnosticCaptureLimitsV1::default(),
                )?;
                let lease = capture.lease().context("native record lease absent")?;
                let _measured = functional_profile(
                    tier,
                    cycles,
                    Some(lease),
                    Some(&output.path().join("native-child-traces")),
                )?;
                let closed = capture.close(Duration::from_secs(5))?;
                anyhow::ensure!(
                    closed.status == DiagnosticCaptureStatusV1::Complete
                        && closed.events_dropped == 0
                        && closed.events_unwritten == 0,
                    "native harness capture incomplete"
                );
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires an explicitly built daemon, native sampling=1 and churn cycles=20"]
    fn real_daemon_small_off_record_capture_preserves_product_gold() -> Result<()> {
        use quanta_index_searchd::{
            DiagnosticCaptureLimitsV1, DiagnosticCaptureStatusV1, DiagnosticCaptureV1,
        };

        let off = functional_profile(ScaleTier::Small, 20, None, None)?;
        let output = tempfile::tempdir()?;
        let trace_path = output.path().join("native-record.jsonl");
        let mut capture =
            DiagnosticCaptureV1::start(trace_path.clone(), DiagnosticCaptureLimitsV1::default())?;
        let lease = capture
            .lease()
            .context("capture refused native record lease")?;
        let record = functional_profile(
            ScaleTier::Small,
            20,
            Some(lease),
            Some(&output.path().join("native-child-traces")),
        )?;
        let closed = capture.close(Duration::from_secs(5))?;
        anyhow::ensure!(
            closed.status == DiagnosticCaptureStatusV1::Complete,
            "native record capture incomplete: {:?}",
            closed.status
        );
        anyhow::ensure!(
            closed.events_dropped == 0,
            "native record capture dropped events"
        );
        anyhow::ensure!(
            closed.events_unwritten == 0,
            "native record capture left events unwritten"
        );

        let rows: Vec<Value> = fs::read_to_string(trace_path)?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        for name in [
            "native_full_event_ack",
            "native_reopen",
            "native_final_drain",
        ] {
            let span_id = rows
                .iter()
                .find(|row| {
                    row.pointer("/record_type").and_then(Value::as_str) == Some("event")
                        && row.pointer("/kind/type").and_then(Value::as_str)
                            == Some("phase_identity")
                        && row.pointer("/kind/phase").and_then(Value::as_str) == Some(name)
                })
                .and_then(|row| row.pointer("/kind/span_id"))
                .and_then(Value::as_u64)
                .with_context(|| format!("native record phase {name} absent"))?;
            anyhow::ensure!(
                rows.iter().any(|row| {
                    row.pointer("/record_type").and_then(Value::as_str) == Some("event")
                        && row.pointer("/kind/type").and_then(Value::as_str) == Some("span_end")
                        && row.pointer("/kind/span_id").and_then(Value::as_u64) == Some(span_id)
                        && row.pointer("/kind/outcome").and_then(Value::as_str) == Some("ok")
                }),
                "native record phase {name} did not end successfully"
            );
        }

        anyhow::ensure!(off.tier == record.tier, "capture changed tier");
        anyhow::ensure!(
            !off.child_trace_verified && record.child_trace_verified,
            "Native child capture was not independently checked in Record mode"
        );
        anyhow::ensure!(off.child_object_io.is_none(), "Off collected object IO");
        let io = record
            .child_object_io
            .as_ref()
            .context("Record object IO absent")?;
        anyhow::ensure!(
            required_field(io, "/epochs/drain/written_bytes")?
                .as_u64()
                .is_some_and(|bytes| bytes > 0),
            "actual object writes were not captured"
        );
        anyhow::ensure!(off.seed == record.seed, "capture changed seed");
        anyhow::ensure!(
            off.corpus_digest == record.corpus_digest,
            "capture changed corpus"
        );
        anyhow::ensure!(
            off.native_config_sha256 == record.native_config_sha256,
            "capture changed native config"
        );
        anyhow::ensure!(
            off.daemon_sha256 == record.daemon_sha256,
            "capture changed daemon binary"
        );
        anyhow::ensure!(
            off.foreground_workload == record.foreground_workload,
            "capture changed foreground funding"
        );
        anyhow::ensure!(
            off.file_count == record.file_count && off.source_bytes == record.source_bytes,
            "capture changed source corpus width"
        );
        anyhow::ensure!(
            off.first_query_matched == record.first_query_matched,
            "capture changed first query result"
        );
        anyhow::ensure!(
            off.churn_cycles == record.churn_cycles,
            "capture changed churn count"
        );
        let off_churn = off.churn.as_ref().context("Off churn result absent")?;
        let record_churn = record
            .churn
            .as_ref()
            .context("Record churn result absent")?;
        for key in ["requested_cycles", "completed_cycles"] {
            anyhow::ensure!(
                required_field(off_churn, &format!("/{key}"))?
                    == required_field(record_churn, &format!("/{key}"))?,
                "capture changed churn {key}"
            );
        }
        Ok(())
    }

    #[expect(
        clippy::print_stdout,
        reason = "the ignored functional driver emits its bounded diagnostic result for the external harness"
    )]
    fn functional_profile(
        tier: ScaleTier,
        cycles: u32,
        capture: Option<quanta_index_searchd::DiagnosticCaptureLeaseV1>,
        child_trace_dir: Option<&Path>,
    ) -> Result<NativeIndexMeasurement> {
        let binary = std::env::var_os("QUANTA_INDEX_SEARCHD_BIN")
            .context("explicit QUANTA_INDEX_SEARCHD_BIN is required")?;
        let binary = std::path::PathBuf::from(binary);
        anyhow::ensure!(
            binary.is_absolute() && binary.is_file(),
            "daemon must be an absolute existing binary"
        );
        anyhow::ensure!(
            ObservationMode::from_env()?.enabled(),
            "native resource sampling must be enabled"
        );
        anyhow::ensure!(
            configured_churn_cycles()? == cycles,
            "functional churn count differs from the selected profile"
        );

        // Exercise the same producer and independent query/reopen oracles as
        // the CLI, without creating a benchmark artifact attributed to a dirty
        // HEAD. Timing/resource values remain unqualified diagnostics.
        let measured = match measure_index_native_tier(
            tier,
            42,
            &binary,
            capture,
            child_trace_dir,
            67_108_864,
        ) {
            Ok(measured) => measured,
            Err(error) => {
                if let Some(prefix) = error.downcast_ref::<NativeFailurePrefix>() {
                    println!("UNQUALIFIED_FAILED_FUNCTIONAL_DIAGNOSTIC {}", prefix.phases);
                }
                return Err(error);
            }
        };
        if measured.child_trace_verified {
            // The measurement's state TempDir has already been removed. Both
            // process epochs must still replay from their artifact-owned files.
            let retained = measured
                .child_traces
                .as_ref()
                .context("verified child traces have no retained raw custody")?
                .success_detail()?;
            let captures = required_field(&retained, "/captures")?
                .as_array()
                .context("retained child capture list absent")?;
            anyhow::ensure!(
                captures.len() == 2,
                "both process epochs must remain replayable"
            );
        }
        anyhow::ensure!(
            u64::try_from(measured.file_count)? == u64::from(params_for(tier).total_files()),
            "selected corpus width changed"
        );
        let churn = measured.churn.as_ref().context("churn result absent")?;
        anyhow::ensure!(
            required_field(churn, "/completed_cycles")?.as_u64() == Some(u64::from(cycles)),
            "churn did not complete"
        );
        for phase in [
            "full_event_ack",
            "first_search",
            "delta_search",
            "noop_event_ack",
            "deleted_search",
            "old_pin_search",
            "churn_cycles",
            "churn_old_view_search",
            "reopen_read_head",
            "reopen_deleted_search",
            "reopen_churn_search",
            "reopen_search",
            "final_drain",
        ] {
            anyhow::ensure!(
                measured
                    .phases
                    .get(phase)
                    .is_some_and(|phase| phase.completed),
                "required product phase {phase} incomplete"
            );
        }
        let observation = measured
            .resource_observation
            .as_ref()
            .context("resource observation absent")?;
        anyhow::ensure!(
            required_field(observation, "/continuous_peak")?.as_bool() == Some(false),
            "sampled maximum promoted to continuous peak"
        );
        let epochs = required_field(observation, "/epochs")?
            .as_array()
            .context("resource epochs absent")?;
        anyhow::ensure!(epochs.len() == 2, "both daemon epochs must be observed");
        for epoch in epochs {
            anyhow::ensure!(
                required_field(epoch, "/worker_failure")?.is_null(),
                "resource observer failed: {epoch}"
            );
            for kind in ["child_rss", "allocated_state_root"] {
                let resource = required_field(epoch, &format!("/{kind}"))?;
                anyhow::ensure!(
                    required_field(resource, "/sample_count")?
                        .as_u64()
                        .is_some_and(|count| count > 0),
                    "{kind} has no actual samples"
                );
                anyhow::ensure!(
                    required_field(resource, "/sampled_max_bytes")?
                        .as_u64()
                        .is_some_and(|bytes| bytes > 0),
                    "{kind} has no actual byte observation"
                );
            }
            let scratch = required_field(epoch, "/allocated_native_scratch")?;
            anyhow::ensure!(
                required_field(scratch, "/sample_count")?
                    .as_u64()
                    .is_some_and(|count| count > 0)
                    && required_field(scratch, "/sampled_max_bytes")?
                        .as_u64()
                        .is_some(),
                "native scratch has no allocated-block observation"
            );
            if cfg!(target_os = "linux") {
                let aggregate = required_field(epoch, "/aggregate_process_rss")?;
                anyhow::ensure!(
                    required_field(aggregate, "/sample_count")?
                        .as_u64()
                        .is_some_and(|count| count > 0)
                        && required_field(aggregate, "/sampled_max_bytes")?
                            .as_u64()
                            .is_some_and(|bytes| bytes > 0),
                    "aggregate process RSS has no actual samples"
                );
            }
            let child_io = required_field(epoch, "/child_process_io")?;
            anyhow::ensure!(
                required_field(child_io, "/regression_error")?.is_null(),
                "child IO counter regression: {child_io}"
            );
            #[cfg(target_os = "linux")]
            {
                anyhow::ensure!(
                    required_field(child_io, "/method")? == "proc_pid_io",
                    "Linux child IO method absent"
                );
                anyhow::ensure!(
                    required_field(child_io, "/sample_count")?
                        .as_u64()
                        .is_some_and(|count| count >= 2),
                    "Linux child IO needs at least two actual samples"
                );
                anyhow::ensure!(
                    required_field(child_io, "/missing_count")? == 0
                        && required_field(child_io, "/last_missing_reason")?.is_null(),
                    "Linux child IO probe was incomplete: {child_io}"
                );
                for counter in ["read_bytes", "write_bytes", "cancelled_write_bytes"] {
                    let first = required_field(child_io, &format!("/{counter}/first"))?
                        .as_u64()
                        .with_context(|| format!("Linux child {counter} first sample absent"))?;
                    let last = required_field(child_io, &format!("/{counter}/last"))?
                        .as_u64()
                        .with_context(|| format!("Linux child {counter} last sample absent"))?;
                    let delta = required_field(child_io, &format!("/{counter}/delta"))?
                        .as_u64()
                        .with_context(|| format!("Linux child {counter} delta absent"))?;
                    anyhow::ensure!(
                        last.checked_sub(first) == Some(delta),
                        "Linux child {counter} delta differs from observed counters"
                    );
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::ensure!(
                    required_field(child_io, "/method")? == "unsupported",
                    "non-Linux child IO must be explicitly unsupported"
                );
                anyhow::ensure!(
                    required_field(child_io, "/sample_count")? == 0
                        && required_field(child_io, "/missing_count")?
                            .as_u64()
                            .is_some_and(|count| count > 0)
                        && required_field(child_io, "/last_missing_reason")?
                            .as_str()
                            .is_some_and(|reason| reason.contains("unsupported")),
                    "non-Linux child IO missing reason absent: {child_io}"
                );
                for counter in ["read_bytes", "write_bytes", "cancelled_write_bytes"] {
                    anyhow::ensure!(
                        required_field(child_io, &format!("/{counter}/first"))?.is_null()
                            && required_field(child_io, &format!("/{counter}/last"))?.is_null()
                            && required_field(child_io, &format!("/{counter}/delta"))?.is_null(),
                        "non-Linux child {counter} fabricated a numeric observation"
                    );
                }
            }
        }
        println!(
            "UNQUALIFIED_FUNCTIONAL_DIAGNOSTIC {}",
            json!({
                "daemon_sha256":measured.daemon_sha256,
                "child_trace_verified":measured.child_trace_verified,
                "child_object_io":measured.child_object_io,
                "mixed_load":measured.mixed_load,
                "tier":tier.as_str(),
                "seed":measured.seed,
                "corpus_digest":measured.corpus_digest,
                "native_config_sha256":measured.native_config_sha256,
                "file_count":measured.file_count,
                "source_bytes":measured.source_bytes,
                "phases":phase_json(&measured.phases),
                "publication_outcomes":measured.publication_outcomes,
                "finite_churn":churn,
                "native_resource_observation":observation,
                "scope":"product functional gold; no clean-HEAD benchmark attribution or resource SLO"
            })
        );
        Ok(measured)
    }
}
