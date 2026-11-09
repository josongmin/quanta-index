//! Fixed Native UDS mixed-load point over the existing offered-arrival scheduler.
//! The returned rows are observations; sampled resources are not continuous peaks.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, TryLockError,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use quanta_index_contract::{
    index::{
        CatalogView, ObjectRef, OwnerKey, OwnerMutation, SourceTextAdmission, ViewRef,
        ViewSelection,
    },
    native_protocol::NativeTextResult,
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::native_uds::NativeUdsClient;
use serde_json::{Value, json};

use crate::offered_load::{
    self, ArrivalModel, OfferedConfig, OfferedProgress, OfferedRun, OfferedTask,
};

use super::super::{ScopedFile, repo_query_token};
#[path = "mixed_snapshot.rs"]
mod mixed_snapshot;
use mixed_snapshot::{QuerySnapshot, SnapshotReads};

use super::retained_read::{
    HeldRead, QUERY_DEADLINE, retained_witness, session_key, text_query, verify_witness,
};
use super::{
    FIRST_PATH, PublicationAudit, PublicationLabel, ScaleTier, file_query_token, operation, owner,
    params_for, prepare_source_advance, publication, text_spec,
};

const WARMUP: Duration = Duration::from_secs(60);
const MEASURE: Duration = Duration::from_secs(900);
const TOTAL: Duration = Duration::from_secs(WARMUP.as_secs() + MEASURE.as_secs());
const PUBLISH_PERIOD: Duration = Duration::from_secs(1);
const SOURCE_BOUND: usize = 16 * 1024;
const RATE_QPS: u32 = 50;
const WORKERS: usize = 4;
const QUEUE: usize = 64;
const FAILURE_PUBLISHER: u8 = 1;
const FAILURE_HELD: u8 = 2;
const FAILURE_SCHEDULER: u8 = 3;

#[derive(Clone)]
pub(super) struct MixedStart {
    pub(super) view: CatalogView,
    pub(super) first_ref: Option<ObjectRef>,
    pub(super) first_token: Option<String>,
    pub(super) next_operation_sequence: u64,
    pub(super) next_event_sequence: u64,
    pub(super) transition_step: u64,
}

impl MixedStart {
    fn advanced(
        &self,
        view: CatalogView,
        first_ref: Option<ObjectRef>,
        first_token: Option<String>,
    ) -> Result<Self> {
        ensure!(
            first_ref.is_some() == first_token.is_some(),
            "mixed next owner binding is inconsistent"
        );
        ensure!(
            view.identity.workspace == self.view.identity.workspace
                && view.identity.epoch == self.view.identity.epoch,
            "mixed next view scope differs"
        );
        ensure!(
            view.identity.sequence
                == self
                    .view
                    .identity
                    .sequence
                    .checked_add(1)
                    .context("mixed view sequence overflow")?,
            "mixed next view sequence differs"
        );
        Ok(Self {
            view,
            first_ref,
            first_token,
            next_operation_sequence: self
                .next_operation_sequence
                .checked_add(1)
                .context("mixed operation sequence overflow")?,
            next_event_sequence: self
                .next_event_sequence
                .checked_add(1)
                .context("mixed event sequence overflow")?,
            transition_step: self
                .transition_step
                .checked_add(1)
                .context("mixed transition step overflow")?,
        })
    }
}

#[cfg(test)]
fn fixture_catalog_view(identity: ViewRef) -> CatalogView {
    use quanta_index_contract::index::{Digest, MapRoot, SourceCut};

    CatalogView {
        identity,
        profile: ObjectRef(Digest([11; 32])),
        source: SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        projections: MapRoot(None),
        members: MapRoot(None),
        coverage: MapRoot(None),
    }
}

pub(super) struct MixedOutcome {
    pub(super) final_view: ViewRef,
    pub(super) first_ref: Option<ObjectRef>,
    pub(super) first_token: Option<String>,
    pub(super) detail: Value,
}

#[derive(Debug)]
pub(super) struct MixedFailurePrefix {
    pub(super) detail: Value,
}

impl std::fmt::Display for MixedFailurePrefix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("native mixed-load stopped with partial progress")
    }
}

impl std::error::Error for MixedFailurePrefix {}

#[derive(Clone, Copy)]
enum QueryClass {
    Unique,
    Broad,
    Miss,
}

impl QueryClass {
    fn for_id(id: u64) -> Self {
        match id % 10 {
            0..=6 => Self::Unique,
            7..=8 => Self::Broad,
            _ => Self::Miss,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Unique => "unique",
            Self::Broad => "broad_top20",
            Self::Miss => "miss",
        }
    }
}

struct QueryCompletion {
    class: QueryClass,
    latency_ms: f64,
    queue_ms: f64,
    failure_stage: Option<&'static str>,
    result: Result<()>,
}

#[derive(Default)]
struct QueryProgress {
    dispatched: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
    first_failure: Mutex<Option<Value>>,
}

impl QueryProgress {
    fn record_failure(&self, task: OfferedTask, completion: &QueryCompletion) {
        let Some(error) = completion.result.as_ref().err() else {
            return;
        };
        if let Ok(mut first) = self.first_failure.lock() {
            if first.is_none() {
                let message: String = format!("{error:#}").chars().take(512).collect();
                *first = Some(json!({
                    "task_id":task.id,
                    "class":completion.class.name(),
                    "stage":completion.failure_stage,
                    "error":message,
                    "queue_ms":completion.queue_ms,
                    "latency_ms":completion.latency_ms,
                }));
            }
        }
    }

    fn detail(&self) -> Value {
        let first_failure = self.first_failure.lock().map_or_else(
            |_poison| json!({"capture_error":"mixed query progress poisoned"}),
            |first| json!(&*first),
        );
        json!({
            "dispatched":self.dispatched.load(Ordering::Acquire),
            "completed":self.completed.load(Ordering::Acquire),
            "failed":self.failed.load(Ordering::Acquire),
            "first_failure":first_failure,
        })
    }
}

#[derive(Default)]
struct PublisherWindowProgress {
    scheduled: u64,
    attempted: u64,
    acked: u64,
    published: u64,
    missed_late: u64,
    last_operation_sequence: Option<u64>,
    last_error: Option<String>,
    records: Vec<Value>,
    freshness_ms: Vec<f64>,
    old_witness_ms: Vec<f64>,
}

#[derive(Default)]
struct TickTimings {
    input_ready: Option<f64>,
    mutation_ready: Option<f64>,
    ack: Option<f64>,
    freshness: Option<f64>,
    old_witness: Option<f64>,
    failure_stage: Option<&'static str>,
}

impl TickTimings {
    fn detail(&self) -> Value {
        json!({
            "input_ready_from_scheduled_ms":self.input_ready,
            "mutation_ready_from_scheduled_ms":self.mutation_ready,
            "ack_from_scheduled_ms":self.ack,
            "current_gold_from_scheduled_ms":self.freshness,
            "old_view_witness_ms":self.old_witness,
            "failure_stage":self.failure_stage,
        })
    }
}

impl PublisherWindowProgress {
    fn skip_expired_tick(&mut self, now: Instant, deadline: Instant) -> Result<bool> {
        if now < deadline {
            return Ok(false);
        }
        self.missed_late = self
            .missed_late
            .checked_add(1)
            .context("mixed late count overflow")?;
        Ok(true)
    }

    fn detail(&self) -> Value {
        let mut freshness = self.freshness_ms.clone();
        freshness.sort_by(f64::total_cmp);
        let mut old = self.old_witness_ms.clone();
        old.sort_by(f64::total_cmp);
        json!({
            "scheduled": self.scheduled,
            "attempted": self.attempted,
            "acked": self.acked,
            "published": self.published,
            "missed_late": self.missed_late,
            "last_operation_sequence": self.last_operation_sequence,
            "last_error": self.last_error,
            "publication_records": self.records,
            "current_gold_from_scheduled_p95_ms":percentile_opt(&freshness,95),
            "current_gold_from_scheduled_p99_ms":percentile_opt(&freshness,99),
            "old_view_witness_p95_ms":percentile_opt(&old,95),
            "old_view_witness_p99_ms":percentile_opt(&old,99),
        })
    }
}

#[derive(Default)]
struct PublisherProgress {
    warmup: PublisherWindowProgress,
    measure: PublisherWindowProgress,
    failure_stage: Option<&'static str>,
    failure_error: Option<String>,
}

impl PublisherProgress {
    fn for_tick(&mut self, tick: u64) -> &mut PublisherWindowProgress {
        if tick < WARMUP.as_secs() {
            &mut self.warmup
        } else {
            &mut self.measure
        }
    }

    fn record_reached_tick(&mut self, tick: u64) -> Result<()> {
        let window = self.for_tick(tick);
        window.scheduled = window
            .scheduled
            .checked_add(1)
            .context("mixed scheduled count overflow")?;
        Ok(())
    }

    fn detail(&self) -> Value {
        json!({"warmup":self.warmup.detail(),"measure":self.measure.detail(),
            "failure_stage":self.failure_stage,"failure_error":self.failure_error})
    }
}

fn record_publisher_failure(
    progress: &Mutex<PublisherProgress>,
    stage: &'static str,
    error: &anyhow::Error,
) {
    if let Ok(mut progress) = progress.lock() {
        if progress.failure_stage.is_none() {
            progress.failure_stage = Some(stage);
            progress.failure_error = Some(format!("{error:#}").chars().take(512).collect());
        }
    }
}

