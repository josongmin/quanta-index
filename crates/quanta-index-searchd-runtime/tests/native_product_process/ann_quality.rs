//! Fixed 4,096 x 64 Native ANN quality gate through the product UDS and SDK.

use super::*;
use std::{cell::Cell, collections::BTreeSet, fmt::Write as _, io::Read as _, path::PathBuf};

use anyhow::Context as _;
use std::io::Write as _;

use quanta_index_contract::{
    index::ViewRef,
    native_protocol::{
        NativeOverlayDocument, NativeOverlayReply, NativeOverlayRequest, NativeOverlaySession,
        NativeSemanticResult, NativeSemanticTraversal,
    },
};
use quanta_index_sdk::native_uds::MAX_NATIVE_STAGE_SOURCES_INPUTS;
use quanta_index_searchd_harness::ann::{
    AnnRailConfig, exact_top_k, query_direction, unit_direction,
};

#[path = "ann_quality/filtered_quality.rs"]
pub(crate) mod filtered_quality;
#[path = "ann_quality/materialization_diagnostic.rs"]
mod materialization_diagnostic;
#[path = "ann_quality/selected_changes.rs"]
mod selected_changes;
#[path = "ann_quality/work_status.rs"]
pub(super) mod work_status;

use selected_changes::{
    check_delta_provider_inputs, check_exact_path_query, overlay_quality_sweep, stage_quality_delta,
};

const QUALITY_WATCHDOG: Duration = Duration::from_secs(1_200);
const EXACT_SCORE_TOLERANCE: f64 = 1.0e-4;
const QUALITY_MAX_OWNERS: u64 = 1_024;
// Three 64-owner seals carry 64+64 to 128, then retain the final 64 as a
// second region: the third carry cannot pull a larger prior run into 64 rows.
pub(super) const FILTERED_CONFIG: AnnRailConfig = AnnRailConfig {
    rows: 192,
    width: 64,
    queries: 4,
    k: 10,
    recall_floor: 0.95,
};

// Keep the real selected-change phase behind one testable deadline handoff.
fn with_original_quality_deadline<T>(
    original_deadline: Instant,
    run: impl FnOnce(Instant) -> TestResult<T>,
) -> TestResult<T> {
    require(
        Instant::now() < original_deadline,
        "ANN selected-change watchdog expired before phase",
    )?;
    run(original_deadline)
}

#[test]
fn selected_change_phase_refuses_expired_original_deadline_before_work() -> TestResult {
    let original = Instant::now()
        .checked_sub(Duration::from_secs(1))
        .ok_or("expired quality fixture deadline overflow")?;
    let entered = Cell::new(false);
    let result = with_original_quality_deadline(original, |_deadline| {
        entered.set(true);
        Ok(())
    });
    require(
        result.is_err(),
        "expired original quality deadline was renewed",
    )?;
    require(
        !entered.get(),
        "expired quality phase reached selected work",
    )
}

#[test]
fn selected_change_phase_preserves_live_original_deadline_and_result() -> TestResult {
    let original = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or("live quality fixture deadline overflow")?;
    let result = with_original_quality_deadline(original, |received| {
        require_equal(
            &received,
            &original,
            "selected phase extended the original watchdog",
        )?;
        Ok(42_u32)
    })?;
    require_equal(&result, &42, "selected phase changed the original result")
}

fn quality_owner(seed: u64, config: &AnnRailConfig) -> TestResult<OwnerKey> {
    require(
        seed < config.rows,
        "ANN owner seed exceeds the fixed corpus",
    )?;
    let id = format!("row-{seed}");
    let mut stable = [0_u8; 16];
    stable
        .get_mut(..id.len())
        .ok_or("ANN row identity exceeds StableOwnerId")?
        .copy_from_slice(id.as_bytes());
    Ok(OwnerKey {
        producer: operation(2).producer,
        stable_id: StableOwnerId(stable),
    })
}

fn quality_row_id(owner: OwnerKey, config: &AnnRailConfig) -> TestResult<String> {
    require_equal(&owner.producer, &operation(2).producer, "ANN hit producer")?;
    let bytes = &owner.stable_id.0;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    require(
        bytes
            .get(end..)
            .ok_or("ANN owner suffix missing")?
            .iter()
            .all(|byte| *byte == 0),
        "ANN hit owner has noncanonical padding",
    )?;
    let id = std::str::from_utf8(bytes.get(..end).ok_or("ANN owner prefix missing")?)?;
    let seed = id
        .strip_prefix("row-")
        .ok_or("ANN hit owner has no row prefix")?
        .parse::<u64>()?;
    require_equal(
        &quality_owner(seed, config)?,
        &owner,
        "ANN hit canonical owner identity",
    )?;
    Ok(id.to_owned())
}

fn quality_query(seed: u64, config: &AnnRailConfig) -> TestResult<SemanticQueryRequest> {
    require(
        seed < config.queries,
        "ANN query seed exceeds the fixed workload",
    )?;
    Ok(SemanticQueryRequest {
        query_text: format!("query-{seed}"),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: u32::try_from(config.k)?,
    })
}

fn corpus(config: &AnnRailConfig) -> Vec<(String, Vec<f32>)> {
    (0..config.rows)
        .map(|seed| (format!("row-{seed}"), unit_direction(seed, config.width)))
        .collect()
}

fn selected_vector<'a>(rows: &'a [(String, Vec<f32>)], id: &str) -> TestResult<&'a [f32]> {
    rows.iter()
        .find(|(row_id, _vector)| row_id == id)
        .map(|(_row_id, vector)| vector.as_slice())
        .ok_or_else(|| format!("ANN selected oracle row {id} absent").into())
}

struct StagedQuality {
    view: ViewRef,
    sources: Vec<(OwnerKey, ObjectRef)>,
}

