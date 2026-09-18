//! W5 phase 3 — a request budget that runs out inside the dense lane is
//! answered from the lane, counted, and does not poison the daemon.
//!
//! Through the daemon's own front door: the query socket admits dispatches
//! under a short budget, and the semantic adapter's debug hold parks the
//! dense lane before it issues its vector query — the one deterministic way
//! to have a request end *inside* the lane, where a naturally slow query
//! would be a timing guess. The deadline is then observed by the lane's
//! watcher and nowhere else, so the typed answer must carry the lane's
//! checkpoint; the scrape must count one interruption in that lane and no
//! query issued for it; and the next semantic query, once the hold is
//! released, must serve on the same daemon and the same opened generation.
//!
//! The oracles are outside the accounting under test: the wire code and
//! message, the counters read back through the control socket against the
//! number of queries this test sent, and the served query's own ranking.

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::Duration;

use quanta_index_contract::MetricsSnapshotV1;
use quanta_index_core::REQUEST_DEADLINE_EXCEEDED_CODE;
use quanta_index_ipc::ServerAdmissionPolicy;
use quanta_index_searchd_harness as e2e_harness;
use quanta_index_semantic::test_support;

use e2e_harness::{E2eQueryResult, E2eRuntime, E2eTextChunkSpec};

type TestResult = Result<(), Box<dyn Error>>;

/// Files in the corpus: well below the approximate-index floor, so the
/// generation seals an exact lane and the checkpoint is `semantic:exact`.
const FILES: u32 = 4;
const TOP_K: u32 = 3;
/// The dispatch budget the daemon runs every query under here. Long enough
/// for the route to reach the lane on a contended host, short enough for
/// the held request to end inside one test.
const DISPATCH_BUDGET: Duration = Duration::from_secs(2);

/// One file's text: five tokens no other file shares, so the daemon's
/// token-hashing embedder makes the file's own text its nearest neighbour.
fn file_content(index: u32) -> String {
    format!("fn bd{index}a() {{ let bd{index}b = bd{index}c(bd{index}d, bd{index}e); }}")
}

fn file_path(index: u32) -> String {
    format!("src/budget/item_{index:03}.rs")
}