fn validate_publisher_counts(progress: &PublisherWindowProgress, duration: Duration) -> Result<()> {
    ensure!(
        progress.scheduled == duration.as_secs(),
        "mixed publisher scheduled count incomplete"
    );
    ensure!(
        progress
            .attempted
            .checked_add(progress.missed_late)
            .context("mixed publisher denominator overflow")?
            == progress.scheduled,
        "mixed publisher lost a scheduled attempt"
    );
    ensure!(
        progress.missed_late == 0,
        "mixed publisher missed a scheduled update"
    );
    ensure!(
        progress.acked == progress.attempted,
        "mixed publisher did not ACK every attempt"
    );
    ensure!(
        progress.published == progress.acked,
        "mixed publisher did not verify every ACK"
    );
    Ok(())
}

fn lock_snapshot(snapshot: &Mutex<SnapshotReads>) -> Result<Arc<QuerySnapshot>> {
    snapshot
        .lock()
        .map_err(|error| anyhow!("mixed snapshot poisoned: {error}"))?
        .capture()
}

fn capture_query_snapshot(
    snapshot: &Mutex<SnapshotReads>,
    budget: &RequestBudgetV1,
) -> Result<Arc<QuerySnapshot>> {
    loop {
        budget.checkpoint("mixed snapshot acquisition")?;
        match snapshot.try_lock() {
            Ok(reads) => return reads.capture(),
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(1)),
            Err(TryLockError::Poisoned(error)) => {
                return Err(anyhow!("mixed snapshot poisoned: {error}"));
            }
        }
    }
}

fn lock_progress(
    progress: &Mutex<PublisherProgress>,
) -> Result<std::sync::MutexGuard<'_, PublisherProgress>> {
    progress
        .lock()
        .map_err(|error| anyhow!("mixed publisher progress poisoned: {error}"))
}

fn query_budget(task: OfferedTask, timeout: Duration) -> Result<RequestBudgetV1> {
    let deadline = task
        .scheduled
        .checked_add(timeout)
        .context("mixed query deadline overflow")?;
    ensure!(
        Instant::now() < deadline,
        "mixed query expired before dispatch"
    );
    Ok(RequestBudgetV1::until(deadline))
}

fn search(
    client: &NativeUdsClient,
    view: ViewRef,
    token: &str,
    top_k: u32,
    budget: &RequestBudgetV1,
) -> Result<NativeTextResult> {
    Ok(client.search_text(
        ViewSelection::Exact(view),
        text_spec().instance,
        text_query(token, top_k),
        budget,
    )?)
}

fn file_identity(file: &ScopedFile) -> String {
    format!("{}/{}", file.source_repo_id, file.repo_relative_path)
}

struct UniqueFixture {
    token: String,
    path: String,
    owner: OwnerKey,
    source: ObjectRef,
}

#[derive(Clone)]
struct BroadRow {
    path: String,
    source: ObjectRef,
}

struct QueryFixture {
    unique: Vec<UniqueFixture>,
    broad: Vec<BTreeMap<OwnerKey, BroadRow>>,
    broad_ranked: Vec<Vec<(u64, OwnerKey)>>,
    first_indexed_tokens: u64,
    missing_prefix: String,
}

fn source_token_stats(source: &str, repo_token: &str, file_token: &str) -> Result<(u64, u64, u64)> {
    ensure!(
        source.is_ascii(),
        "mixed rank oracle requires ASCII fixture source"
    );
    let bytes = source.as_bytes();
    let mut start = None;
    let mut indexed = 0_u64;
    let mut repo_frequency = 0_u64;
    let mut file_frequency = 0_u64;
    for (position, byte) in bytes
        .iter()
        .enumerate()
        .chain(std::iter::once((bytes.len(), &b' ')))
    {
        let is_token = byte.is_ascii_alphanumeric() || *byte == b'_';
        match (start, is_token) {
            (None, true) => start = Some(position),
            (Some(begin), false) => {
                let run = source
                    .get(begin..position)
                    .context("mixed ASCII run absent")?;
                ensure!(run.len() <= 256, "mixed source has overlong run");
                indexed = indexed
                    .checked_add(1)
                    .context("mixed indexed-token count overflow")?;
                if run == repo_token {
                    repo_frequency = repo_frequency
                        .checked_add(1)
                        .context("mixed repo frequency overflow")?;
                }
                if run == file_token {
                    file_frequency = file_frequency
                        .checked_add(1)
                        .context("mixed file frequency overflow")?;
                }
                start = None;
            }
            _ => {}
        }
    }
    Ok((indexed, repo_frequency, file_frequency))
}

impl QueryFixture {
    fn from_source(
        corpus: &[ScopedFile],
        refs: &[ObjectRef],
        repo_count: u32,
        seed: u64,
    ) -> Result<Self> {
        ensure!(
            corpus.len() == refs.len() && corpus.len() >= 2,
            "mixed source/ref count differs"
        );
        let missing_prefix = format!("qidxmixedmissing{seed:016x}id");
        let mut broad = (0..repo_count).map(|_| BTreeMap::new()).collect::<Vec<_>>();
        let mut broad_ranked = (0..repo_count).map(|_| Vec::new()).collect::<Vec<_>>();
        let mut unique = Vec::with_capacity(corpus.len().saturating_sub(1));
        let mut first_indexed_tokens = 0_u64;
        for (index, file) in corpus.iter().enumerate() {
            ensure!(
                !file.content.contains(&missing_prefix),
                "mixed miss prefix occurs in source"
            );
            let repo = file
                .source_repo_id
                .strip_prefix("repo")
                .context("mixed repo ID invalid")?
                .parse::<u32>()?;
            ensure!(repo < repo_count, "mixed source repository outside tier");
            let file_index = file
                .repo_relative_path
                .strip_prefix("src/file_")
                .and_then(|part| part.strip_suffix(".rs"))
                .context("mixed file path invalid")?
                .parse::<u32>()?;
            let token = file_query_token(repo, file_index);
            let path = file_identity(file);
            ensure!(
                file.content.contains(&token),
                "mixed unique token absent from source"
            );
            let repo_token = repo_query_token(repo);
            let (indexed_tokens, repo_frequency, file_frequency) =
                source_token_stats(&file.content, &repo_token, &token)?;
            ensure!(
                repo_frequency == 1 && file_frequency == 1,
                "mixed source anchors are not unique terms"
            );
            let source = *refs.get(index).context("mixed source ref absent")?;
            if index == 0 {
                ensure!(
                    repo == 0 && file_index == 0 && path == FIRST_PATH,
                    "mixed first source shape differs"
                );
                first_indexed_tokens = indexed_tokens;
            } else {
                unique.push(UniqueFixture {
                    token,
                    path: path.clone(),
                    owner: owner(index)?,
                    source,
                });
                let rows = broad
                    .get_mut(usize::try_from(repo)?)
                    .context("mixed repo index outside tier")?;
                ensure!(
                    rows.insert(owner(index)?, BroadRow { path, source })
                        .is_none(),
                    "mixed broad fixture owner duplicated"
                );
                broad_ranked
                    .get_mut(usize::try_from(repo)?)
                    .context("mixed rank repo absent")?
                    .push((indexed_tokens, owner(index)?));
            }
        }
        for ranked in &mut broad_ranked {
            ranked.sort_unstable();
        }
        ensure!(!unique.is_empty(), "mixed source lacks stable files");
        Ok(Self {
            unique,
            broad,
            broad_ranked,
            first_indexed_tokens,
            missing_prefix,
        })
    }
}

fn verify_unique(page: &NativeTextResult, view: ViewRef, expected: &UniqueFixture) -> Result<()> {
    let hit = page.hits.first().context("mixed unique hit absent")?;
    ensure!(
        page.view == view && page.exhaustive,
        "mixed unique coverage/view differs"
    );
    ensure!(
        page.matched == 1 && page.hits.len() == 1,
        "mixed unique count differs"
    );
    ensure!(hit.path == expected.path, "mixed unique path differs");
    ensure!(
        hit.owner == expected.owner && hit.source == expected.source,
        "mixed unique owner/source differs"
    );
    Ok(())
}

fn verify_broad(
    page: &NativeTextResult,
    view: ViewRef,
    repo: u32,
    snapshot: &MixedStart,
    fixture: &QueryFixture,
) -> Result<()> {
    let expected = fixture
        .broad
        .get(usize::try_from(repo)?)
        .context("mixed broad repo absent")?;
    let first = if repo == 0 { snapshot.first_ref } else { None };
    let first_owner = first.map(|_| owner(0)).transpose()?;
    let mut ranked = fixture
        .broad_ranked
        .get(usize::try_from(repo)?)
        .context("mixed broad rank repo absent")?
        .clone();
    if let Some(first_owner) = first_owner {
        let token = snapshot
            .first_token
            .as_deref()
            .context("mixed live first owner has no token")?;
        let indexed_tokens = if token == file_query_token(0, 0) {
            fixture.first_indexed_tokens
        } else {
            fixture
                .first_indexed_tokens
                .checked_add(1)
                .context("mixed changed first source token count overflow")?
        };
        let position = match ranked.binary_search(&(indexed_tokens, first_owner)) {
            Ok(position) | Err(position) => position,
        };
        ranked.insert(position, (indexed_tokens, first_owner));
    } else {
        ensure!(
            snapshot.first_token.is_none() || repo != 0,
            "mixed deleted first owner retained token"
        );
    }
    let expected_count = u64::try_from(expected.len())?
        .checked_add(u64::from(first.is_some()))
        .context("mixed broad count overflow")?;
    ensure!(
        page.view == view && page.exhaustive,
        "mixed broad coverage/view differs"
    );
    ensure!(
        page.matched == expected_count,
        "mixed broad matched count differs"
    );
    ensure!(
        page.hits.len() == ranked.len().min(20),
        "mixed broad top20 size differs"
    );
    let mut seen = BTreeSet::new();
    let mut previous: Option<&quanta_index_contract::native_protocol::NativeTextHit> = None;
    for (position, hit) in page.hits.iter().enumerate() {
        let expected_owner = ranked
            .get(position)
            .context("mixed broad rank row absent")?
            .1;
        ensure!(
            hit.owner == expected_owner,
            "mixed broad top20 omitted a higher-ranked fixture owner"
        );
        if Some(hit.owner) == first_owner {
            ensure!(
                hit.path == FIRST_PATH && Some(hit.source) == first,
                "mixed broad first path/source differs"
            );
        } else {
            let row = expected
                .get(&hit.owner)
                .context("mixed broad unexpected owner")?;
            ensure!(
                hit.path == row.path && hit.source == row.source,
                "mixed broad path/source differs"
            );
        }
        ensure!(seen.insert(hit.owner), "mixed broad duplicate owner");
        if let Some(prior) = previous {
            let order = prior.score.get().total_cmp(&hit.score.get());
            ensure!(order.is_ge(), "mixed broad score increased");
            if order.is_eq() {
                ensure!(prior.owner < hit.owner, "mixed broad tie order differs");
            }
        }
        previous = Some(hit);
    }
    Ok(())
}