fn stage_quality_sources(
    client: &NativeUdsClient,
    configured: ViewRef,
    config: &AnnRailConfig,
    deadline: Instant,
    source_text: impl Fn(u64) -> String,
) -> TestResult<StagedQuality> {
    let key = operation(2);
    let ingress = client
        .begin_input(key, &budget().capped_at(deadline))
        .context("ANN quality begin source ingress")?;
    let mut sources = (0..config.rows).map(|seed| SourceInput {
        path: format!("src/{seed:04}.rs"),
        bytes: source_text(seed).into_bytes(),
        text: SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
    });
    let mut records = Vec::new();
    loop {
        require(
            Instant::now() < deadline,
            "ANN source staging watchdog expired",
        )?;
        let batch: Vec<_> = sources
            .by_ref()
            .take(MAX_NATIVE_STAGE_SOURCES_INPUTS)
            .collect();
        if batch.is_empty() {
            break;
        }
        let offset = records.len();
        let count = batch.len();
        records.extend(
            client
                .stage_sources(ingress, batch, &budget().capped_at(deadline))
                .with_context(|| format!("ANN quality stage_sources at {offset}, count {count}"))?,
        );
    }
    require_equal(
        &records.len(),
        &usize::try_from(config.rows)?,
        "ANN staged source count",
    )?;
    let prior = client
        .read_view(
            ViewSelection::Exact(configured),
            &budget().capped_at(deadline),
        )
        .context("ANN quality read configured source cut")?;
    let owner_keys = (0..config.rows)
        .map(|seed| quality_owner(seed, config))
        .collect::<TestResult<Vec<_>>>()?;
    let mut expected: Vec<_> = owner_keys
        .iter()
        .copied()
        .zip(records.iter().copied())
        .collect();
    expected.sort_unstable_by_key(|(owner, _source)| *owner);
    require(
        expected
            .windows(2)
            .all(|pair| matches!(pair, [left, right] if left.0 < right.0)),
        "ANN staged owner identities are not unique",
    )?;
    let ordinal = Cell::new(None);
    let mutation_budget = budget().capped_at(deadline);
    let mutation_started = Instant::now();
    let owners = client
        .stage_mutations(
            ingress,
            // Reuse the independently checked owner/source inventory order.
            // Numeric row order revisits lexical COW subtrees across RPC pages.
            expected
                .iter()
                .copied()
                .enumerate()
                .map(|(index, (owner, record))| {
                    ordinal.set(Some(index));
                    OwnerMutation {
                        owner,
                        expected: None,
                        next: Some(record),
                    }
                }),
            &mutation_budget,
        )
        .with_context(|| {
            format!(
                "ANN quality stage_mutations through owner ordinal {:?} of {}, elapsed {:?}, remaining {:?}",
                ordinal.get(),
                config.rows,
                mutation_started.elapsed(),
                mutation_budget.remaining(),
            )
        })?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
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
    let submit_budget = budget().capped_at(deadline);
    let submit_started = Instant::now();
    let submitted = client.submit(
        UpdateRequest {
            operation: key,
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &submit_budget,
    );
    let submit_elapsed = submit_started.elapsed();
    let submitted = submitted.with_context(|| {
        format!(
            "ANN quality submit SourceAdvance: elapsed {:?}, remaining {:?}, interruption {:?}",
            submit_started.elapsed(),
            submit_budget.remaining(),
            submit_budget.interruption(),
        )
    })?;
    let view = published(&submitted)
        .map_err(|error| format!("ANN quality SourceAdvance terminal status: {error}"))?;
    writeln!(
        std::io::stderr().lock(),
        "native-ann-source-submit rows={} elapsed_ns={} published=true",
        config.rows,
        submit_elapsed.as_nanos(),
    )?;
    require(
        Instant::now() < deadline,
        "ANN source staging watchdog expired after SourceAdvance",
    )?;
    Ok(StagedQuality {
        view,
        sources: expected,
    })
}

fn verify_selected_sources(
    client: &NativeUdsClient,
    selected: ViewRef,
    expected: &[(OwnerKey, ObjectRef)],
    deadline: Instant,
) -> TestResult {
    let view = client.read_view(
        ViewSelection::Exact(selected),
        &budget().capped_at(deadline),
    )?;
    require_equal(
        &view.identity,
        &selected,
        "ANN source inventory selected view",
    )?;
    let mut after = None;
    let mut observed = 0_usize;
    let mut pages = 0_usize;
    loop {
        require(
            Instant::now() < deadline,
            "ANN source inventory watchdog expired",
        )?;
        pages = pages
            .checked_add(1)
            .ok_or("ANN source page count overflow")?;
        require(pages <= 33, "ANN source inventory exceeded 128-row pages")?;
        let page = client.list_producer_source_owners(
            ViewSelection::Exact(selected),
            operation(2).producer,
            after,
            128,
            &budget().capped_at(deadline),
        )?;
        require_equal(&page.view, &selected, "ANN source page view")?;
        require_equal(&page.source, &view.source, "ANN source page SourceCut")?;
        require_equal(&page.source_epoch, &Some(1), "ANN source page epoch")?;
        for row in page.rows.as_slice() {
            let expected_row = expected
                .get(observed)
                .ok_or("ANN selected source contains an extra owner")?;
            require_equal(
                &(row.owner, row.source),
                expected_row,
                "ANN selected owner and source reference",
            )?;
            observed = observed
                .checked_add(1)
                .ok_or("ANN selected source count overflow")?;
        }
        if !page.has_more {
            break;
        }
        after = Some(
            page.next_after()
                .ok_or("ANN source continuation lost cursor")?,
        );
    }
    require_equal(&observed, &expected.len(), "ANN selected source count")?;
    require(
        Instant::now() < deadline,
        "ANN source inventory watchdog expired after final page",
    )
}

#[derive(Debug)]
struct WorkStatusObservationError {
    context: String,
    cause: Box<dyn std::error::Error>,
}

impl std::fmt::Display for WorkStatusObservationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.context)
    }
}

impl std::error::Error for WorkStatusObservationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

