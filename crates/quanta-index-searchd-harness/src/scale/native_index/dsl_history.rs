//! Query the DSL History fixture through an existing exact Published view.

use std::{
    fs::{self, File, OpenOptions},
    io::Read as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, ensure};
use nix::fcntl::OFlag;
use quanta_index_contract::{
    HistoryIngestBatch, HistoryOrderV1, HistoryQueryRequest, TextQuerySyntax,
    index::{HistorySourceCutV1, InstanceSpec, ViewRef, ViewSelection},
    native_protocol::{NativeHistoryPage, NativeReadSessionHistoryCutV1},
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

use super::{
    NativeDaemon, NativeExecutable,
    dsl_text::NativeDslTiming,
    hex_sha256,
    retained_read::{QUERY_DEADLINE, text_query},
};
use crate::{
    artifact::{BenchSyntax, GitHeadV1, LatencySummary, ResultShape},
    history_fixture::HISTORY_LEDGER,
    scenarios::{DslBenchScenario, FixtureKind, TOP_K, scenario_by_id},
};

pub(super) const HISTORY_SAMPLES_SCHEMA: &str = "native-dsl-history-samples-v2";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishedHistoryFixture {
    socket: PathBuf,
    view: ViewRef,
    instance: InstanceSpec,
    cut: HistorySourceCutV1,
    // The fixture supplies the seed; each invocation opens its own session.
    open_key: [u8; 32],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColdHistoryFixture {
    history: PublishedHistoryFixture,
    daemon: PathBuf,
    daemon_sha256: String,
    config: PathBuf,
    config_sha256: String,
    source_git_head: Option<String>,
}

fn read_native_config(path: &Path) -> Result<Vec<u8>> {
    const MAX_CONFIG_BYTES: u64 = 64 * 1024;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW).bits())
        .open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "Native History config must be a regular file"
    );
    let mut bytes = Vec::new();
    let size = file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        u64::try_from(size)? <= MAX_CONFIG_BYTES,
        "Native History config exceeds 64 KiB"
    );
    Ok(bytes)
}

/// Measure one of the DSL History queries against the producer's Published member.
/// The input JSON binds the socket, exact view, instance, cut and open-key seed.
pub fn measure_native_dsl_history(
    input_path: &Path,
    scenario_id: &str,
    timing: NativeDslTiming,
) -> Result<Value> {
    let input = serde_json::from_reader(File::open(input_path)?)?;
    measure_published_history(&input, scenario_id, timing)
}

/// Reopen the producer's isolated fixture and measure the first History query.
/// This measures a fresh process, with the operating system's cache state left as-is.
pub fn measure_cold_native_dsl_history(input_path: &Path, scenario_id: &str) -> Result<Value> {
    measure_reopened_history(
        input_path,
        scenario_id,
        NativeDslTiming::PROCESS_COLD,
        "native-dsl-history-process-cold-v1",
    )
}

/// Reopen the same published fixture and sample warm queries against its exact view.
pub fn measure_reopened_native_dsl_history(
    input_path: &Path,
    scenario_id: &str,
    timing: NativeDslTiming,
) -> Result<Value> {
    measure_reopened_history(
        input_path,
        scenario_id,
        timing,
        "native-dsl-history-process-warm-v1",
    )
}