fn execute_query(
    task: OfferedTask,
    timeout: Duration,
    socket: &Path,
    fixture: &QueryFixture,
    snapshot: &Mutex<SnapshotReads>,
) -> QueryCompletion {
    let class = QueryClass::for_id(task.id);
    let mut stage = "deadline";
    let started = Instant::now();
    let queue_ms = started
        .saturating_duration_since(task.scheduled)
        .as_secs_f64()
        * 1_000.0;
    let result = (|| {
        let budget = query_budget(task, timeout)?;
        stage = "snapshot_capture";
        let captured = capture_query_snapshot(snapshot, &budget)?;
        let state = &captured.state;
        let client = NativeUdsClient::new(socket.to_path_buf());
        match class {
            QueryClass::Unique => {
                stage = "unique_fixture";
                let index = usize::try_from(
                    task.id
                        .checked_rem(u64::try_from(fixture.unique.len())?)
                        .context("mixed unique fixture count is zero")?,
                )?;
                let expected = fixture
                    .unique
                    .get(index)
                    .context("mixed unique fixture absent")?;
                stage = "unique_read_session_search";
                let page = captured.search(&client, &expected.token, 1, &budget)?;
                stage = "unique_verify";
                verify_unique(&page, state.view.identity, expected)?;
            }
            QueryClass::Broad => {
                stage = "broad_fixture";
                let repo = u32::try_from(
                    task.id
                        .checked_rem(u64::try_from(fixture.broad.len())?)
                        .context("mixed broad fixture count is zero")?,
                )?;
                let token = repo_query_token(repo);
                stage = "broad_read_session_search";
                let page = captured.search(&client, &token, 20, &budget)?;
                stage = "broad_verify";
                verify_broad(&page, state.view.identity, repo, state, fixture)?;
            }
            QueryClass::Miss => {
                let token = format!("{}{:016x}", fixture.missing_prefix, task.id);
                stage = "miss_read_session_search";
                let page = captured.search(&client, &token, 1, &budget)?;
                stage = "miss_verify";
                ensure!(
                    page.view == state.view.identity && page.exhaustive,
                    "mixed miss coverage/view differs"
                );
                ensure!(
                    page.matched == 0 && page.hits.is_empty(),
                    "mixed miss returned hits"
                );
            }
        }
        stage = "completion_deadline";
        ensure!(
            Instant::now()
                <= task
                    .scheduled
                    .checked_add(timeout)
                    .context("mixed query deadline overflow")?,
            "mixed query exceeded absolute deadline"
        );
        Ok(())
    })();
    QueryCompletion {
        class,
        latency_ms: Instant::now()
            .saturating_duration_since(task.scheduled)
            .as_secs_f64()
            * 1_000.0,
        queue_ms,
        failure_stage: result.as_ref().err().map(|_| stage),
        result,
    }
}

fn witness(
    client: &NativeUdsClient,
    view: ViewRef,
    token: Option<&str>,
    expected_ref: Option<ObjectRef>,
) -> Result<()> {
    let selected = token.unwrap_or("scalefileneedle000file00000");
    let page = search(
        client,
        view,
        selected,
        1,
        &RequestBudgetV1::for_duration(QUERY_DEADLINE),
    )?;
    verify_witness(&page, view, token, expected_ref)
}

struct PlannedSource {
    content: Option<String>,
    token: Option<String>,
}

fn planned_source(step: u64, sequence: u64, original: &str) -> Result<PlannedSource> {
    let planned = match step % 3 {
        0 => {
            let token = format!("qidxmixedneedle{sequence}");
            PlannedSource {
                content: Some(format!("{original}\n// {token}\n")),
                token: Some(token),
            }
        }
        1 => PlannedSource {
            content: None,
            token: None,
        },
        _ => PlannedSource {
            content: Some(original.to_owned()),
            token: Some(file_query_token(0, 0)),
        },
    };
    if let Some(content) = &planned.content {
        ensure!(
            content.len() <= SOURCE_BOUND,
            "mixed staged source exceeds 16 KiB"
        );
    }
    Ok(planned)
}

fn publish_tick(
    client: &NativeUdsClient,
    original: &str,
    captured: &QuerySnapshot,
    audit: &mut PublicationAudit,
    scheduled: Instant,
    timings: &mut TickTimings,
) -> Result<MixedStart> {
    let current = &captured.state;
    let sequence = current.next_operation_sequence;
    let event = current.next_event_sequence;
    let key = operation(sequence);
    timings.failure_stage = Some("begin_input");
    let ingress = client.begin_input(key, &super::budget())?;
    timings.failure_stage = Some("plan_source");
    let plan = planned_source(current.transition_step, sequence, original)?;
    let next_ref = if let Some(content) = &plan.content {
        timings.failure_stage = Some("stage_source");
        let reference = client.stage_source(
            ingress,
            FIRST_PATH.into(),
            content.as_bytes(),
            SourceTextAdmission::Utf8 {
                policy: quanta_index_contract::index::Digest([31; 32]),
            },
            &super::budget(),
        )?;
        Some(reference)
    } else {
        None
    };
    timings.input_ready = Some(
        Instant::now()
            .saturating_duration_since(scheduled)
            .as_secs_f64()
            * 1_000.0,
    );
    // This same snapshot session also protects concurrent offered queries.
    let prior = &captured.session;
    timings.failure_stage = Some("prepare_source_advance");
    let request = prepare_source_advance(
        client,
        ingress,
        &current.view,
        [OwnerMutation {
            owner: owner(0)?,
            expected: current.first_ref,
            next: next_ref,
        }],
        event,
    )?;
    timings.mutation_ready = Some(
        Instant::now()
            .saturating_duration_since(scheduled)
            .as_secs_f64()
            * 1_000.0,
    );
    timings.failure_stage = Some("submit_source_advance");
    let view = publication::publish(
        client,
        request,
        PublicationLabel {
            phase: "mixed_load",
            cycle: None,
        },
        audit,
    )
    .with_context(|| format!("mixed publication key {key:?} seq={sequence}"))?;
    timings.ack = Some(
        Instant::now()
            .saturating_duration_since(scheduled)
            .as_secs_f64()
            * 1_000.0,
    );
    timings.failure_stage = Some("ack_identity_validation");
    ensure!(
        view.workspace == current.view.identity.workspace
            && view.epoch == current.view.identity.epoch,
        "mixed publication workspace/epoch differs"
    );
    ensure!(
        view.sequence
            == current
                .view
                .identity
                .sequence
                .checked_add(1)
                .context("mixed view sequence overflow")?,
        "mixed publication sequence did not advance one"
    );
    timings.failure_stage = Some("current_head_read");
    let latest = client.read_view(
        ViewSelection::Latest {
            workspace: view.workspace,
            epoch: view.epoch,
        },
        &RequestBudgetV1::for_duration(QUERY_DEADLINE),
    )?;
    timings.failure_stage = Some("current_head_verify");
    ensure!(latest.identity == view, "mixed ACK is not the current head");
    timings.failure_stage = Some("current_witness");
    let next = current.advanced(latest, next_ref, plan.token)?;
    witness(
        client,
        next.view.identity,
        next.first_token.as_deref(),
        next.first_ref,
    )?;
    timings.freshness = Some(
        Instant::now()
            .saturating_duration_since(scheduled)
            .as_secs_f64()
            * 1_000.0,
    );
    let old_started = Instant::now();
    timings.failure_stage = Some("old_view_witness");
    retained_witness(
        client,
        prior,
        current.first_token.as_deref(),
        current.first_ref,
        &RequestBudgetV1::for_duration(QUERY_DEADLINE),
    )?;
    timings.old_witness = Some(old_started.elapsed().as_secs_f64() * 1_000.0);
    timings.failure_stage = None;
    Ok(next)
}

fn open_snapshot(
    client: &NativeUdsClient,
    state: &MixedStart,
) -> Result<quanta_index_contract::native_protocol::NativeReadSessionStatus> {
    Ok(client.open_read_session(
        ViewSelection::Exact(state.view.identity),
        session_key(state.view.identity, b"queries"),
        &RequestBudgetV1::for_duration(QUERY_DEADLINE),
    )?)
}

fn release_snapshot(
    client: &NativeUdsClient,
    session: &quanta_index_contract::native_protocol::NativeReadSessionStatus,
) -> Result<()> {
    let (_released, _remaining) =
        client.release_read_session(session, &RequestBudgetV1::for_duration(QUERY_DEADLINE))?;
    Ok(())
}

fn wait_tick(deadline: Instant, cancel: &AtomicBool) -> Result<()> {
    while Instant::now() < deadline {
        ensure!(!cancel.load(Ordering::Acquire), "mixed publisher cancelled");
        thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50)),
        );
    }
    ensure!(!cancel.load(Ordering::Acquire), "mixed publisher cancelled");
    Ok(())
}