fn wait_for_complete(
    client: &NativeUdsClient,
    daemon: &mut Daemon,
    source_view: ViewRef,
    mut completion: work_status::CompletionProgress<'_>,
    config: &AnnRailConfig,
    expected_completion_count: Option<u64>,
    deadline: Instant,
) -> TestResult<ViewRef> {
    let completed_sequence = expected_completion_count
        .map(|count| {
            source_view
                .sequence
                .checked_add(count)
                .ok_or("ANN completion sequence overflow")
        })
        .transpose()?;
    let mut phase = "await exact owner work";
    let mut last_sequence = None;
    let mut last_coverage = None;
    // Readiness uses exact public work receipts, not repeated pin writes.
    // The final view/sequence/coverage and quality oracles remain below.
    // Keep only eight minute samples of independently observed completions.
    let started = Instant::now();
    let mut last_progress = started;
    let mut next_sample = Duration::ZERO;
    let mut progress = std::collections::VecDeque::with_capacity(8);
    let mut observed_completions = 0;
    let mut readiness_pace = work_status::ReadinessPace::new();
    let mut readiness_cost = work_status::ReadinessCost::new(if completion.total() == 1 {
        "incremental"
    } else {
        "initial"
    });
    loop {
        if Instant::now() >= deadline {
            return Err(format!(
                "ANN materialization watchdog expired in {phase}: target sequence {completed_sequence:?}, last sequence {last_sequence:?}, last coverage {last_coverage:?}; bounded progress samples (elapsed seconds, observed completed inputs)={progress:?}, observed completed inputs={observed_completions}/{}, seconds since last observed completed input={}",
                completion.total(),
                last_progress.elapsed().as_secs(),
            )
            .into());
        }
        if let Some(status) = daemon.0.try_wait()? {
            return Err(format!(
                "ANN daemon exited during materialization in {phase}: {status}; target sequence {completed_sequence:?}, last sequence {last_sequence:?}, last coverage {last_coverage:?}; bounded progress samples (elapsed seconds, observed completed inputs)={progress:?}, observed completed inputs={observed_completions}/{}",
                completion.total(),
            )
            .into());
        }
        let polled = completion.poll_next(|intent| {
            require(
                Instant::now() < deadline,
                "ANN materialization watchdog expired before work-status read",
            )?;
            let rpc_started = Instant::now();
            let reply = client.request(
                NativeIndexRequest::WorkStatus(intent),
                &budget().capped_at(deadline),
            );
            readiness_cost.record_rpc(rpc_started.elapsed());
            let reply = reply?;
            let NativeIndexResponse::WorkStatus(status) = reply else {
                return Err(format!("ANN work-status reply differs: {reply:?}").into());
            };
            Ok(status)
        });
        let observed_at = Instant::now();
        readiness_cost.observe_completed(completion.completed());
        let advanced = completion.completed() > observed_completions;
        if advanced {
            last_progress = observed_at;
            observed_completions = completion.completed();
        }
        let elapsed = observed_at.duration_since(started);
        if elapsed >= next_sample {
            if progress.len() == 8 {
                let _oldest = progress.pop_front();
            }
            progress.push_back((elapsed.as_secs(), observed_completions));
            readiness_cost.sample(elapsed);
            next_sample = elapsed.saturating_add(Duration::from_secs(60));
        }
        let ready = polled.map_err(|error| -> Box<dyn std::error::Error> {
            Box::new(WorkStatusObservationError {
                context: format!(
                    "ANN materialization in {phase}: target sequence {completed_sequence:?}, last sequence {last_sequence:?}, last coverage {last_coverage:?}; bounded progress samples (elapsed seconds, observed completed inputs)={progress:?}, observed completed inputs={observed_completions}/{}, seconds since last observed completed input={}; work-status failure; original watchdog expired={}, remaining={}ms",
                    completion.total(),
                    last_progress.elapsed().as_secs(),
                    observed_at >= deadline,
                    deadline.saturating_duration_since(observed_at).as_millis(),
                ),
                cause: error,
            })
        })?;
        require(
            observed_at < deadline,
            format!(
                "ANN materialization watchdog expired during work-status reads in {phase}: target sequence {completed_sequence:?}, last sequence {last_sequence:?}, last coverage {last_coverage:?}; bounded progress samples (elapsed seconds, observed completed inputs)={progress:?}, observed completed inputs={observed_completions}/{}, seconds since last observed completed input={}",
                completion.total(),
                last_progress.elapsed().as_secs(),
            ),
        )?;
        if !ready {
            // A full 64-status page has at least a 500 ms observer pause,
            // except near the original deadline: at most 128 reads/s on
            // average after the initial page. This changes no work funding.
            thread::sleep(
                readiness_pace
                    .after_page(advanced, deadline.saturating_duration_since(Instant::now())),
            );
            continue;
        }
        phase = "read final latest view";
        let latest = client.read_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget().capped_at(deadline),
        )
        .with_context(|| {
            format!(
                "ANN materialization in {phase}: target sequence {completed_sequence:?}, last sequence {last_sequence:?}, last coverage {last_coverage:?}; bounded progress samples (elapsed seconds, observed completed inputs)={progress:?}, observed completed inputs={observed_completions}/{}, seconds since last observed completed input={}",
                completion.total(),
                last_progress.elapsed().as_secs(),
            )
        })?;
        last_sequence = Some(latest.identity.sequence);
        phase = "await published sequence";
        if latest.identity.sequence >= source_view.sequence
            && completed_sequence.is_none_or(|sequence| latest.identity.sequence >= sequence)
        {
            let probe = client
                .search_semantic(
                    ViewSelection::Exact(latest.identity),
                    completion.instance(),
                    quality_query(0, config)?,
                    &budget().capped_at(deadline),
                )
                .with_context(|| {
                    format!(
                        "ANN materialization probe coverage at sequence {}: target sequence {completed_sequence:?}, last coverage {last_coverage:?}; bounded progress samples (elapsed seconds, observed completed inputs)={progress:?}, observed completed inputs={observed_completions}/{}, seconds since last observed completed input={}",
                        latest.identity.sequence,
                        completion.total(),
                        last_progress.elapsed().as_secs(),
                    )
                })?;
            last_coverage = Some((latest.identity, probe.coverage));
            if probe.coverage.status == CoverageStatus::Complete
                && probe.coverage.desired.root == latest.source.owners
                && probe.coverage.covered == Some(probe.coverage.desired)
            {
                require(
                    Instant::now() < deadline,
                    format!(
                        "ANN materialization watchdog expired during final coverage probe: target sequence {completed_sequence:?}, last sequence {last_sequence:?}, last coverage {last_coverage:?}; bounded progress samples (elapsed seconds, observed completed inputs)={progress:?}, observed completed inputs={observed_completions}/{}, seconds since last observed completed input={}",
                        completion.total(),
                        last_progress.elapsed().as_secs(),
                    ),
                )?;
                readiness_cost.mark_success();
                return Ok(latest.identity);
            }
            phase = "await complete coverage";
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn quality_error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    for _depth in 0..16 {
        let Some(cause) = source else {
            return message;
        };
        let _written = write!(message, ": {cause}");
        source = cause.source();
    }
    if source.is_some() {
        message.push_str(": error chain truncated after 16 causes");
    }
    message
}

pub(super) fn quality_failure(
    phase: &str,
    error: &dyn std::error::Error,
    mut daemon: Daemon,
) -> Box<dyn std::error::Error> {
    let mut primary = format!("ANN quality {phase} failed: {}", quality_error_chain(error));
    let running = match daemon.0.try_wait() {
        Ok(Some(_status)) => false,
        Ok(None) => true,
        Err(observation) => {
            let _written = write!(
                primary,
                "; native daemon exit observation failed: {observation}"
            );
            true
        }
    };
    if running && let Err(cleanup) = daemon.stop_in_place() {
        let _written = write!(primary, "; native daemon cleanup also failed: {cleanup:#}");
    }
    quality_failure_after_cleanup(primary, daemon)
}

fn quality_failure_after_cleanup(
    mut primary: String,
    mut daemon: Daemon,
) -> Box<dyn std::error::Error> {
    let (exit, forced) = match daemon.0.try_wait() {
        Ok(Some(status)) => (Some(status), false),
        Ok(None) => {
            primary.push_str("; native daemon still running after cleanup");
            match daemon.0.kill() {
                Ok(()) => match daemon.0.wait() {
                    Ok(status) => (Some(status), true),
                    Err(error) => {
                        let _written = write!(primary, "; forced daemon wait failed: {error}");
                        (None, true)
                    }
                },
                Err(error) => {
                    let _written = write!(primary, "; forced daemon kill failed: {error}");
                    match daemon.0.try_wait() {
                        Ok(status) => (status, false),
                        Err(observation) => {
                            let _written = write!(
                                primary,
                                "; daemon exit observation after failed kill: {observation}"
                            );
                            (None, false)
                        }
                    }
                }
            }
        }
        Err(observation) => {
            let _written = write!(primary, "; daemon exit observation failed: {observation}");
            (None, false)
        }
    };
    if let Some(status) = exit {
        if forced {
            let _written = write!(
                primary,
                "; native daemon forced exit: {status}; graceful drain and trace footer unconfirmed"
            );
        } else {
            let _written = write!(primary, "; native daemon exit: {status}");
        }
        let stderr = daemon.stderr_reader();
        // Reopen before dropping the named temporary file. Exit is confirmed,
        // so Daemon::drop's Child::kill cannot signal a reused PID.
        drop(daemon);
        let stderr = match stderr {
            Ok(stderr) => stderr,
            Err(error) => {
                let _written = write!(primary, "; daemon diagnostic open failed: {error}");
                return primary.into();
            }
        };
        let mut diagnostic = String::new();
        match stderr.take(64 * 1024).read_to_string(&mut diagnostic) {
            Ok(bytes) => {
                let _written = write!(primary, "; bounded daemon diagnostics: {diagnostic}");
                if bytes == 64 * 1024 {
                    primary.push_str("; diagnostic may be truncated at 65536 bytes");
                }
            }
            Err(diagnostic_error) => {
                let _written = write!(primary, "; diagnostic read also failed: {diagnostic_error}");
            }
        }
    } else {
        primary.push_str("; daemon diagnostics skipped because exit was not confirmed");
    }
    primary.into()
}

fn quality_stop(phase: &str, mut daemon: Daemon) -> TestResult {
    if let Err(error) = daemon.stop_in_place() {
        return Err(quality_failure_after_cleanup(
            format!("ANN quality {phase} failed: {error:#}"),
            daemon,
        ));
    }
    // A successful final result may conceal failed attempts or slow renewals.
    // Relay only the existing bounded diagnostic records after confirmed exit.
    relay_owned_diagnostics(
        phase,
        daemon.stderr_reader()?,
        &mut std::io::stderr().lock(),
    )
}

fn relay_owned_diagnostics(
    phase: &str,
    reader: impl std::io::Read,
    output: &mut impl std::io::Write,
) -> TestResult {
    const LIMIT: u64 = 64 * 1024;
    let mut diagnostic = String::new();
    let bytes = reader.take(LIMIT).read_to_string(&mut diagnostic)?;
    for line in diagnostic.lines() {
        if line.contains("\"event\":\"native_owned_work_refusal\"")
            || line.contains("\"event\":\"native_owned_renewal\"")
            || line.contains("\"event\":\"native_owned_renewal_followup\"")
            || line.contains("\"event\":\"native_sqlite_vfs_refusal_observed\"")
            || line.contains("\"event\":\"native_effort_error_observed\"")
        {
            writeln!(
                output,
                "native-ann-diagnostic phase={phase:?} record={line}"
            )?;
        }
    }
    if u64::try_from(bytes)? == LIMIT {
        writeln!(
            output,
            "native-ann-diagnostic phase={phase:?} truncated_at={LIMIT}"
        )?;
    }
    Ok(())
}

#[test]
fn completed_ann_run_retains_owned_refusal_and_renewal_diagnostics() -> TestResult {
    let input = concat!(
        "searchd: native_boot_stage=config_preflight\n",
        "{\"event\":\"native_owned_work_refusal\",\"outcome\":\"Err(Cancelled)\"}\n",
        "{\"event\":\"native_owned_renewal\",\"elapsed_ms\":1900}\n",
        "{\"event\":\"native_owned_renewal_followup\",\"submit_outcome\":\"Terminal(Published)\",\"finish_outcome\":\"Ok\"}\n",
        "{\"event\":\"native_sqlite_vfs_refusal_observed\",\"origin\":\"core_error_observed\"}\n",
        "{\"event\":\"native_effort_error_observed\",\"first_local_denial\":\"Some(Reads)\"}\n",
        "native-object-io status=quiescent\n",
    );
    let mut output = Vec::new();
    relay_owned_diagnostics("diagnostic daemon shutdown", input.as_bytes(), &mut output)?;
    let actual = String::from_utf8(output)?;
    require_equal(
        &actual,
        &concat!(
            "native-ann-diagnostic phase=\"diagnostic daemon shutdown\" record={\"event\":\"native_owned_work_refusal\",\"outcome\":\"Err(Cancelled)\"}\n",
            "native-ann-diagnostic phase=\"diagnostic daemon shutdown\" record={\"event\":\"native_owned_renewal\",\"elapsed_ms\":1900}\n",
            "native-ann-diagnostic phase=\"diagnostic daemon shutdown\" record={\"event\":\"native_owned_renewal_followup\",\"submit_outcome\":\"Terminal(Published)\",\"finish_outcome\":\"Ok\"}\n",
            "native-ann-diagnostic phase=\"diagnostic daemon shutdown\" record={\"event\":\"native_sqlite_vfs_refusal_observed\",\"origin\":\"core_error_observed\"}\n",
            "native-ann-diagnostic phase=\"diagnostic daemon shutdown\" record={\"event\":\"native_effort_error_observed\",\"first_local_denial\":\"Some(Reads)\"}\n",
        ).to_string(),
        "successful run must preserve original failure records without boot/IO noise",
    )
}

#[test]
fn completed_ann_diagnostics_bound_reads_and_propagate_output_failure() -> TestResult {
    let input = vec![b'x'; 65_537];
    let mut reader = input.as_slice();
    let mut output = Vec::new();
    relay_owned_diagnostics("bounded", &mut reader, &mut output)?;
    require_equal(&reader.len(), &1, "diagnostic read bound")?;
    require_equal(
        &String::from_utf8(output)?,
        &"native-ann-diagnostic phase=\"bounded\" truncated_at=65536\n".to_string(),
        "truncation must be explicit",
    )?;
    let input = b"{\"event\":\"native_owned_renewal\"}\n";
    let mut no_capacity = std::io::Cursor::new([0_u8; 0]);
    require(
        relay_owned_diagnostics("broken output", input.as_slice(), &mut no_capacity).is_err(),
        "diagnostic output failure was hidden",
    )
}

struct QualitySweep {
    matches: usize,
    pages: Vec<Vec<String>>,
}

struct QualitySelection<'a> {
    view: ViewRef,
    source_owners: quanta_index_contract::index::MapRoot,
    instance: IndexInstanceId,
    config: &'a AnnRailConfig,
    rows: &'a [(String, Vec<f32>)],
}