fn ingest_all_files_one_batch(rt: &mut E2eRuntime) -> Result<Vec<String>, Box<dyn Error>> {
    let contents: Vec<(String, String)> = (0..FILES)
        .map(|index| (file_path(index), file_content(index)))
        .collect();
    let chunks: Vec<[E2eTextChunkSpec<'_>; 1]> = contents
        .iter()
        .map(|(_, content)| {
            [E2eTextChunkSpec {
                content,
                start_line: 1,
                end_line: 1,
                source_repo_id: None,
            }]
        })
        .collect();
    let files: Vec<(&str, &[E2eTextChunkSpec<'_>])> = contents
        .iter()
        .zip(chunks.iter())
        .map(|((path, _), chunk)| (path.as_str(), chunk.as_slice()))
        .collect();
    Ok(rt.ingest_text_files_one_batch(&files)?)
}

fn counter(snapshot: &MetricsSnapshotV1, name: &str) -> Result<u64, Box<dyn Error>> {
    snapshot
        .counters
        .iter()
        .find(|counter| counter.name == name)
        .map(|counter| counter.value)
        .ok_or_else(|| format!("counter `{name}` is in the scrape").into())
}

/// A counter the query plane only emits once a sample exists; absent is
/// zero.
fn counter_or_zero(snapshot: &MetricsSnapshotV1, name: &str) -> u64 {
    snapshot
        .counters
        .iter()
        .find(|counter| counter.name == name)
        .map_or(0, |counter| counter.value)
}

fn served(result: &E2eQueryResult, what: &str) -> Result<(), Box<dyn Error>> {
    result.typed_error.as_ref().map_or_else(
        || Ok(()),
        |error| Err(format!("{what} was refused: {error}").into()),
    )
}

fn top_is(result: &E2eQueryResult, id: &str, what: &str) -> TestResult {
    let top = result.candidates.first();
    if top.map(|candidate| candidate.candidate_id.as_str()) != Some(id)
        || top.is_none_or(|candidate| (candidate.score - 1.0).abs() > 1e-5)
    {
        let ranked: Vec<(String, f32)> = result
            .candidates
            .iter()
            .map(|candidate| (candidate.candidate_id.clone(), candidate.score))
            .collect();
        return Err(
            format!("{what} must rank `{id}` first at cosine 1; top-{TOP_K}: {ranked:?}").into(),
        );
    }
    Ok(())
}

/// Arms the dense-lane hold for its lifetime and releases it on drop, so a
/// failed assertion cannot leave the process-wide hold armed.
struct DenseLaneHold;

impl DenseLaneHold {
    fn arm() -> Self {
        test_support::hold_dense_lane_until_interrupted(true);
        Self
    }
}

impl Drop for DenseLaneHold {
    fn drop(&mut self) {
        test_support::hold_dense_lane_until_interrupted(false);
    }
}

struct LaneTallies {
    exact_queries: u64,
    exact_interruptions: u64,
    ann_queries: u64,
    ann_interruptions: u64,
    interrupted_errors: u64,
}

fn lane_tallies(rt: &mut E2eRuntime) -> Result<LaneTallies, Box<dyn Error>> {
    let snapshot = rt.metrics_snapshot()?;
    Ok(LaneTallies {
        exact_queries: counter(&snapshot, "semantic_dense_queries_exact_total")?,
        exact_interruptions: counter(&snapshot, "semantic_budget_interruptions_exact_total")?,
        ann_queries: counter(&snapshot, "semantic_dense_queries_ann_total")?,
        ann_interruptions: counter(&snapshot, "semantic_budget_interruptions_ann_total")?,
        interrupted_errors: counter_or_zero(&snapshot, "lq_typed_error_deadline_exceeded_total"),
    })
}

#[test]
fn a_deadline_inside_the_dense_lane_is_answered_from_the_lane_counted_and_survived() -> TestResult {
    let policy = ServerAdmissionPolicy::new(
        ServerAdmissionPolicy::DEFAULT.max_connections(),
        ServerAdmissionPolicy::DEFAULT.dispatch_slots(),
        ServerAdmissionPolicy::DEFAULT.max_in_flight_per_repo(),
        ServerAdmissionPolicy::DEFAULT.queue_wait(),
        DISPATCH_BUDGET,
        ServerAdmissionPolicy::DEFAULT.io_timeout(),
    )?;
    let mut rt = E2eRuntime::boot_with_query_admission_policy(policy)?;
    let ids = ingest_all_files_one_batch(&mut rt)?;
    if ids.len() != usize::try_from(FILES)? {
        return Err(format!("ingest returned {} ids for {FILES} files", ids.len()).into());
    }
    let [warm_id, held_id, ..] = ids.as_slice() else {
        return Err(format!("ingest returned fewer than two ids: {ids:?}").into());
    };
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    // Served once before anything is held: the route, the opened generation
    // and the exact lane are all proven live, and the scrape's baseline is
    // one issued query and no interruption.
    let warm = rt.query_semantic(&file_content(0), TOP_K, None);
    served(&warm, "the warm-up semantic query")?;
    top_is(&warm, warm_id, "the warm-up semantic query")?;
    let before = lane_tallies(&mut rt)?;
    if before.exact_queries != 1 || before.exact_interruptions != 0 {
        return Err(format!(
            "one served exact-lane query is one issued query and no interruption, the scrape says queries={} interruptions={}",
            before.exact_queries, before.exact_interruptions
        )
        .into());
    }
    if before.ann_queries != 0 || before.ann_interruptions != 0 {
        return Err("a corpus below the index floor never touches the approximate lane".into());
    }

    // Held: the lane parks before issuing its query, so the dispatch budget
    // is the only way this request ends, and the lane's watcher is the only
    // place the deadline can be observed.
    let held = {
        let _hold = DenseLaneHold::arm();
        rt.query_semantic(&file_content(1), TOP_K, None)
    };
    let Some(error) = held.typed_error.as_ref() else {
        return Err(format!(
            "a query held inside the dense lane past its budget must be refused, it served {:?}",
            held.candidate_ids
        )
        .into());
    };
    if error.code != REQUEST_DEADLINE_EXCEEDED_CODE
        || !error.message.contains("checkpoint `semantic:exact`")
    {
        return Err(format!(
            "the deadline must be answered from the exact lane's checkpoint: {error}"
        )
        .into());
    }

    // The scrape: one interruption in the exact lane, no query issued for
    // the held request, and the route counted the error as interrupted.
    let after = lane_tallies(&mut rt)?;
    if after.exact_queries != before.exact_queries {
        return Err(format!(
            "the held request must never reach the library: issued queries went {} -> {}",
            before.exact_queries, after.exact_queries
        )
        .into());
    }
    if after.exact_interruptions != 1 || after.ann_interruptions != 0 {
        return Err(format!(
            "exactly one interruption, in the exact lane: exact={} ann={}",
            after.exact_interruptions, after.ann_interruptions
        )
        .into());
    }
    if after.interrupted_errors != before.interrupted_errors.saturating_add(1) {
        return Err(format!(
            "the route counts the interruption as `interrupted`: {} -> {}",
            before.interrupted_errors, after.interrupted_errors
        )
        .into());
    }

    // Released: the same daemon and the same opened generation serve the
    // query that was refused, and the tallies move by that one query.
    let again = rt.query_semantic(&file_content(1), TOP_K, None);
    served(&again, "the semantic query after the hold")?;
    top_is(&again, held_id, "the semantic query after the hold")?;
    let last = lane_tallies(&mut rt)?;
    if last.exact_queries != after.exact_queries.saturating_add(1) || last.exact_interruptions != 1
    {
        return Err(format!(
            "the served query is one more issued query and no new interruption: queries {} -> {}, interruptions {}",
            after.exact_queries, last.exact_queries, last.exact_interruptions
        )
        .into());
    }
    Ok(())
}