fn tick_bounds(origin: Instant, tick: u64) -> Result<(Instant, Instant)> {
    let scheduled = origin
        .checked_add(
            PUBLISH_PERIOD
                .checked_mul(u32::try_from(tick)?)
                .context("mixed tick offset overflow")?,
        )
        .context("mixed tick instant overflow")?;
    let deadline = scheduled
        .checked_add(PUBLISH_PERIOD)
        .context("mixed tick deadline overflow")?;
    Ok((scheduled, deadline))
}

#[derive(Default)]
struct HeldWorkerProgress {
    last_tick: Option<u64>,
    attempted: u64,
    missed_late: u64,
    settled: bool,
    current_stage: &'static str,
    failure_stage: Option<&'static str>,
    failure_error: Option<String>,
}

impl HeldWorkerProgress {
    fn detail(&self) -> Value {
        json!({
            "last_tick":self.last_tick,
            "attempted":self.attempted,
            "missed_late":self.missed_late,
            "settled":self.settled,
            "failure_stage":self.failure_stage,
            "failure_error":self.failure_error,
        })
    }
}

fn record_held_failure(progress: &Mutex<HeldWorkerProgress>, error: &anyhow::Error) {
    if let Ok(mut progress) = progress.lock() {
        progress.failure_stage = Some(progress.current_stage);
        progress.failure_error = Some(format!("{error:#}").chars().take(512).collect());
    }
}

fn held_worker(
    socket: PathBuf,
    origin: Instant,
    held: &Mutex<HeldRead>,
    progress: &Mutex<HeldWorkerProgress>,
    cancel: &AtomicBool,
) -> Result<()> {
    let client = NativeUdsClient::new(socket);
    for tick in 0..TOTAL.as_secs() {
        let (scheduled, deadline) = tick_bounds(origin, tick)?;
        {
            let mut progress = progress
                .lock()
                .map_err(|error| anyhow!("mixed held progress poisoned: {error}"))?;
            progress.last_tick = Some(tick);
            progress.current_stage = "wait_tick";
        }
        wait_tick(scheduled, cancel)?;
        let mut progress_guard = progress
            .lock()
            .map_err(|error| anyhow!("mixed held progress poisoned: {error}"))?;
        if Instant::now() >= deadline {
            progress_guard.missed_late = progress_guard
                .missed_late
                .checked_add(1)
                .context("mixed held late count overflow")?;
            continue;
        }
        progress_guard.current_stage = "held_lock";
        drop(progress_guard);
        let mut retained = held
            .lock()
            .map_err(|error| anyhow!("mixed held read poisoned: {error}"))?;
        let mut progress_guard = progress
            .lock()
            .map_err(|error| anyhow!("mixed held progress poisoned: {error}"))?;
        if Instant::now() >= deadline {
            progress_guard.missed_late = progress_guard
                .missed_late
                .checked_add(1)
                .context("mixed held late count overflow")?;
            continue;
        }
        progress_guard.attempted = progress_guard
            .attempted
            .checked_add(1)
            .context("mixed held attempt count overflow")?;
        progress_guard.current_stage = "held_tick";
        drop(progress_guard);
        // Renewal, its witness, and release/settlement all share this exact
        // scheduled second. A delayed worker never receives a fresh budget.
        retained.tick(&client, &RequestBudgetV1::until(deadline))?;
        if retained.is_settled() {
            retained.complete()?;
            drop(retained);
            progress
                .lock()
                .map_err(|error| anyhow!("mixed held progress poisoned: {error}"))?
                .settled = true;
            return Ok(());
        }
    }
    held.lock()
        .map_err(|error| anyhow!("mixed held read poisoned: {error}"))?
        .complete()
        .context("mixed held lifecycle did not settle by load end")
}

struct MixedWorkers {
    publisher: Option<JoinHandle<Result<()>>>,
    held: Option<JoinHandle<Result<()>>>,
    publisher_start: Option<mpsc::Sender<Instant>>,
    held_start: Option<mpsc::Sender<Instant>>,
    cancel: Arc<AtomicBool>,
}

impl MixedWorkers {
    fn start(&mut self, origin: Instant, held: &Mutex<HeldRead>) -> Result<()> {
        held.lock()
            .map_err(|error| anyhow!("mixed held read poisoned: {error}"))?
            .bind_load_start(origin)?;
        self.signal_start(origin)
    }

    fn signal_start(&mut self, origin: Instant) -> Result<()> {
        self.publisher_start
            .take()
            .context("mixed publisher start sender absent")?
            .send(origin)
            .context("mixed publisher start receiver closed")?;
        self.held_start
            .take()
            .context("mixed held start sender absent")?
            .send(origin)
            .context("mixed held start receiver closed")?;
        Ok(())
    }

    fn join_all(&mut self) -> (Result<()>, Result<()>) {
        drop(self.publisher_start.take());
        drop(self.held_start.take());
        let publisher = self
            .publisher
            .take()
            .context("mixed publisher worker absent")
            .and_then(|worker| {
                worker
                    .join()
                    .map_err(|panic| anyhow!("mixed publisher panicked: {panic:?}"))?
            });
        let held = self
            .held
            .take()
            .context("mixed held worker absent")
            .and_then(|worker| {
                worker
                    .join()
                    .map_err(|panic| anyhow!("mixed held worker panicked: {panic:?}"))?
            });
        (publisher, held)
    }
}

impl Drop for MixedWorkers {
    fn drop(&mut self) {
        if self.publisher.is_some() || self.held.is_some() {
            self.cancel.store(true, Ordering::Release);
            drop(self.join_all());
        }
    }
}

fn mark_failure(owner: &AtomicU8, role: u8, cancel: &AtomicBool) {
    // Offered-load worker panic cancels its siblings before the scheduler
    // returns. A sibling observing that cancellation cannot claim first cause.
    let observed_role = if role != FAILURE_SCHEDULER && cancel.load(Ordering::Acquire) {
        FAILURE_SCHEDULER
    } else {
        role
    };
    let _first = owner.compare_exchange(0, observed_role, Ordering::AcqRel, Ordering::Acquire);
    cancel.store(true, Ordering::Release);
}

fn publisher(
    socket: PathBuf,
    original: &str,
    origin: Instant,
    snapshot: &Mutex<SnapshotReads>,
    progress: &Mutex<PublisherProgress>,
    cancel: &AtomicBool,
) -> Result<()> {
    let client = NativeUdsClient::new(socket);
    for tick in 0..TOTAL.as_secs() {
        let (scheduled, tick_deadline) = tick_bounds(origin, tick)?;
        wait_tick(scheduled, cancel)?;
        // A reached tick remains in the denominator even if pre-publication
        // maintenance fails before an attempt can start. Overdue ticks never
        // dispatch maintenance with an already expired request budget.
        {
            let mut progress_lock = lock_progress(progress)?;
            progress_lock.record_reached_tick(tick)?;
            if progress_lock
                .for_tick(tick)
                .skip_expired_tick(Instant::now(), tick_deadline)?
            {
                continue;
            }
        }
        SnapshotReads::reap_shared(snapshot, |session| {
            release_snapshot(&client, session).context("retired snapshot release")
        })
        .map_err(|error| {
            record_publisher_failure(progress, "retired_snapshot_reap", &error);
            error
        })?;
        let captured = lock_snapshot(snapshot).map_err(|error| {
            record_publisher_failure(progress, "snapshot_capture", &error);
            error
        })?;
        let state = &captured.state;
        let mut progress_lock = lock_progress(progress)?;
        let progress_guard = progress_lock.for_tick(tick);
        if progress_guard.skip_expired_tick(Instant::now(), tick_deadline)? {
            drop(progress_lock);
            continue;
        }
        progress_guard.attempted = progress_guard
            .attempted
            .checked_add(1)
            .context("mixed attempted count overflow")?;
        progress_guard.last_operation_sequence = Some(state.next_operation_sequence);
        drop(progress_lock);
        let mut audit = PublicationAudit::default();
        let mut timings = TickTimings::default();
        let outcome = publish_tick(
            &client,
            original,
            &captured,
            &mut audit,
            scheduled,
            &mut timings,
        );
        let audit_detail = audit.detail();
        let timing_detail = timings.detail();
        let mut progress_lock = lock_progress(progress)?;
        let progress_guard = progress_lock.for_tick(tick);
        match outcome {
            Ok(next) => {
                progress_guard.acked = progress_guard
                    .acked
                    .checked_add(1)
                    .context("mixed ACK count overflow")?;
                progress_guard.published = progress_guard
                    .published
                    .checked_add(1)
                    .context("mixed published count overflow")?;
                if let Some(sample) = timings.freshness {
                    progress_guard.freshness_ms.push(sample);
                }
                if let Some(sample) = timings.old_witness {
                    progress_guard.old_witness_ms.push(sample);
                }
                progress_guard.records.push(json!({"operation_sequence":state.next_operation_sequence,"status":"published","audit":audit_detail,"timing":timing_detail}));
                drop(progress_lock);
                let session = open_snapshot(&client, &next).map_err(|error| {
                    record_publisher_failure(progress, "next_snapshot_open", &error);
                    error
                })?;
                snapshot
                    .lock()
                    .map_err(|error| anyhow!("mixed snapshot poisoned: {error}"))?
                    .install(next, session);
                drop(captured);
            }
            Err(error) => {
                if timings.ack.is_some() {
                    progress_guard.acked = progress_guard
                        .acked
                        .checked_add(1)
                        .context("mixed ACK count overflow")?;
                }
                let message = format!("{error:#}");
                progress_guard.last_error = Some(message.clone());
                let failure_error = message.chars().take(512).collect();
                progress_guard.records.push(json!({"operation_sequence":state.next_operation_sequence,"status":"failed","error":message,"audit":audit_detail,"timing":timing_detail}));
                progress_lock.failure_stage = timings.failure_stage;
                progress_lock.failure_error = Some(failure_error);
                drop(progress_lock);
                return Err(error);
            }
        }
    }
    Ok(())
}