fn check_quality_hit(
    owner: OwnerKey,
    score: f64,
    selection: &QualitySelection<'_>,
    direction: &[f32],
    unique_owners: &mut BTreeSet<String>,
) -> TestResult<String> {
    let id = quality_row_id(owner, selection.config)?;
    require(
        unique_owners.insert(id.clone()),
        format!("ANN duplicate hit owner {id}"),
    )?;
    let (_gold_id, vector) = selection
        .rows
        .iter()
        .find(|(gold_id, _vector)| gold_id == &id)
        .ok_or("ANN hit is outside selected eligible corpus")?;
    let gold_score: f64 = vector
        .iter()
        .zip(direction)
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum();
    require(
        (score - gold_score).abs() <= EXACT_SCORE_TOLERANCE,
        format!("ANN score mismatch for {id}: {score} != {gold_score}"),
    )?;
    Ok(id)
}

fn check_query_result(
    result: &NativeSemanticResult,
    selection: &QualitySelection<'_>,
    seed: u64,
) -> TestResult<(usize, Vec<String>)> {
    require_equal(&result.view, &selection.view, "ANN selected view")?;
    require_equal(
        &result.instance,
        &selection.instance,
        "ANN selected instance",
    )?;
    require_equal(
        &result.coverage.status,
        &CoverageStatus::Complete,
        "ANN source coverage",
    )?;
    require_equal(
        &result.coverage.covered,
        &Some(result.coverage.desired),
        "ANN covered input binding",
    )?;
    require_equal(
        &result.coverage.desired.root,
        &selection.source_owners,
        "ANN selected source owner root",
    )?;
    require(
        result.coverage.searchable.is_some(),
        "ANN searchable root absent",
    )?;
    require(
        matches!(
            result.traversal,
            NativeSemanticTraversal::Approximate { .. }
        ),
        format!("ANN query {seed} did not use the sealed Native path"),
    )?;
    require(
        !result.exhaustive,
        "ANN approximate query claimed exhaustion",
    )?;
    require_equal(
        &result.hits.len(),
        &selection.config.k,
        "ANN top-k page length",
    )?;
    require(
        result.scanned_owners <= u32::try_from(QUALITY_MAX_OWNERS)?,
        "ANN scanned beyond fixed candidate bound",
    )?;

    let direction = query_direction(seed, selection.config)?;
    let expected = exact_top_k(selection.rows, &direction, selection.config.k);
    require_equal(
        &expected.len(),
        &selection.config.k,
        "ANN exact golden length",
    )?;
    let mut unique_owners = BTreeSet::new();
    let mut page = Vec::with_capacity(selection.config.k);
    let mut matches = 0_usize;
    for hit in &result.hits {
        require_equal(
            &hit.vector_id.as_slice(),
            &b"whole-file".as_slice(),
            "ANN whole-file vector ID",
        )?;
        let id = check_quality_hit(
            hit.owner,
            hit.score.get(),
            selection,
            &direction,
            &mut unique_owners,
        )?;
        if expected.contains(&id) {
            matches = matches.checked_add(1).ok_or("ANN recall match overflow")?;
        }
        page.push(id);
    }
    Ok((matches, page))
}

