//! W5 phase 3 — the request budget is observed inside the dense lane, not
//! only at the dispatcher's checkpoints around it.
//!
//! Two sealed generations in one adapter, one below the approximate-index
//! floor (an exact lane) and one at it (an approximate lane). Every case
//! hands a searcher a budget that is already interrupted, so the only place
//! the interruption can be observed is inside the lane, and the typed answer
//! must name that lane. The oracles are independent of the lane's own
//! claim: the adapter's scrape counts the queries each lane handed to the
//! library — zero for a refused request — and the interruptions it
//! observed, per lane; the same searcher then serves under a live budget,
//! ranking the query's own row first, so the refusal is the budget and not
//! the query or a poisoned handle.
//!
//! One case parks the lane through the adapter's debug hold and cancels the
//! budget from the outside: the shape of a peer leaving while the library
//! is busy, made deterministic — the lane cannot finish on its own, so
//! whenever the cancellation lands the lane is what answers it, and the
//! query is never issued.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use quanta_index_contract::{
    EmbeddingRecord, LexicalCandidate, ManifestGeneration, QueryConstraintSetV1, RepoId, RevisionId,
};
use quanta_index_core::{
    CoreError, DenseIndexV1, MetricSourcePort, MetricValueV1, REQUEST_CANCELLED_CODE,
    REQUEST_DEADLINE_EXCEEDED_CODE, RequestBudgetV1, SemanticIndexOpenPort, SemanticSearcher,
};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, legacy_chunk_embedding_record_v1,
    sealed_replace_batch_v1, test_support,
};

type TestResult = Result<(), Box<dyn Error>>;

const DIMENSION: usize = 16;
/// Rows of the generation that seals an exact lane: below the index floor.
const EXACT_ROWS: u64 = 64;
/// Rows of the generation that seals an approximate lane: the index floor.
const ANN_ROWS: u64 = 256;
const TOP_K: u32 = 3;

/// The dense-lane hold is process-wide; the tests in this binary run on
/// parallel threads, so the one that arms it and the ones that expect live
/// budgets to serve take turns.
static HOLD_TURN: Mutex<()> = Mutex::new(());

fn take_turn() -> MutexGuard<'static, ()> {
    match HOLD_TURN.lock() {
        Ok(turn) => turn,
        // A test that failed while holding the turn poisons nothing this
        // guard protects; the next test still takes its turn.
        Err(poisoned) => poisoned.into_inner(),
    }
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

fn repo() -> RepoId {
    RepoId::new("cancel-semantic-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("cancel-semantic-rev")
}

fn exact_generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn ann_generation() -> ManifestGeneration {
    ManifestGeneration::new(2)
}

/// A deterministic direction for `seed`, unit-normalized so the stored row
/// equals it up to float rounding and its own query ranks it first.
fn unit_vector(seed: u64) -> Vec<f32> {
    let mut state = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xD1B5_4A32_D192_ED03);
    let mut vector = Vec::with_capacity(DIMENSION);
    for _ in 0..DIMENSION {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let lane = u16::try_from(state & 0xFFFF).map_or(0.0_f32, f32::from);
        vector.push(lane / 32768.0_f32 - 1.0);
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    vector.iter().map(|value| value / norm).collect()
}

fn rows(path: &str, count: u64) -> Result<Vec<EmbeddingRecord>, Box<dyn Error>> {
    (0..count)
        .map(|seed| {
            legacy_chunk_embedding_record_v1(&format!("row-{seed}"), path, unit_vector(seed))
                .map_err(|err| -> Box<dyn Error> { err.into() })
        })
        .collect()
}

fn seal_rows(adapter: &SemanticAdapter, generation: ManifestGeneration, count: u64) -> TestResult {
    let path = format!("src/g{}.rs", generation.get());
    let batch = sealed_replace_batch_v1(
        repo(),
        revision(),
        generation,
        &path,
        rows(&path, count)?,
        u32::try_from(DIMENSION)?,
    );
    build_resident_batch_v1(adapter, &batch)?;
    Ok(())
}

/// An adapter with the exact-lane and approximate-lane generations sealed.
fn adapter_with_both_lanes() -> Result<(tempfile::TempDir, SemanticAdapter), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    seal_rows(&adapter, exact_generation(), EXACT_ROWS)?;
    seal_rows(&adapter, ann_generation(), ANN_ROWS)?;
    Ok((temp, adapter))
}