fn percentile(values: &[f64], percent: usize) -> Result<f64> {
    ensure!(!values.is_empty(), "mixed percentile has no samples");
    let rank = values
        .len()
        .checked_mul(percent)
        .context("mixed percentile overflow")?
        .div_ceil(100)
        .saturating_sub(1);
    Ok(*values.get(rank).context("mixed percentile rank absent")?)
}

fn percentile_opt(values: &[f64], percent: usize) -> Option<f64> {
    let rank = values
        .len()
        .checked_mul(percent)?
        .div_ceil(100)
        .checked_sub(1)?;
    values.get(rank).copied()
}

fn json_field<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value
        .get(key)
        .with_context(|| format!("mixed result missing {key}"))
}

fn json_u64(value: &Value, key: &str) -> Result<u64> {
    json_field(value, key)?
        .as_u64()
        .with_context(|| format!("mixed result {key} is not unsigned"))
}

fn json_f64(value: &Value, key: &str) -> Result<f64> {
    json_field(value, key)?
        .as_f64()
        .with_context(|| format!("mixed result {key} is not finite numeric"))
}

fn summarize_queries(run: OfferedRun<QueryCompletion>, duration: Duration) -> Result<Value> {
    let mut groups: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
    let mut errors: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut first_error: Option<String> = None;
    let mut max_queue_ms = 0.0_f64;
    for completion in run.completions {
        let name = completion.value.class.name();
        max_queue_ms = max_queue_ms.max(completion.value.queue_ms);
        groups
            .entry(name)
            .or_default()
            .push(completion.value.latency_ms);
        match completion.value.result {
            Ok(()) => {}
            Err(error) => {
                let count = errors.entry(name).or_default();
                *count = count
                    .checked_add(1)
                    .context("mixed query error count overflow")?;
                if first_error.is_none() {
                    first_error = Some(format!("task {} {name}: {error:#}", completion.task.id));
                }
            }
        }
    }
    let mut by_class = serde_json::Map::new();
    for name in ["unique", "broad_top20", "miss"] {
        let samples = groups.entry(name).or_default();
        samples.sort_by(f64::total_cmp);
        let failed = errors.get(name).copied().unwrap_or_default();
        let success = u64::try_from(samples.len())?
            .checked_sub(failed)
            .context("mixed class error count exceeds completions")?;
        let _previous = by_class.insert(
            name.into(),
            json!({
                "success":success,
                "failed":failed,
                "latency_sample_count":samples.len(),
                "p50_ms":if samples.is_empty() { None } else { Some(percentile(samples, 50)?) },
                "p95_ms":if samples.is_empty() { None } else { Some(percentile(samples, 95)?) },
                "p99_ms":if samples.is_empty() { None } else { Some(percentile(samples, 99)?) },
            }),
        );
    }
    Ok(json!({
        "window_seconds": duration.as_secs(),
        "offered":run.offered,
        "dropped_queue_full":run.dropped_queue_full,
        "dropped_scheduler_late":run.dropped_scheduler_late,
        "drain_secs":run.drain_secs,
        "max_queue_ms":max_queue_ms,
        "classes":by_class,
        "first_query_error":first_error,
        "latency_scope":"scheduled arrival to decoded and validated reply, includes failed completions; drops have no latency and are separate failures",
        "qualification":"observation_only",
    }))
}

fn warmup_query_count() -> u64 {
    WARMUP.as_secs() * u64::from(RATE_QPS)
}

fn is_warmup_query(id: u64) -> bool {
    id <= warmup_query_count()
}

fn summarize_query_windows(run: OfferedRun<QueryCompletion>) -> Result<(Value, Value)> {
    let warmup_offered = warmup_query_count();
    let measure_offered = MEASURE.as_secs() * u64::from(RATE_QPS);
    ensure!(
        run.offered == warmup_offered + measure_offered,
        "mixed combined offered count differs"
    );
    ensure!(
        run.dropped_queue_full == 0 && run.dropped_scheduler_late == 0,
        "mixed combined schedule dropped offered queries"
    );
    let origin = run
        .completions
        .iter()
        .find(|completion| completion.task.id == 1)
        .context("mixed first offered query is absent")?
        .task
        .scheduled;
    let mut warmup_drain = WARMUP.as_secs_f64();
    let mut warmup = Vec::new();
    let mut measure = Vec::new();
    for completion in run.completions {
        ensure!(
            (1..=run.offered).contains(&completion.task.id),
            "mixed completion ID outside combined schedule"
        );
        if is_warmup_query(completion.task.id) {
            let offset = completion
                .task
                .scheduled
                .checked_duration_since(origin)
                .context("mixed query scheduled before its origin")?;
            warmup_drain =
                warmup_drain.max(offset.as_secs_f64() + completion.value.latency_ms / 1_000.0);
            warmup.push(completion);
        } else {
            measure.push(completion);
        }
    }
    ensure!(
        warmup.len() == usize::try_from(warmup_offered)?
            && measure.len() == usize::try_from(measure_offered)?,
        "mixed combined schedule has missing completions"
    );
    let warmup = summarize_queries(
        OfferedRun {
            offered: warmup_offered,
            dropped_queue_full: 0,
            dropped_scheduler_late: 0,
            completions: warmup,
            drain_secs: warmup_drain,
        },
        WARMUP,
    )?;
    let measure = summarize_queries(
        OfferedRun {
            offered: measure_offered,
            dropped_queue_full: 0,
            dropped_scheduler_late: 0,
            completions: measure,
            drain_secs: run.drain_secs - WARMUP.as_secs_f64(),
        },
        MEASURE,
    )?;
    Ok((warmup, measure))
}

fn offered_aggregate(run: &OfferedRun<QueryCompletion>) -> Value {
    let first_query_error = run.completions.iter().find_map(|completion| {
        completion.value.result.as_ref().err().map(|error| {
            format!(
                "task {} {}: {error:#}",
                completion.task.id,
                completion.value.class.name()
            )
        })
    });
    json!({
        "offered":run.offered,
        "dropped_queue_full":run.dropped_queue_full,
        "dropped_scheduler_late":run.dropped_scheduler_late,
        "completed":run.completions.len(),
        "drain_secs":run.drain_secs,
        "first_query_error":first_query_error,
    })
}

fn interrupted_offered_detail(progress: &OfferedProgress) -> Value {
    json!({
        "status":"interrupted",
        "planned_arrivals":progress.planned_arrivals,
        "due_arrivals":progress.due_arrivals,
        "enqueued":progress.enqueued,
        "dropped_queue_full":progress.dropped_queue_full,
        "dropped_scheduler_late":progress.dropped_scheduler_late,
        "disconnected":progress.disconnected,
    })
}