fn measure_reopened_history(
    input_path: &Path,
    scenario_id: &str,
    timing: NativeDslTiming,
    schema: &'static str,
) -> Result<Value> {
    let input: ColdHistoryFixture = serde_json::from_reader(File::open(input_path)?)?;
    if let Some(head) = input.source_git_head.as_deref() {
        ensure!(
            GitHeadV1::parse(head)?.as_str() == head,
            "Native History source HEAD must be canonical"
        );
    }
    ensure!(
        input.config.is_absolute() && input.history.socket.is_absolute(),
        "Native History config and socket must be absolute",
    );
    let executable = NativeExecutable::capture(&input.daemon)?;
    ensure!(
        executable.sha256() == input.daemon_sha256.as_str(),
        "Native History daemon digest differs"
    );
    let config_bytes = read_native_config(&input.config)?;
    ensure!(
        hex_sha256(&config_bytes) == input.config_sha256,
        "Native History config digest differs"
    );
    let config: Value = serde_json::from_slice(&config_bytes)?;
    let socket_path = config
        .get("socket_path")
        .and_then(Value::as_str)
        .context("missing daemon socket path")?;
    ensure!(
        Path::new(socket_path) == input.history.socket.as_path(),
        "Native History socket differs from daemon config"
    );
    ensure!(
        !input.history.socket.try_exists()?,
        "Native History cold fixture socket is already present"
    );
    ensure!(
        config.get("read_sessions").is_some_and(Value::is_object),
        "Native History cold fixture requires configured read sessions",
    );
    let temp = TempDir::new()?;
    let pinned_config = temp.path().join("native.json");
    fs::write(&pinned_config, &config_bytes)?;
    let stderr = temp.path().join("history.stderr");
    let daemon = NativeDaemon::start(
        &executable,
        &pinned_config,
        &input.history.socket,
        None,
        0,
        &stderr,
    )?;
    let measured = measure_published_history(&input.history, scenario_id, timing)
        .and_then(|report| Ok((report, daemon.linux_peak_rss_bytes()?)));
    match (measured, daemon.stop(&mut None)) {
        (Ok((report, peak_rss_bytes)), Ok(())) => {
            ensure!(
                read_native_config(&input.config)? == config_bytes,
                "Native History config changed during measurement"
            );
            Ok(json!({
                "schema": schema,
                "daemon_sha256": input.daemon_sha256,
                "daemon_peak_rss_bytes": peak_rss_bytes,
                "config_sha256": input.config_sha256,
                "history": report,
            }))
        }
        (Err(cause), Ok(())) | (Ok(_), Err(cause)) => Err(cause),
        (Err(cause), Err(drain)) => {
            Err(cause.context(format!("Native History daemon drain failed: {drain:#}")))
        }
    }
}

fn new_open_key(seed: [u8; 32]) -> Result<[u8; 32]> {
    ensure!(
        seed != [0; 32],
        "Native History open-key seed must be nonzero"
    );
    let mut entropy = [0; 32];
    File::open("/dev/urandom")?
        .read_exact(&mut entropy)
        .context("read Native History session entropy")?;
    // A released key can still replay its previous session in the daemon.
    let key = Sha256::new()
        .chain_update(seed)
        .chain_update(entropy)
        .finalize();
    Ok(key.into())
}

pub(super) fn history_query(scenario: &DslBenchScenario) -> HistoryQueryRequest {
    let mut text_query = text_query(&scenario.query.resolve(), TOP_K);
    text_query.syntax = match scenario.syntax {
        BenchSyntax::Native => TextQuerySyntax::Native,
        BenchSyntax::Sourcegraph => TextQuerySyntax::Sourcegraph,
    };
    HistoryQueryRequest {
        text_query,
        order: HistoryOrderV1::Recency,
        cursor: None,
    }
}

fn measure_published_history(
    input: &PublishedHistoryFixture,
    scenario_id: &str,
    timing: NativeDslTiming,
) -> Result<Value> {
    let scenario = scenario_by_id(scenario_id).context("unregistered DSL scenario")?;
    ensure!(
        scenario.fixture == FixtureKind::HistoryLedger,
        "{scenario_id} does not use the History fixture"
    );
    ensure!(
        input.socket.is_absolute(),
        "Native History socket must be absolute"
    );
    let generation = &input.cut.generation;
    let fixture = HISTORY_LEDGER.build_batch(
        generation.repo_id.clone(),
        generation.revision_id.clone(),
        generation.generation,
    )?;
    let query = history_query(scenario);
    let sdk = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
        &input.socket,
        ViewSelection::Exact(input.view),
    ))?;
    let budget = || RequestBudgetV1::for_duration(QUERY_DEADLINE);
    let mut session = sdk
        .reader()
        .native()?
        .open(new_open_key(input.open_key)?, &budget())?;
    let run = (|| -> Result<Value> {
        let _profile = session.load_profile(&budget())?;
        let mut witness = None;
        let mut observed_page = None;
        let mut samples_ms = Vec::new();
        let keep_warm_session = timing.prime_queries > 0;
        let request_timeout_ms = u64::try_from(QUERY_DEADLINE.as_millis())?;
        for _ in 0..timing.passes {
            samples_ms.extend(timing.sample_pass(|| {
                if keep_warm_session {
                    let now_ms =
                        u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
                    if now_ms.saturating_add(request_timeout_ms) >= session.status().expires_at_ms {
                        session.renew(&budget())?;
                    }
                }
                let query = query.clone();
                let request_budget = budget();
                let started = Instant::now();
                let page = session.history(&input.instance, query, None, &request_budget)?;
                let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
                // Reading the cut opens History metadata. Capture it only after
                // the first History query returns so the cold sample includes it.
                if witness.is_none() {
                    let observed = session.history_cut(&input.instance, &budget())?;
                    ensure!(
                        observed.view == input.view && observed.cut == input.cut,
                        "Published History fixture cut differs from the selected member",
                    );
                    witness = Some(observed);
                }
                let pinned = witness
                    .as_ref()
                    .context("missing Published History witness")?;
                verify_sample(&page, pinned, &fixture, scenario.expected_shape)
                    .with_context(|| format!("Native History DSL scenario {scenario_id}"))?;
                // The pass ends with measured queries, after any priming.
                // Keep one page so the reported witness is from a timed query.
                observed_page = Some(page);
                Ok(latency_ms)
            })?);
        }
        let latency =
            LatencySummary::from_samples_ms(&samples_ms).context("empty History sample set")?;
        Ok(json!({
            "schema": HISTORY_SAMPLES_SCHEMA,
            "scenario_id": scenario_id,
            "query": query,
            "result_shape": scenario.expected_shape.as_str(),
            "published": witness.context("missing Published History witness")?,
            "observed_page": observed_page.context("missing verified History page")?,
            "passes": timing.passes,
            "samples_per_pass": timing.samples,
            "prime_queries": timing.prime_queries,
            "cooldown_ms": timing.cooldown.as_secs_f64() * 1000.0,
            "pass_settle_ms": timing.pass_settle.as_secs_f64() * 1000.0,
            "samples_ms": samples_ms,
            "latency": latency,
        }))
    })();
    match (run, session.release(&budget())) {
        (Ok(report), Ok(_)) => Ok(report),
        (Err(cause), Ok(_)) => Err(cause),
        (Ok(_), Err(release)) => Err(release.into()),
        (Err(cause), Err(release)) => {
            Err(cause.context(format!("History session release failed: {release}")))
        }
    }
}