fn quality_sweep(
    client: &NativeUdsClient,
    selected: ViewRef,
    instance: IndexInstanceId,
    config: &AnnRailConfig,
    rows: &[(String, Vec<f32>)],
    deadline: Instant,
) -> TestResult<QualitySweep> {
    let view = client.read_view(
        ViewSelection::Exact(selected),
        &budget().capped_at(deadline),
    )?;
    require_equal(&view.identity, &selected, "ANN selected catalog view")?;
    let selection = QualitySelection {
        view: selected,
        source_owners: view.source.owners,
        instance,
        config,
        rows,
    };
    let mut matches = 0_usize;
    let mut pages = Vec::new();
    for seed in 0..config.queries {
        require(
            Instant::now() < deadline,
            format!("ANN query watchdog expired at seed {seed}"),
        )?;
        let result = client
            .search_semantic(
                ViewSelection::Exact(selected),
                instance,
                quality_query(seed, config)?,
                &budget().capped_at(deadline),
            )
            .with_context(|| format!("ANN selected query failed at seed {seed}"))?;
        let (overlap, page) = check_query_result(&result, &selection, seed).map_err(|error| {
            format!("ANN selected query result failed at seed {seed}: {error:#}")
        })?;
        matches = matches
            .checked_add(overlap)
            .ok_or("ANN recall total overflow")?;
        pages.push(page);
    }
    let possible = usize::try_from(config.queries)?
        .checked_mul(config.k)
        .ok_or("ANN recall denominator overflow")?;
    require_equal(
        &pages.len(),
        &usize::try_from(config.queries)?,
        "ANN query count",
    )?;
    let recall = f64::from(u32::try_from(matches)?) / f64::from(u32::try_from(possible)?);
    require(
        recall >= config.recall_floor,
        format!(
            "Native ANN recall@{} {recall:.6} < {:.6} ({matches}/{possible})",
            config.k, config.recall_floor
        ),
    )?;
    require(
        Instant::now() < deadline,
        "ANN quality watchdog expired after final query",
    )?;
    Ok(QualitySweep { matches, pages })
}