fn run_windows(
    socket: &Path,
    seed: u64,
    original: String,
    fixture: Arc<QueryFixture>,
    snapshot: &Arc<Mutex<SnapshotReads>>,
    held: &Arc<Mutex<HeldRead>>,
) -> Result<(MixedStart, Value, Value)> {
    let progress = Arc::new(Mutex::new(PublisherProgress::default()));
    let cancel = Arc::new(AtomicBool::new(false));
    ensure!(
        original.len() <= SOURCE_BOUND,
        "mixed original source exceeds 16 KiB"
    );
    let pub_socket = socket.to_path_buf();
    let pub_snapshot = Arc::clone(snapshot);
    let pub_progress = Arc::clone(&progress);
    let pub_cancel = Arc::clone(&cancel);
    let failure_owner = Arc::new(AtomicU8::new(0));
    let pub_failure = Arc::clone(&failure_owner);
    let (publisher_start, publisher_origin) = mpsc::channel::<Instant>();
    let publisher = thread::Builder::new()
        .name("native-mixed-publisher".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let origin = publisher_origin
                    .recv()
                    .context("mixed publisher start unavailable")?;
                publisher(
                    pub_socket,
                    &original,
                    origin,
                    &pub_snapshot,
                    &pub_progress,
                    &pub_cancel,
                )
            }));
            let result =
                result.unwrap_or_else(|panic| Err(anyhow!("mixed publisher panicked: {panic:?}")));
            if let Err(error) = &result {
                record_publisher_failure(&pub_progress, "publisher_worker", error);
                mark_failure(&pub_failure, FAILURE_PUBLISHER, &pub_cancel);
            }
            result
        })
        .context("spawn mixed publisher")?;
    let mut workers = MixedWorkers {
        publisher: Some(publisher),
        held: None,
        publisher_start: Some(publisher_start),
        held_start: None,
        cancel: Arc::clone(&cancel),
    };
    let held_progress = Arc::new(Mutex::new(HeldWorkerProgress::default()));
    let held_socket = socket.to_path_buf();
    let held_read = Arc::clone(held);
    let held_status = Arc::clone(&held_progress);
    let held_cancel = Arc::clone(&cancel);
    let held_failure = Arc::clone(&failure_owner);
    let (held_start, held_origin) = mpsc::channel::<Instant>();
    let held_worker = thread::Builder::new()
        .name("native-mixed-held-read".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let origin = held_origin.recv().context("mixed held start unavailable")?;
                held_worker(held_socket, origin, &held_read, &held_status, &held_cancel)
            }));
            let result = result
                .unwrap_or_else(|panic| Err(anyhow!("mixed held worker panicked: {panic:?}")));
            if let Err(error) = &result {
                record_held_failure(&held_status, error);
                mark_failure(&held_failure, FAILURE_HELD, &held_cancel);
            }
            result
        })
        .context("spawn mixed held worker")?;
    workers.held = Some(held_worker);
    workers.held_start = Some(held_start);
    let config = OfferedConfig {
        seed,
        arrival_model: ArrivalModel::DeterministicPeriodic,
        rate_qps: RATE_QPS,
        duration: TOTAL,
        workers: WORKERS,
        queue_capacity: QUEUE,
        request_timeout: QUERY_DEADLINE,
    };
    let query_socket = socket.to_path_buf();
    let query_snapshot = Arc::clone(snapshot);
    let query_progress = Arc::new(QueryProgress::default());
    let dispatched_progress = Arc::clone(&query_progress);
    let mut offered_progress = OfferedProgress::default();
    let scheduled = offered_load::run_point_with_start_observed(
        &config,
        &cancel,
        &mut offered_progress,
        |origin| workers.start(origin, held),
        move |task, timeout| {
            let _previous = dispatched_progress
                .dispatched
                .fetch_add(1, Ordering::AcqRel);
            let completion = execute_query(task, timeout, &query_socket, &fixture, &query_snapshot);
            if completion.result.is_err() {
                dispatched_progress.record_failure(task, &completion);
                let _previous = dispatched_progress.failed.fetch_add(1, Ordering::AcqRel);
            }
            let _previous = dispatched_progress.completed.fetch_add(1, Ordering::AcqRel);
            completion
        },
    );
    if scheduled.is_err() {
        mark_failure(&failure_owner, FAILURE_SCHEDULER, &cancel);
    }
    let offered_aggregate = scheduled.as_ref().ok().map(offered_aggregate);
    let scheduler_partial = scheduled
        .as_ref()
        .err()
        .map(|_| interrupted_offered_detail(&offered_progress));
    let (publication, held_lifecycle) = workers.join_all();
    let state = lock_snapshot(snapshot).map(|snapshot| snapshot.state.clone());
    // A publisher panic can poison this diagnostic mutex. Both workers have
    // joined; keep its partial counts for refusal attribution, never GREEN.
    let (progress_guard, publisher_progress_poisoned) = match progress.lock() {
        Ok(guard) => (guard, false),
        Err(poisoned) => (poisoned.into_inner(), true),
    };
    let warmup_counts = validate_publisher_counts(&progress_guard.warmup, WARMUP);
    let measure_counts = validate_publisher_counts(&progress_guard.measure, MEASURE);
    let mut publication_detail = progress_guard.detail();
    if let Some(object) = publication_detail.as_object_mut() {
        let _previous = object.insert(
            "progress_poisoned".into(),
            json!(publisher_progress_poisoned),
        );
    }
    let warmup_publisher = progress_guard.warmup.detail();
    let measure_publisher = progress_guard.measure.detail();
    drop(progress_guard);
    let held_worker_detail = held_progress.lock().map_or_else(
        |error| json!({"progress_error":format!("{error}")}),
        |progress| progress.detail(),
    );
    let (query_values, mut query_failure) = match scheduled.and_then(summarize_query_windows) {
        Ok(values) => (Some(values), None),
        Err(error) => (None, Some(error)),
    };
    let (warmup_query, measure_query) = query_values
        .as_ref()
        .map_or((None, None), |(warmup, measure)| {
            (Some(warmup.clone()), Some(measure.clone()))
        });
    let mut publisher_failure = publication.err().or_else(|| {
        publisher_progress_poisoned.then(|| anyhow!("mixed publisher progress poisoned"))
    });
    let mut held_failure = held_lifecycle.err();
    let warmup_detail =
        json!({"name":"warmup","queries":warmup_query,"publisher":warmup_publisher});
    let measure_detail =
        json!({"name":"measure","queries":measure_query,"publisher":measure_publisher});
    let detail = json!({
        "warmup":warmup_detail,
        "measure":measure_detail,
        "query_progress":query_progress.detail(),
        "scheduler_error":query_failure.as_ref().map(|error| format!("{error:#}")),
        "scheduler_partial":scheduler_partial,
        "offered_aggregate":offered_aggregate,
        "publisher_aggregate":publication_detail,
        "publisher_error":publisher_failure.as_ref().map(|error| format!("{error:#}")),
        "held_worker":held_worker_detail,
        "held_worker_error":held_failure.as_ref().map(|error| format!("{error:#}")),
        "first_failure_owner":match failure_owner.load(Ordering::Acquire) {
            FAILURE_PUBLISHER => Some("publisher"),
            FAILURE_HELD => Some("held_read"),
            FAILURE_SCHEDULER => Some("offered_scheduler"),
            _ => None,
        },
    });
    let primary = match failure_owner.load(Ordering::Acquire) {
        FAILURE_PUBLISHER => publisher_failure
            .take()
            .or_else(|| held_failure.take())
            .or_else(|| query_failure.take()),
        FAILURE_HELD => held_failure
            .take()
            .or_else(|| publisher_failure.take())
            .or_else(|| query_failure.take()),
        FAILURE_SCHEDULER => query_failure
            .take()
            .or_else(|| publisher_failure.take())
            .or_else(|| held_failure.take()),
        _ => publisher_failure
            .take()
            .or_else(|| held_failure.take())
            .or_else(|| query_failure.take()),
    };
    if let Some(error) = primary {
        return Err(error.context(MixedFailurePrefix { detail }));
    }
    let state = state.map_err(|error| {
        error.context(MixedFailurePrefix {
            detail: detail.clone(),
        })
    })?;
    let (warmup_query, measure_query) = query_values.context("mixed query values absent")?;
    (|| -> Result<()> {
        for (name, query) in [("warmup", &warmup_query), ("measure", &measure_query)] {
            let first_error = json_field(query, "first_query_error")?;
            ensure!(
                first_error.is_null(),
                "mixed {name} query failures: {first_error}"
            );
        }
        ensure!(
            json_u64(&measure_query, "offered")? >= 10_000,
            "mixed measure offered fewer than 10,000 queries"
        );
        let classes = json_field(&measure_query, "classes")?;
        for class in ["unique", "broad_top20", "miss"] {
            let row = json_field(classes, class)?;
            ensure!(
                json_u64(row, "success")? >= 1_000,
                "mixed measure class {class} had fewer than 1,000 successes"
            );
            ensure!(
                json_f64(row, "p95_ms")? <= 100.0 && json_f64(row, "p99_ms")? <= 500.0,
                "mixed measure class {class} exceeded warm lexical latency target"
            );
        }
        ensure!(
            json_f64(&measure_publisher, "current_gold_from_scheduled_p95_ms")? <= 1_000.0
                && json_f64(&measure_publisher, "current_gold_from_scheduled_p99_ms")? <= 5_000.0,
            "mixed one-file freshness exceeded target"
        );
        warmup_counts?;
        measure_counts?;
        Ok(())
    })()
    .map_err(|error| {
        error.context(MixedFailurePrefix {
            detail: detail.clone(),
        })
    })?;
    Ok((state, warmup_detail, measure_detail))
}