fn counter(adapter: &SemanticAdapter, name: &str) -> Result<u64, Box<dyn Error>> {
    let Some(point) = adapter
        .scrape()?
        .into_iter()
        .find(|point| point.name == name)
    else {
        return Err(format!("`{name}` is in the adapter's scrape").into());
    };
    match point.value {
        MetricValueV1::Counter(value) => Ok(value),
        MetricValueV1::Gauge(value) => {
            Err(format!("`{name}` is a counter, the scrape reports gauge {value}").into())
        }
    }
}

/// The two lanes' tallies as the scrape reports them.
#[derive(Debug, Eq, PartialEq)]
struct Tallies {
    exact_queries: u64,
    exact_interruptions: u64,
    ann_queries: u64,
    ann_interruptions: u64,
}

fn tallies(adapter: &SemanticAdapter) -> Result<Tallies, Box<dyn Error>> {
    Ok(Tallies {
        exact_queries: counter(adapter, "semantic_dense_queries_exact_total")?,
        exact_interruptions: counter(adapter, "semantic_budget_interruptions_exact_total")?,
        ann_queries: counter(adapter, "semantic_dense_queries_ann_total")?,
        ann_interruptions: counter(adapter, "semantic_budget_interruptions_ann_total")?,
    })
}

fn typed(
    result: &Result<Vec<LexicalCandidate>, CoreError>,
) -> Result<(String, String), Box<dyn Error>> {
    match result {
        Err(CoreError::Typed { code, message }) => Ok((code.clone(), message.clone())),
        Ok(hits) => Err(format!(
            "expected a typed interruption, the lane served {} hits",
            hits.len()
        )
        .into()),
        Err(other) => Err(format!("expected a typed interruption, got {other:?}").into()),
    }
}

fn expect_interruption(
    result: &Result<Vec<LexicalCandidate>, CoreError>,
    code: &str,
    checkpoint: &str,
    what: &str,
) -> TestResult {
    let (observed_code, message) = typed(result)?;
    if observed_code != code || !message.contains(&format!("checkpoint `{checkpoint}`")) {
        return Err(format!(
            "{what}: expected {code} at `{checkpoint}`, got {observed_code}: {message}"
        )
        .into());
    }
    Ok(())
}

fn expect_served(
    result: Result<Vec<LexicalCandidate>, CoreError>,
    seed: u64,
    what: &str,
) -> TestResult {
    let hits = result.map_err(|err| format!("{what}: refused: {err}"))?;
    let Some(top) = hits.first() else {
        return Err(format!("{what}: served no hits").into());
    };
    if top.candidate_id != format!("row-{seed}") || (top.score - 1.0).abs() > 1e-5 {
        return Err(format!(
            "{what}: the query's own row must rank first at cosine 1, got {top:?}"
        )
        .into());
    }
    Ok(())
}

fn expired_budget() -> Result<RequestBudgetV1, Box<dyn Error>> {
    Ok(RequestBudgetV1::until(
        Instant::now()
            .checked_sub(Duration::from_millis(5))
            .ok_or("the clock is more than five milliseconds old")?,
    ))
}

/// Which lane a searcher runs, as the checkpoint and metric names spell it.
fn lane_names(searcher: &dyn SemanticSearcher) -> (&'static str, &'static str) {
    match searcher.dense_lane().index {
        DenseIndexV1::Exact => ("semantic:exact", "exact"),
        DenseIndexV1::Approximate { .. } => ("semantic:ann", "ann"),
    }
}