fn check_provider_inputs(
    inputs: &[String],
    config: &AnnRailConfig,
    require_rows: bool,
) -> TestResult {
    let mut rows = BTreeSet::new();
    let mut queries = BTreeSet::new();
    for input in inputs {
        if let Some(seed) = input.strip_prefix("row-") {
            let seed = seed.parse::<u64>()?;
            require(
                seed < config.rows && input == &format!("row-{seed}"),
                "ANN provider observed foreign row",
            )?;
            let _prior = rows.insert(seed);
        } else if let Some(seed) = input.strip_prefix("query-") {
            let seed = seed.parse::<u64>()?;
            require(
                seed < config.queries && input == &format!("query-{seed}"),
                "ANN provider observed foreign query",
            )?;
            let _prior = queries.insert(seed);
        } else {
            return Err(format!("ANN provider observed unknown input {input:?}").into());
        }
    }
    if require_rows {
        require_equal(
            &rows.len(),
            &usize::try_from(config.rows)?,
            "ANN provider corpus delivery",
        )?;
    } else {
        require(
            rows.is_empty(),
            "ANN reopen unexpectedly re-embedded corpus rows",
        )?;
    }
    require_equal(
        &queries.len(),
        &usize::try_from(config.queries)?,
        "ANN provider query delivery",
    )
}

#[derive(Debug, Eq, PartialEq)]
struct ProviderCallCounts {
    row_total: usize,
    row_unique: usize,
    query_total: usize,
    query_unique: usize,
    other_total: usize,
}

impl ProviderCallCounts {
    fn from_inputs(inputs: &[String]) -> Self {
        let mut rows = BTreeSet::new();
        let mut queries = BTreeSet::new();
        let mut counts = Self {
            row_total: 0,
            row_unique: 0,
            query_total: 0,
            query_unique: 0,
            other_total: 0,
        };
        for input in inputs {
            if input.starts_with("row-") {
                counts.row_total = counts.row_total.saturating_add(1);
                let _inserted = rows.insert(input.as_str());
            } else if input.starts_with("query-") {
                counts.query_total = counts.query_total.saturating_add(1);
                let _inserted = queries.insert(input.as_str());
            } else {
                counts.other_total = counts.other_total.saturating_add(1);
            }
        }
        counts.row_unique = rows.len();
        counts.query_unique = queries.len();
        counts
    }

    fn report(&self, phase: &str) -> String {
        format!(
            "native-ann-provider phase={phase:?} rows=(unique:{},total:{},duplicates:{}) queries=(unique:{},total:{},duplicates:{}) other_total={}",
            self.row_unique,
            self.row_total,
            self.row_total.saturating_sub(self.row_unique),
            self.query_unique,
            self.query_total,
            self.query_total.saturating_sub(self.query_unique),
            self.other_total,
        )
    }
}

fn report_provider_inputs(phase: &str, inputs: &[String]) {
    let counts = ProviderCallCounts::from_inputs(inputs);
    let _written = writeln!(std::io::stderr().lock(), "{}", counts.report(phase));
}

fn bounded_provider_error(error: &str) -> (String, bool) {
    let mut characters = error.chars();
    let prefix = characters.by_ref().take(512).collect();
    (prefix, characters.next().is_some())
}

fn report_provider_from_stub(phase: &str, stub: &EmbeddingStub, after: usize) {
    match stub.observed_inputs() {
        Ok(inputs) => match inputs.get(after..) {
            Some(phase_inputs) => report_provider_inputs(phase, phase_inputs),
            None => {
                let _written = writeln!(
                    std::io::stderr().lock(),
                    "native-ann-provider phase={phase:?} observation_error=history_regressed"
                );
            }
        },
        Err(error) => {
            let error = error.to_string();
            let (bounded_error, truncated) = bounded_provider_error(&error);
            let _written = writeln!(
                std::io::stderr().lock(),
                "native-ann-provider phase={phase:?} observation_error={bounded_error:?} truncated={truncated}"
            );
        }
    }
}

#[test]
fn provider_cost_counts_duplicates_without_changing_coverage_oracle() -> TestResult {
    let inputs = [
        "row-1".to_owned(),
        "query-0".to_owned(),
        "row-1".to_owned(),
        "row-2".to_owned(),
        "query-0".to_owned(),
        "foreign".to_owned(),
    ];
    let counts = ProviderCallCounts::from_inputs(&inputs);
    require_equal(
        &counts,
        &ProviderCallCounts {
            row_total: 3,
            row_unique: 2,
            query_total: 2,
            query_unique: 1,
            other_total: 1,
        },
        "ANN provider duplicate cost observation",
    )?;
    require_equal(
        &counts.report("unit"),
        &"native-ann-provider phase=\"unit\" rows=(unique:2,total:3,duplicates:1) queries=(unique:1,total:2,duplicates:1) other_total=1".to_owned(),
        "ANN provider cost report omitted a duplicate class",
    )
}

#[test]
fn provider_observation_error_marks_exact_truncation_boundary() -> TestResult {
    let exact = "x".repeat(512);
    require_equal(
        &bounded_provider_error(&exact),
        &(exact.clone(), false),
        "ANN provider exact diagnostic boundary",
    )?;
    require_equal(
        &bounded_provider_error(&(exact.clone() + "y")),
        &(exact, true),
        "ANN provider truncated diagnostic boundary",
    )
}