pub(super) fn run(
    socket: &Path,
    tier: ScaleTier,
    seed: u64,
    corpus: &[ScopedFile],
    refs: &[ObjectRef],
    start: MixedStart,
) -> Result<MixedOutcome> {
    ensure!(
        corpus.len() == refs.len() && corpus.len() >= 2,
        "mixed corpus/source refs mismatch"
    );
    let repo_count = params_for(tier).repo_count;
    ensure!(repo_count > 0, "mixed tier has no repositories");
    let fixture = Arc::new(QueryFixture::from_source(corpus, refs, repo_count, seed)?);
    let original = corpus
        .first()
        .context("mixed first source absent")?
        .content
        .clone();
    let client = NativeUdsClient::new(socket.to_path_buf());
    let held = Arc::new(Mutex::new(HeldRead::open(
        &client,
        start.view.identity,
        start.first_token.as_deref(),
        start.first_ref,
    )?));
    let snapshot = Arc::new(Mutex::new(SnapshotReads::default()));
    let result = (|| -> Result<MixedOutcome> {
        let session = open_snapshot(&client, &start)?;
        snapshot
            .lock()
            .map_err(|error| anyhow!("mixed snapshot poisoned: {error}"))?
            .install(start, session);
        let (measured, warm_detail, measure_detail) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_windows(socket, seed, original, fixture, &snapshot, &held)
            }))
            .unwrap_or_else(|panic| Err(anyhow!("mixed windows panicked: {panic:?}")))?;
        let retained_detail = {
            let retained = held
                .lock()
                .map_err(|error| anyhow!("mixed held read poisoned: {error}"))?;
            retained.complete()?;
            retained.detail()
        };
        Ok(MixedOutcome {
            final_view: measured.view.identity,
            first_ref: measured.first_ref,
            first_token: measured.first_token,
            detail: json!({"warmup":warm_detail,"measure":measure_detail,
                "retained_read":retained_detail,
                "scope":"Native UDS mixed offered load on one continuous 960-second epoch with product retained-read witness; sampled resource observations are not continuous peaks"}),
        })
    })();
    // The combined run joined its publisher and workers before reaching this point.
    let cleanup = Arc::try_unwrap(snapshot)
        .map_err(|remaining| {
            anyhow!(
                "mixed snapshot owner remains after join: {} owners",
                Arc::strong_count(&remaining)
            )
        })
        .and_then(|snapshots| {
            SnapshotReads::drain_owned(snapshots, |session| release_snapshot(&client, session))
        });
    let snapshot_cleanup_error = cleanup.as_ref().err().map(|error| format!("{error:#}"));
    let result = match (result, cleanup) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.context("mixed snapshot cleanup")),
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("mixed snapshot cleanup also failed: {cleanup:#}")))
        }
    };
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let mut detail = error
                .downcast_ref::<MixedFailurePrefix>()
                .map_or_else(|| json!({}), |prefix| prefix.detail.clone());
            // The held worker has joined. A panic cannot invalidate the
            // immutable session identity used by the idempotent Release.
            let (mut retained, held_lock_poisoned) = match held.lock() {
                Ok(retained) => (retained, false),
                Err(error) => (error.into_inner(), true),
            };
            let cleanup = if retained.is_active() {
                retained.release(&client, &RequestBudgetV1::for_duration(QUERY_DEADLINE))
            } else {
                Ok(())
            };
            let held_detail = retained.detail();
            let cleanup_error = cleanup.err().map(|failure| format!("{failure:#}"));
            if let Some(object) = detail.as_object_mut() {
                let _previous = object.insert("retained_read".into(), held_detail);
                let _previous =
                    object.insert("held_lock_poisoned".into(), json!(held_lock_poisoned));
                let _previous = object.insert("cleanup_error".into(), json!(cleanup_error));
                let _previous = object.insert(
                    "snapshot_cleanup_error".into(),
                    json!(snapshot_cleanup_error),
                );
            }
            Err(error.context(MixedFailurePrefix { detail }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_clock_and_worker_start_do_not_follow_publisher_backlog() -> Result<()> {
        const HELD_TEST_TICK: u64 = 600;
        let origin = Instant::now();
        let (publisher_start, publisher_origin) = mpsc::channel();
        let (held_start, held_origin) = mpsc::channel();
        let (publisher_entered, entered) = mpsc::channel();
        let (release_publisher, publisher_release) = mpsc::channel();
        let (held_observed, observed) = mpsc::channel();
        let publisher = thread::spawn(move || -> Result<()> {
            let started = publisher_origin.recv()?;
            publisher_entered.send(started)?;
            publisher_release.recv_timeout(Duration::from_secs(3))?;
            Ok(())
        });
        let held = thread::spawn(move || -> Result<()> {
            let started = held_origin.recv()?;
            held_observed.send(tick_bounds(started, HELD_TEST_TICK)?)?;
            Ok(())
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let mut workers = MixedWorkers {
            publisher: Some(publisher),
            held: Some(held),
            publisher_start: Some(publisher_start),
            held_start: Some(held_start),
            cancel: Arc::clone(&cancel),
        };
        workers.signal_start(origin)?;
        let publisher_started = entered.recv_timeout(Duration::from_secs(1));
        let held_bounds = observed.recv_timeout(Duration::from_secs(1));
        release_publisher.send(())?;
        let (publisher_result, held_result) = workers.join_all();
        publisher_result?;
        held_result?;
        ensure!(publisher_started? == origin, "publisher origin differs");
        let (scheduled, deadline) = held_bounds?;
        ensure!(
            scheduled == origin + Duration::from_secs(HELD_TEST_TICK)
                && deadline == scheduled + PUBLISH_PERIOD,
            "held deadline followed publisher backlog"
        );
        let owner = AtomicU8::new(0);
        mark_failure(&owner, FAILURE_HELD, &cancel);
        mark_failure(&owner, FAILURE_SCHEDULER, &cancel);
        ensure!(
            owner.load(Ordering::Acquire) == FAILURE_HELD && cancel.load(Ordering::Acquire),
            "secondary cancellation replaced the first held failure"
        );
        let external_cancel = AtomicBool::new(true);
        let external_owner = AtomicU8::new(0);
        mark_failure(&external_owner, FAILURE_PUBLISHER, &external_cancel);
        ensure!(
            external_owner.load(Ordering::Acquire) == FAILURE_SCHEDULER,
            "offered worker cancellation was attributed to a sibling"
        );
        Ok(())
    }

    #[test]
    fn interrupted_scheduler_detail_separates_queue_outcomes() {
        let detail = interrupted_offered_detail(&OfferedProgress {
            planned_arrivals: 48_000,
            due_arrivals: 7,
            enqueued: 2,
            dropped_queue_full: 3,
            dropped_scheduler_late: 1,
            disconnected: 1,
        });
        assert_eq!(
            detail,
            json!({
                "status":"interrupted",
                "planned_arrivals":48_000,
                "due_arrivals":7,
                "enqueued":2,
                "dropped_queue_full":3,
                "dropped_scheduler_late":1,
                "disconnected":1,
            })
        );
    }

    #[test]
    fn partial_failure_keeps_first_query_stage_and_bounded_error() -> Result<()> {
        let progress = QueryProgress::default();
        let scheduled = Instant::now();
        progress.record_failure(
            OfferedTask { id: 7, scheduled },
            &QueryCompletion {
                class: QueryClass::Broad,
                latency_ms: 200.0,
                queue_ms: 10.0,
                failure_stage: Some("broad_read_session_search"),
                result: Err(anyhow!("{}", "x".repeat(1_024))),
            },
        );
        progress.record_failure(
            OfferedTask { id: 8, scheduled },
            &QueryCompletion {
                class: QueryClass::Miss,
                latency_ms: 210.0,
                queue_ms: 20.0,
                failure_stage: Some("miss_verify"),
                result: Err(anyhow!("second failure")),
            },
        );
        let first = progress.detail();
        assert_eq!(first.pointer("/first_failure/task_id"), Some(&json!(7)));
        assert_eq!(
            first.pointer("/first_failure/stage"),
            Some(&json!("broad_read_session_search"))
        );
        assert_eq!(
            first
                .pointer("/first_failure/error")
                .and_then(Value::as_str)
                .context("first query error absent")?
                .chars()
                .count(),
            512
        );
        Ok(())
    }
    use quanta_index_contract::{
        FiniteScore,
        index::Digest,
        native_protocol::{NativeTextCoverage, NativeTextHit},
    };

    fn fixed_fixture() -> Result<(Vec<ScopedFile>, Vec<ObjectRef>, QueryFixture, MixedStart)> {
        let mut corpus = Vec::new();
        let mut refs = Vec::new();
        for index in 0..22_usize {
            let file_index = u32::try_from(index)?;
            let filler = "x ".repeat(index.div_euclid(2));
            corpus.push(ScopedFile {
                source_repo_id: "repo0".into(),
                repo_relative_path: format!("src/file_{index}.rs"),
                content: format!(
                    "{filler}// {} anchor\n// {} anchor\n",
                    repo_query_token(0),
                    file_query_token(0, file_index)
                ),
            });
            refs.push(ObjectRef(Digest([u8::try_from(index)?; 32])));
        }
        let fixture = QueryFixture::from_source(&corpus, &refs, 1, 42)?;
        let first_ref = refs.first().copied().context("fixed first ref absent")?;
        let state = MixedStart {
            view: fixture_catalog_view(ViewRef {
                workspace: operation(1).workspace,
                epoch: 1,
                sequence: 5,
                digest: Digest([7; 32]),
            }),
            first_ref: Some(first_ref),
            first_token: Some(file_query_token(0, 0)),
            next_operation_sequence: 6,
            next_event_sequence: 5,
            transition_step: 0,
        };
        Ok((corpus, refs, fixture, state))
    }

    #[test]
    fn mixed_product_config_selects_bounded_retention_and_session_custody_only() -> Result<()> {
        let workload = super::super::workload::selected_workload_from_json(ScaleTier::Small, None)?;
        let root = Path::new("/fixture-state");
        let socket = Path::new("/fixture.sock");
        let ordinary = super::super::native_config(root, socket, workload);
        let mixed = super::super::mixed_native_config(root, socket, workload)?;
        ensure!(ordinary.get("retention").is_none() && ordinary.get("read_sessions").is_none());
        ensure!(mixed.pointer("/retention/sequence_window") == Some(&json!(1)));
        ensure!(mixed.pointer("/read_sessions/global_limit") == Some(&json!(128)));
        ensure!(mixed.pointer("/read_sessions/per_uid_limit") == Some(&json!(128)));
        ensure!(mixed.pointer("/read_sessions/terminal_ttl_millis") == Some(&json!(60_000)));
        ensure!(mixed.pointer("/resources/read_ticket_lease_ms") == Some(&json!(300_000)));
        ensure!(ordinary.pointer("/resources/read_ticket_lease_ms") == Some(&json!(30_000)));
        ensure!(mixed.pointer("/resources/execution") == ordinary.pointer("/resources/execution"));
        ensure!(mixed.pointer("/resources/physical") == ordinary.pointer("/resources/physical"));
        ensure!(mixed.pointer("/work_limits") == ordinary.pointer("/work_limits"));
        Ok(())
    }

    fn fixed_page(
        corpus: &[ScopedFile],
        refs: &[ObjectRef],
        view: ViewRef,
    ) -> Result<NativeTextResult> {
        let mut hits = Vec::new();
        for index in 0..20_usize {
            let file = corpus.get(index).context("fixed file absent")?;
            let indexed = source_token_stats(
                &file.content,
                &repo_query_token(0),
                &file_query_token(0, u32::try_from(index)?),
            )?
            .0;
            let score = FiniteScore::new(1.0 / (1.0 + f64::from(u32::try_from(indexed)?)))?;
            hits.push(NativeTextHit {
                owner: owner(index)?,
                source: *refs.get(index).context("fixed source absent")?,
                path: file_identity(file),
                score,
            });
        }
        Ok(NativeTextResult {
            view,
            instance: text_spec().instance,
            coverage: NativeTextCoverage::CoreRequired,
            artifact: ObjectRef(Digest([9; 32])),
            matched: 22,
            exhaustive: true,
            hits,
        })
    }

    #[test]
    fn broad_top20_gold_rejects_wrong_binding_rank_count_and_partial() -> Result<()> {
        let (corpus, refs, fixture, state) = fixed_fixture()?;
        let view = state.view.identity;
        let page = fixed_page(&corpus, &refs, view)?;
        verify_broad(&page, view, 0, &state, &fixture)?;
        let mut changed = page.clone();
        changed.hits.get_mut(0).context("fixed hit absent")?.source = ObjectRef(Digest([90; 32]));
        ensure!(
            verify_broad(&changed, view, 0, &state, &fixture).is_err(),
            "wrong source passed broad gold"
        );
        let mut changed = page.clone();
        changed.hits.get_mut(0).context("fixed hit absent")?.path = "repo0/src/wrong.rs".into();
        ensure!(
            verify_broad(&changed, view, 0, &state, &fixture).is_err(),
            "wrong path passed broad gold"
        );
        let mut changed = page.clone();
        changed.hits.get_mut(0).context("fixed hit absent")?.owner = owner(21)?;
        ensure!(
            verify_broad(&changed, view, 0, &state, &fixture).is_err(),
            "wrong owner passed broad gold"
        );
        let mut changed = page.clone();
        changed.matched = 21;
        ensure!(
            verify_broad(&changed, view, 0, &state, &fixture).is_err(),
            "wrong matched count passed broad gold"
        );
        let mut changed = page.clone();
        changed.exhaustive = false;
        ensure!(
            verify_broad(&changed, view, 0, &state, &fixture).is_err(),
            "partial page passed broad gold"
        );
        let mut changed = page.clone();
        let lower = corpus.get(20).context("fixed lower-ranked source absent")?;
        let lower_tokens = source_token_stats(
            &lower.content,
            &repo_query_token(0),
            &file_query_token(0, 20),
        )?
        .0;
        *changed
            .hits
            .get_mut(19)
            .context("fixed twentieth hit absent")? = NativeTextHit {
            owner: owner(20)?,
            source: *refs.get(20).context("fixed lower-ranked ref absent")?,
            path: file_identity(lower),
            score: FiniteScore::new(1.0 / (1.0 + f64::from(u32::try_from(lower_tokens)?)))?,
        };
        ensure!(
            verify_broad(&changed, view, 0, &state, &fixture).is_err(),
            "lower-ranked replacement passed broad top20 gold"
        );
        let mut changed = page;
        changed.hits.swap(0, 1);
        ensure!(
            verify_broad(&changed, view, 0, &state, &fixture).is_err(),
            "tie-order reversal passed broad gold"
        );
        Ok(())
    }

    #[test]
    fn query_deadline_uses_original_offered_instant() -> Result<()> {
        let scheduled = Instant::now();
        let task = OfferedTask { id: 1, scheduled };
        let deadline = scheduled
            .checked_add(QUERY_DEADLINE)
            .context("test deadline overflow")?;
        ensure!(
            query_budget(task, QUERY_DEADLINE)?.deadline() == deadline,
            "query budget extended scheduled deadline"
        );
        let expired = scheduled
            .checked_sub(Duration::from_secs(3))
            .context("test prior instant absent")?;
        ensure!(
            query_budget(
                OfferedTask {
                    id: 2,
                    scheduled: expired
                },
                QUERY_DEADLINE
            )
            .is_err(),
            "expired scheduled query received a new budget"
        );
        Ok(())
    }

    #[test]
    fn query_snapshot_wait_preserves_original_cancellation_and_deadline() -> Result<()> {
        let snapshot = Mutex::new(SnapshotReads::default());
        let _held = snapshot
            .lock()
            .map_err(|error| anyhow!("test snapshot lock: {error}"))?;
        let cancelled = RequestBudgetV1::for_duration(QUERY_DEADLINE);
        cancelled.cancel_handle().cancel();
        ensure!(capture_query_snapshot(&snapshot, &cancelled).is_err());
        let past = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .context("test prior deadline absent")?;
        let expired = RequestBudgetV1::until(past);
        ensure!(capture_query_snapshot(&snapshot, &expired).is_err());
        Ok(())
    }

    #[test]
    fn replace_delete_restore_keep_one_key_and_event_step_per_publication() -> Result<()> {
        let (_corpus, refs, _fixture, mut state) = fixed_fixture()?;
        let original = "// scalereponeedle000 scalefileneedle000file00000\n";
        for step in 0..3_u64 {
            let plan = planned_source(
                state.transition_step,
                state.next_operation_sequence,
                original,
            )?;
            match step {
                0 => ensure!(
                    plan.content
                        .as_deref()
                        .is_some_and(|text| text.contains("qidxmixedneedle6")),
                    "replace source missing token"
                ),
                1 => ensure!(
                    plan.content.is_none() && plan.token.is_none(),
                    "delete staged source"
                ),
                _ => ensure!(
                    plan.content.as_deref() == Some(original)
                        && plan.token.as_deref() == Some(file_query_token(0, 0).as_str()),
                    "restore differs from original"
                ),
            }
            let mut next_view = state.view.clone();
            next_view.identity.sequence = next_view
                .identity
                .sequence
                .checked_add(1)
                .context("test view overflow")?;
            next_view.source.owners.0 = Some(ObjectRef(Digest([u8::try_from(step)?; 32])));
            let expected_source = next_view.source;
            let reference = if step == 1 {
                None
            } else {
                Some(*refs.first().context("test ref absent")?)
            };
            state = state.advanced(next_view, reference, plan.token)?;
            ensure!(
                state.view.source == expected_source,
                "mixed source cut drifted"
            );
            ensure!(
                state.next_operation_sequence
                    == 7_u64.checked_add(step).context("test operation overflow")?
                    && state.next_event_sequence
                        == 6_u64.checked_add(step).context("test event overflow")?
                    && state.transition_step
                        == 1_u64.checked_add(step).context("test step overflow")?,
                "mixed key/event/transition sequence drifted"
            );
        }
        Ok(())
    }

    #[test]
    fn publisher_failure_and_late_tick_stay_in_denominator() -> Result<()> {
        let mut progress = PublisherWindowProgress {
            scheduled: 1,
            attempted: 1,
            ..PublisherWindowProgress::default()
        };
        ensure!(
            validate_publisher_counts(&progress, Duration::from_secs(1)).is_err(),
            "unacked attempt passed"
        );
        progress.acked = 1;
        ensure!(
            validate_publisher_counts(&progress, Duration::from_secs(1)).is_err(),
            "ACK without gold passed"
        );
        progress.published = 1;
        validate_publisher_counts(&progress, Duration::from_secs(1))?;
        progress.attempted = 0;
        progress.acked = 0;
        progress.published = 0;
        progress.missed_late = 1;
        ensure!(
            validate_publisher_counts(&progress, Duration::from_secs(1)).is_err(),
            "late dropped publication passed"
        );
        Ok(())
    }

    #[test]
    fn reached_tick_prepublication_failure_keeps_denominator_and_stage() -> Result<()> {
        let progress = Mutex::new(PublisherProgress::default());
        lock_progress(&progress)?.record_reached_tick(WARMUP.as_secs())?;
        record_publisher_failure(&progress, "snapshot_capture", &anyhow!("snapshot failed"));
        let recorded = lock_progress(&progress)?;
        assert_eq!(recorded.warmup.scheduled, 0);
        assert_eq!(recorded.measure.scheduled, 1);
        assert_eq!(recorded.measure.attempted, 0);
        assert_eq!(recorded.failure_stage, Some("snapshot_capture"));
        assert!(validate_publisher_counts(&recorded.measure, Duration::from_secs(1)).is_err());
        Ok(())
    }

    #[test]
    fn expired_tick_is_counted_before_tick_budget_can_be_used() -> Result<()> {
        let mut progress = PublisherProgress::default();
        let tick = WARMUP.as_secs();
        let scheduled = Instant::now();
        let deadline = scheduled
            .checked_add(PUBLISH_PERIOD)
            .context("mixed test deadline overflow")?;
        progress.record_reached_tick(tick)?;
        let window = progress.for_tick(tick);
        assert!(!window.skip_expired_tick(scheduled, deadline)?);
        assert_eq!(window.missed_late, 0);
        assert!(window.skip_expired_tick(deadline, deadline)?);
        assert_eq!(window.scheduled, 1);
        assert_eq!(window.missed_late, 1);
        assert_eq!(window.attempted, 0);
        assert!(validate_publisher_counts(window, PUBLISH_PERIOD).is_err());
        Ok(())
    }

    #[test]
    fn combined_schedule_split_has_exact_warmup_boundary() -> Result<()> {
        let mut progress = PublisherProgress::default();
        progress.for_tick(WARMUP.as_secs() - 1).scheduled += 1;
        progress.for_tick(WARMUP.as_secs()).scheduled += 1;
        assert_eq!(progress.warmup.scheduled, 1);
        assert_eq!(progress.measure.scheduled, 1);
        assert_eq!(warmup_query_count(), 3_000);
        assert_eq!(TOTAL.as_secs(), 960);
        assert!(is_warmup_query(3_000));
        assert!(!is_warmup_query(3_001));
        let origin = Instant::now();
        let completions = (1_u64..=48_000)
            .map(|id| offered_load::OfferedCompletion {
                task: OfferedTask {
                    id,
                    scheduled: origin + Duration::from_millis((id - 1) * 20),
                },
                value: QueryCompletion {
                    class: QueryClass::for_id(id),
                    latency_ms: if id == 3_000 { 270.0 } else { 5.0 },
                    queue_ms: 0.0,
                    failure_stage: None,
                    result: Ok(()),
                },
            })
            .collect();
        let (warmup, measure) = summarize_query_windows(OfferedRun {
            offered: 48_000,
            dropped_queue_full: 0,
            dropped_scheduler_late: 0,
            completions,
            drain_secs: 960.5,
        })?;
        assert_eq!(json_u64(&warmup, "offered")?, 3_000);
        assert_eq!(json_u64(&measure, "offered")?, 45_000);
        assert!((json_f64(&warmup, "drain_secs")? - 60.25).abs() < 1e-9);
        assert!((json_f64(&measure, "drain_secs")? - 900.5).abs() < 1e-9);
        Ok(())
    }
}