pub(super) fn verify_sample(
    page: &NativeHistoryPage,
    witness: &NativeReadSessionHistoryCutV1,
    fixture: &HistoryIngestBatch,
    shape: ResultShape,
) -> Result<()> {
    ensure!(
        page.view == witness.view
            && page.session == witness.session
            && page.member == witness.member
            && page.instance == witness.instance
            && page.read_epoch == witness.cut.durable_epoch
            && page.matched == 1
            && page.remaining == 1
            && page.after.is_none(),
        "History view, member, cut or result cardinality differs",
    );
    match (shape, page.commits.as_slice(), page.diffs.as_slice()) {
        (ResultShape::Commits, [row], []) => {
            let expected = fixture
                .commits
                .first()
                .context("missing commit in History fixture")?;
            let committed_at = i64::try_from(expected.committer_time_ms.div_euclid(1_000))?;
            ensure!(
                row.sha == expected.sha
                    && row.parent_ids == expected.parents
                    && row.committed_at_unix_s == committed_at
                    && row.author == expected.author.as_ref()
                    && row.committer == expected.committer.as_ref()
                    && row.message == expected.message.as_ref()
                    && row.is_merge == expected.is_merge
                    && row
                        .tags
                        .iter()
                        .map(String::as_str)
                        .eq(expected.tags.iter().map(|tag| tag.as_ref())),
                "History commit payload differs",
            );
        }
        (ResultShape::DiffPaths, [], [row]) => {
            let expected = fixture
                .diff_hunks
                .first()
                .context("missing diff in History fixture")?;
            ensure!(
                row.repo_relative_path == expected.file_path.as_ref()
                    && row.hunk_header == expected.record.hunk_header.as_ref()
                    && row.side == expected.record.side
                    && row.line_start == expected.record.byte_start
                    && row.line_end == expected.record.byte_end
                    && row.snippet == expected.record.touched_text.as_ref(),
                "History diff payload differs",
            );
        }
        _ => anyhow::bail!("History query returned a different row shape"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_fixture_reuse_opens_distinct_sessions() -> Result<()> {
        let first = new_open_key([1; 32])?;
        let next = new_open_key([1; 32])?;
        ensure!(first != [0; 32] && next != [0; 32] && first != next);
        ensure!(new_open_key([0; 32]).is_err());
        Ok(())
    }

    #[test]
    fn history_config_copy_requires_bounded_regular_input() -> Result<()> {
        let temp = TempDir::new()?;
        let config = temp.path().join("native.json");
        fs::write(&config, b"{}")?;
        ensure!(read_native_config(&config)? == b"{}");
        let alias = temp.path().join("alias.json");
        std::os::unix::fs::symlink(&config, &alias)?;
        ensure!(read_native_config(&alias).is_err());
        fs::write(&config, vec![0; 64 * 1024 + 1])?;
        ensure!(read_native_config(&config).is_err());
        Ok(())
    }
}