#[test]
fn product_native_ann_quality_4096x64_reopens_at_fixed_recall() -> TestResult {
    let config = AnnRailConfig::DEFAULT;
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-ann-quality.json");
    let stub = EmbeddingStub::start_ann_quality()?;
    let base_url = stub.base_url();
    let mut configuration =
        config_with_openai_semantic_dimension(&root, &socket, u32::try_from(config.width)?)?;
    set_fixture_json(
        &mut configuration,
        "/semantic/read/max_owners",
        serde_json::json!(QUALITY_MAX_OWNERS),
    )?;
    configure_semantic_endpoint(&mut configuration, &base_url)?;
    configure_owned_semantic(&mut configuration)?;
    configure_effort(&mut configuration)?;
    require(
        configuration
            .get("retention")
            .is_none_or(serde_json::Value::is_null),
        "ANN work readiness requires completed work to remain observable",
    )?;
    let prior_overlay = configuration
        .as_object_mut()
        .ok_or("ANN quality config is not an object")?
        .insert(
            "overlay".into(),
            serde_json::json!({
                "text_instance": vec![71_u8; 16],
                "sessions": 2,
                "documents": 4,
                "document_bytes": 4096,
                "total_bytes": 16384
            }),
        );
    require(
        prior_overlay.is_none(),
        "ANN quality overlay config already exists",
    )?;
    fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
    let semantic = openai_semantic_spec_dimension(&base_url, u32::try_from(config.width)?)?;
    // Optional diagnosis uses the deployed capture path, leaving the ordinary
    // quality gate's observer disabled. The caller supplies an existing directory.
    let trace_directory = std::env::var_os("QIDX_NATIVE_ANN_TRACE_DIR").map(PathBuf::from);
    let first_trace = trace_directory
        .as_ref()
        .map(|directory| directory.join("ann-initial.qtrace"));
    let mut daemon = Daemon::start_with_api_key_and_trace(
        &config_path,
        &socket,
        Some("test-key"),
        first_trace.as_deref(),
    )?;
    let client = NativeUdsClient::new(socket.clone());
    let phase = Cell::new("workspace creation");
    let setup = (|| -> TestResult<ViewRef> {
        let NativeIndexResponse::Created(genesis) = client.request(
            NativeIndexRequest::CreateWorkspace {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?
        else {
            return Err("ANN quality workspace creation failed".into());
        };
        phase.set("profile ingress");
        let ingress = client.begin_input(operation(1), &budget())?;
        phase.set("profile staging");
        let profile = client.stage_profile(
            ingress,
            IndexProfile {
                instances: vec![text_spec(), semantic.clone()],
            },
            &budget(),
        )?;
        phase.set("profile Reconfigure");
        published(&client.submit(
            UpdateRequest {
                operation: operation(1),
                expected_view: genesis,
                kind: UpdateKind::Reconfigure { profile },
            },
            &budget(),
        )?)
    })();
    let configured = match setup {
        Ok(configured) => configured,
        Err(error) => {
            report_provider_from_stub("setup failure", &stub, 0);
            return Err(quality_failure(phase.get(), error.as_ref(), daemon));
        }
    };

    // The watchdog bounds this test process; it does not modify SDK request
    // funding or the owned worker's ten-second assignment deadline.
    let deadline = Instant::now()
        .checked_add(QUALITY_WATCHDOG)
        .ok_or("ANN quality watchdog overflow")?;
    phase.set("source ingress and SourceAdvance");
    let initial = (|| -> TestResult<_> {
        let staged = stage_quality_sources(&client, configured, &config, deadline, |seed| {
            format!("row-{seed}")
        })
        .map_err(|error| -> Box<dyn std::error::Error> {
            let status = match daemon.0.try_wait() {
                Ok(None) => format!("{:?}", client.status(operation(2), &budget())),
                Ok(Some(exit)) => format!("unavailable because daemon exited {exit}"),
                Err(observation) => {
                    format!("unavailable: daemon exit check failed: {observation}")
                }
            };
            format!(
                "{}; original operation status: {status}",
                quality_error_chain(error.as_ref())
            )
            .into()
        })?;
        phase.set("materialization");
        let selected = wait_for_complete(
            &client,
            &mut daemon,
            staged.view,
            work_status::CompletionProgress::new(&staged.sources, staged.view, &semantic)?,
            &config,
            Some(config.rows),
            deadline,
        )
        .map_err(|error| {
            work_status::with_snapshot(error.as_ref(), &client, &semantic, &staged.sources)
        })?;
        phase.set("selected source inventory");
        verify_selected_sources(&client, selected, &staged.sources, deadline)?;
        let rows = corpus(&config);
        phase.set("initial ANN quality sweep");
        let first = quality_sweep(
            &client,
            selected,
            semantic.instance,
            &config,
            &rows,
            deadline,
        )?;
        phase.set("initial provider observations");
        let before_inputs = stub.observed_inputs()?;
        report_provider_inputs("initial through quality sweep", &before_inputs);
        check_provider_inputs(&before_inputs, &config, true)?;
        require(
            Instant::now() < deadline,
            "ANN initial quality watchdog expired after provider observations",
        )?;
        Ok((selected, staged.sources, rows, first, before_inputs))
    })();
    let (selected, sources, rows, first, before_inputs) = match initial {
        Ok(result) => result,
        Err(error) => {
            report_provider_from_stub("initial failure", &stub, 0);
            return Err(quality_failure(phase.get(), error.as_ref(), daemon));
        }
    };

    quality_stop("initial daemon shutdown", daemon)?;
    let reopen_trace = trace_directory
        .as_ref()
        .map(|directory| directory.join("ann-reopen.qtrace"));
    let mut reopened = Daemon::start_with_api_key_and_trace(
        &config_path,
        &socket,
        Some("test-key"),
        reopen_trace.as_deref(),
    )
    .map_err(|error| format!("ANN reopen daemon startup failed: {error:#}"))?;
    let restored = NativeUdsClient::new(socket);
    let phase = Cell::new("reopened selected source inventory");
    let replay = (|| -> TestResult<usize> {
        verify_selected_sources(&restored, selected, &sources, deadline)?;
        phase.set("reopened ANN quality sweep");
        let second = quality_sweep(
            &restored,
            selected,
            semantic.instance,
            &config,
            &rows,
            deadline,
        )?;
        require_equal(&second.matches, &first.matches, "ANN recall after restart")?;
        require_equal(
            &second.pages,
            &first.pages,
            "ANN selected pages after restart",
        )?;
        phase.set("reopened provider observations");
        let after_inputs = stub.observed_inputs()?;
        let reopened_inputs = after_inputs
            .get(before_inputs.len()..)
            .ok_or("ANN provider input history regressed")?;
        report_provider_inputs("cold reopen quality sweep", reopened_inputs);
        check_provider_inputs(reopened_inputs, &config, false)?;
        require(
            Instant::now() < deadline,
            "ANN cold-reopen quality watchdog expired after provider observations",
        )?;
        Ok(after_inputs.len())
    })();
    let after_reopen_input_count = match replay {
        Ok(count) => count,
        Err(error) => {
            report_provider_from_stub("cold reopen failure", &stub, before_inputs.len());
            return Err(quality_failure(phase.get(), error.as_ref(), reopened));
        }
    };

    // Selected path, delta and overlay requests share the original absolute
    // watchdog with source/materialization/reopen, including the phase handoff.
    phase.set("selected original public exact-path quality");
    let changed = with_original_quality_deadline(deadline, |changed_deadline| {
        check_exact_path_query(
            &restored,
            selected,
            semantic.instance,
            &config,
            "src/0000.rs",
            Some((quality_owner(0, &config)?, selected_vector(&rows, "row-0")?)),
            changed_deadline,
        )?;
        phase.set("incremental replacement and deletion");
        let delta = stage_quality_delta(
            &restored,
            selected,
            &sources,
            &rows,
            &config,
            changed_deadline,
        )?;
        phase.set("incremental selected coverage");
        let changed_view = wait_for_complete(
            &restored,
            &mut reopened,
            delta.source_view,
            work_status::CompletionProgress::new(
                &delta.materialized_sources,
                delta.source_view,
                &semantic,
            )?,
            &config,
            Some(1),
            changed_deadline,
        )
        .map_err(|error| {
            work_status::with_snapshot(error.as_ref(), &restored, &semantic, &delta.sources)
        })?;
        phase.set("incremental selected source inventory");
        verify_selected_sources(&restored, changed_view, &delta.sources, changed_deadline)?;
        phase.set("incremental selected ANN quality sweep");
        let changed_sweep = quality_sweep(
            &restored,
            changed_view,
            semantic.instance,
            &config,
            &delta.rows,
            changed_deadline,
        )?;
        phase.set("incremental public exact-path quality");
        check_exact_path_query(
            &restored,
            changed_view,
            semantic.instance,
            &config,
            "src/0000.rs",
            Some((
                quality_owner(0, &config)?,
                selected_vector(&delta.rows, "row-0")?,
            )),
            changed_deadline,
        )?;
        check_exact_path_query(
            &restored,
            changed_view,
            semantic.instance,
            &config,
            "src/0122.rs",
            None,
            changed_deadline,
        )?;
        let changed_gold = exact_top_k(&delta.rows, &query_direction(2, &config)?, config.k);
        require(
            changed_gold.first().is_some_and(|id| id == "row-0")
                && changed_sweep
                    .pages
                    .get(2)
                    .is_some_and(|page| page.iter().any(|id| id == "row-0")),
            "ANN replacement direction was not visible at the selected view",
        )?;
        let after_delta_inputs = stub.observed_inputs()?;
        let delta_inputs = after_delta_inputs
            .get(after_reopen_input_count..)
            .ok_or("ANN provider input history regressed after delta")?;
        report_provider_inputs("selected path and delta", delta_inputs);
        check_delta_provider_inputs(delta_inputs)?;

        phase.set("unsaved overlay selected ANN quality sweep");
        let publisher = restored.begin_interaction_publisher(
            Duration::from_secs(60),
            &budget().capped_at(changed_deadline),
        )?;
        let session = NativeOverlaySession {
            workspace: workspace(),
            publisher,
            epoch: 1,
        };
        require_equal(
            &restored.overlay(
                NativeOverlayRequest::Begin { session },
                &budget().capped_at(changed_deadline),
            )?,
            &NativeOverlayReply::Begun(session),
            "ANN overlay session begin",
        )?;
        let document = NativeOverlayDocument {
            session,
            owner: quality_owner(0, &config)?,
        };
        require_equal(
            &restored.update_overlay_at_path(
                document,
                None,
                1,
                Some("row-0".into()),
                Some("src/0000.rs".into()),
                &budget().capped_at(changed_deadline),
            )?,
            &NativeOverlayReply::Updated {
                document,
                revision: 1,
            },
            "ANN unsaved replacement revision",
        )?;
        let tombstone = NativeOverlayDocument {
            session,
            owner: quality_owner(1, &config)?,
        };
        require_equal(
            &restored.update_overlay_at_path(
                tombstone,
                None,
                1,
                None,
                None,
                &budget().capped_at(changed_deadline),
            )?,
            &NativeOverlayReply::Updated {
                document: tombstone,
                revision: 1,
            },
            "ANN unsaved delete revision",
        )?;
        let mut overlay_rows = delta.rows.clone();
        let (_id, vector) = overlay_rows
            .iter_mut()
            .find(|(id, _vector)| id == "row-0")
            .ok_or("ANN unsaved row gold absent")?;
        *vector = unit_direction(0, config.width);
        overlay_rows.retain(|(id, _vector)| id != "row-1");
        require_equal(
            &overlay_rows.len(),
            &4094_usize,
            "ANN overlay independent eligible row count",
        )?;
        overlay_quality_sweep(
            &restored,
            changed_view,
            semantic.instance,
            session,
            &config,
            &overlay_rows,
            (document.owner, tombstone.owner),
            changed_deadline,
        )?;
        let overlay_inputs = stub.observed_inputs()?;
        let overlay_only = overlay_inputs
            .get(after_delta_inputs.len()..)
            .ok_or("ANN provider input history regressed after overlay")?;
        report_provider_inputs("unsaved overlay", overlay_only);
        require(
            overlay_only.iter().any(|input| input == "row-0")
                && overlay_only
                    .iter()
                    .filter(|input| input.starts_with("row-"))
                    .all(|input| input == "row-0"),
            "ANN overlay provider input text differs from the unsaved row",
        )?;
        require_equal(
            &restored.overlay(
                NativeOverlayRequest::Close { session },
                &budget().capped_at(changed_deadline),
            )?,
            &NativeOverlayReply::Closed(session),
            "ANN overlay session close",
        )?;
        check_exact_path_query(
            &restored,
            changed_view,
            semantic.instance,
            &config,
            "src/0000.rs",
            Some((
                quality_owner(0, &config)?,
                selected_vector(&delta.rows, "row-0")?,
            )),
            changed_deadline,
        )?;
        check_exact_path_query(
            &restored,
            changed_view,
            semantic.instance,
            &config,
            "src/0001.rs",
            Some((
                quality_owner(1, &config)?,
                selected_vector(&delta.rows, "row-1")?,
            )),
            changed_deadline,
        )?;
        require(
            Instant::now() < changed_deadline,
            "ANN selected-change watchdog expired after durable post-overlay queries",
        )
    });
    if let Err(error) = changed {
        report_provider_from_stub("selected changes failure", &stub, after_reopen_input_count);
        return Err(quality_failure(phase.get(), error.as_ref(), reopened));
    }
    quality_stop("reopened daemon shutdown", reopened)?;
    stub.finish()
}