/// An interrupted budget is refused before the query reaches the library,
/// in either lane, naming the lane.
///
/// The same searcher serves under a live budget, and each lane's tallies
/// move only for that lane.
#[test]
fn an_interrupted_budget_is_refused_inside_the_lane_before_the_query_is_issued() -> TestResult {
    let _turn = take_turn();
    let (_temp, adapter) = adapter_with_both_lanes()?;
    let exact = adapter.open(&repo(), &revision(), exact_generation())?;
    let ann = adapter.open(&repo(), &revision(), ann_generation())?;
    if lane_names(exact.as_ref()) != ("semantic:exact", "exact") {
        return Err(format!("{EXACT_ROWS} rows must seal an exact lane").into());
    }
    if lane_names(ann.as_ref()) != ("semantic:ann", "ann") {
        return Err(format!("{ANN_ROWS} rows must seal an approximate lane").into());
    }
    if tallies(&adapter)?
        != (Tallies {
            exact_queries: 0,
            exact_interruptions: 0,
            ann_queries: 0,
            ann_interruptions: 0,
        })
    {
        return Err("a fresh adapter has every tally at zero".into());
    }

    let query = unit_vector(7);
    let allowlist: BTreeSet<String> = ["row-7".to_string(), "row-8".to_string()]
        .into_iter()
        .collect();
    let constraints = QueryConstraintSetV1::unconstrained();
    for (searcher, checkpoint, infix) in [
        (exact.as_ref(), "semantic:exact", "exact"),
        (ann.as_ref(), "semantic:ann", "ann"),
    ] {
        let cancelled = RequestBudgetV1::unbounded();
        cancelled.cancel_handle().cancel();
        expect_interruption(
            &searcher.search(&query, TOP_K, &cancelled),
            REQUEST_CANCELLED_CODE,
            checkpoint,
            &format!("{infix}: cancelled global search"),
        )?;
        expect_interruption(
            &searcher.search_scoped_constrained(
                &query,
                &allowlist,
                &constraints,
                TOP_K,
                &cancelled,
            ),
            REQUEST_CANCELLED_CODE,
            checkpoint,
            &format!("{infix}: cancelled scoped search"),
        )?;
        expect_interruption(
            &searcher.search(&query, TOP_K, &expired_budget()?),
            REQUEST_DEADLINE_EXCEEDED_CODE,
            checkpoint,
            &format!("{infix}: expired global search"),
        )?;
        let queries = counter(&adapter, &format!("semantic_dense_queries_{infix}_total"))?;
        let interruptions = counter(
            &adapter,
            &format!("semantic_budget_interruptions_{infix}_total"),
        )?;
        if queries != 0 || interruptions != 3 {
            return Err(format!(
                "{infix}: three refused requests are three interruptions and no query handed to the library, the scrape says queries={queries} interruptions={interruptions}"
            )
            .into());
        }
        expect_served(
            searcher.search(&query, TOP_K, &RequestBudgetV1::unbounded()),
            7,
            &format!("{infix}: live global search"),
        )?;
        expect_served(
            searcher.search_scoped_constrained(
                &query,
                &allowlist,
                &constraints,
                TOP_K,
                &RequestBudgetV1::unbounded(),
            ),
            7,
            &format!("{infix}: live scoped search"),
        )?;
    }
    let observed = tallies(&adapter)?;
    let expected = Tallies {
        exact_queries: 2,
        exact_interruptions: 3,
        ann_queries: 2,
        ann_interruptions: 3,
    };
    if observed != expected {
        return Err(format!(
            "each lane counts its own work: expected {expected:?}, scraped {observed:?}"
        )
        .into());
    }
    Ok(())
}

/// A budget cancelled while the lane's query is pending drops the query
/// where it stands.
///
/// The lane answers the cancellation, the library never receives the
/// query, and the same searcher serves the next request.
#[test]
fn a_cancellation_while_the_query_is_pending_drops_it_and_the_handle_serves_again() -> TestResult {
    let _turn = take_turn();
    let (_temp, adapter) = adapter_with_both_lanes()?;
    let searcher = adapter.open(&repo(), &revision(), exact_generation())?;
    let query = unit_vector(3);

    let budget = RequestBudgetV1::unbounded();
    let cancel = budget.cancel_handle();
    let held = {
        let _hold = DenseLaneHold::arm();
        std::thread::scope(|scope| {
            let pending = scope.spawn(|| searcher.search(&query, TOP_K, &budget));
            // The lane parks before it can issue its query, so whether this
            // lands before the lane is reached or while it is parked, the
            // lane is what answers it and the query is never issued.
            cancel.cancel();
            pending.join()
        })
    };
    let held = held.map_err(|_panic| "the held search panicked")?;
    expect_interruption(
        &held,
        REQUEST_CANCELLED_CODE,
        "semantic:exact",
        "the held search",
    )?;
    let after_hold = tallies(&adapter)?;
    if after_hold.exact_queries != 0 || after_hold.exact_interruptions != 1 {
        return Err(format!(
            "a query dropped while pending never reached the library: {after_hold:?}"
        )
        .into());
    }

    expect_served(
        searcher.search(&query, TOP_K, &RequestBudgetV1::unbounded()),
        3,
        "the search after the dropped one",
    )?;
    let after_serve = tallies(&adapter)?;
    if after_serve.exact_queries != 1 || after_serve.exact_interruptions != 1 {
        return Err(format!("the served query is the first one issued: {after_serve:?}").into());
    }
    Ok(())
}
