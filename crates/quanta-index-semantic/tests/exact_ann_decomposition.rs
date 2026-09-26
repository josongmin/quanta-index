//! RBR-07 — the exact/ANN decomposition evidence harness.
//!
//! Pins, through the real public port surface (`SemanticAdapter::open` →
//! `SemanticSearcher` and the adapter's metric scrape), how the dense lane
//! decomposes into an exact exhaustive pass and an approximate indexed pass:
//!
//! - the 255/256 row floor (`VECTOR_INDEX_MIN_ROWS`): 255 rows seal an
//!   exact lane, 256 an approximate `ivf_hnsw_sq` one with the sealed
//!   effort, and the exact lane's served ranking equals an exhaustive
//!   cosine oracle computed in this file;
//! - the exact-completion contract of `run_vector_query`: an approximate
//!   pass that returns `top_k` rows is served as ranked, with no exact
//!   completion — observed through the
//!   `semantic_dense_{queries,exact_completions}_*` counters;
//! - exact-vs-approximate top-k agreement (recall of the exact top-k
//!   inside the approximate top-k) on a deterministic row set;
//! - the scoped-filter path (`search_scoped`) against an exhaustive oracle
//!   over the allowed subset.
//!
//! Every expected value comes from an in-test independent oracle: an
//! exhaustive f64 cosine ranking over the same deterministic unit vectors
//! the rows were sealed with (an xorshift stream, so no external assets
//! and no library output is ever reused as an expectation).
//!
//! Findings this harness had to pin (measured behavior, not the ticket's
//! assumptions) are marked `RBR-07 finding:` at each assertion site; see
//! the module-level summary in each test's doc comment.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_contract::{
    EmbeddingRecord, LexicalCandidate, ManifestGeneration, RepoId, RevisionId,
};
use quanta_index_core::{
    DenseIndexV1, MetricSourcePort, MetricValueV1, RequestBudgetV1, SemanticIndexOpenPort,
};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, legacy_chunk_embedding_record_v1,
    sealed_replace_batch_v1,
};

type TestResult = Result<(), Box<dyn Error>>;

/// Row-vector dimension: wide enough that random directions are well
/// separated, narrow enough that seals stay fast.
const DIMENSION: usize = 16;
/// One row below `VECTOR_INDEX_MIN_ROWS` (256): the lane must be exact.
const EXACT_ROWS: u64 = 255;
/// At `VECTOR_INDEX_MIN_ROWS`: the lane must be approximate.
const ANN_ROWS: u64 = 256;
/// Top-k every ranking comparison uses at the boundary scale.
const TOP_K: u32 = 10;
/// Queries per ranking measurement; enough that a systematic ranking
/// defect cannot hide behind one lucky direction.
const QUERIES: u64 = 40;
/// Query seeds start here so no query direction is any row's own vector.
const QUERY_SEED_BASE: u64 = 9_000_000;
/// The served score is `1 - cosine distance`; float error stays far below.
const SCORE_TOLERANCE: f64 = 1e-4;

fn repo() -> Result<RepoId, Box<dyn Error>> {
    RepoId::new("rbr-07-repo")
        .map_err(|err| -> Box<dyn Error> { format!("fixture repo ID rejected: {err}").into() })
}

fn revision() -> Result<RevisionId, Box<dyn Error>> {
    RepoId::new("rbr-07-rev")
        .map_err(|err| -> Box<dyn Error> { format!("fixture revision ID rejected: {err}").into() })
        .map(|_| RevisionId::new("rbr-07-rev"))
        .and_then(|value| {
            value.map_err(|err| -> Box<dyn Error> {
                format!("fixture revision ID rejected: {err}").into()
            })
        })
}

/// A deterministic unit direction for `seed` (xorshift stream mapped to
/// `[-1, 1)` and normalized), the same construction the sibling harnesses
/// use; no row and no query shares another's direction.
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
        vector.push(lane / 32_768.0_f32 - 1.0);
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    vector.iter().map(|value| value / norm).collect()
}

fn row_id(seed: u64) -> String {
    format!("row-{seed}")
}

/// Rows `row-0..row-{count}` (skipping the zeroth seed's parity), each
/// carrying `unit_vector(seed)`; the fixture normalizes to `L2Unit`, so
/// the stored vector is the unit direction up to float rounding.
fn rows(count: u64) -> Result<Vec<EmbeddingRecord>, Box<dyn Error>> {
    (0..count)
        .map(|seed| {
            legacy_chunk_embedding_record_v1(&row_id(seed), "src/lib.rs", unit_vector(seed))
                .map_err(|err| -> Box<dyn Error> { err.into() })
        })
        .collect()
}

fn seal_generation(
    adapter: &SemanticAdapter,
    generation: ManifestGeneration,
    count: u64,
) -> TestResult {
    let batch = sealed_replace_batch_v1(
        repo()?,
        revision()?,
        generation,
        "src/lib.rs",
        rows(count)?,
        u32::try_from(DIMENSION)?,
    );
    build_resident_batch_v1(adapter, &batch)?;
    Ok(())
}

fn open_generation(
    adapter: &SemanticAdapter,
    generation: ManifestGeneration,
) -> Result<Box<dyn quanta_index_core::domains::semantic::SemanticSearcher>, Box<dyn Error>> {
    Ok(adapter.open(&repo()?, &revision()?, generation)?)
}

/// The exhaustive cosine oracle: ranks `records` (restricted to `allowed`
/// when given) by true f64 cosine against `query`, nearest first, with the
/// embedding-id string as the stable tie-break, and keeps the first `k`.
///
/// This is computed independently of lancedb: raw dot products and norms
/// over the fixture vectors, no query-path code involved.
fn exhaustive_cosine_oracle(
    query: &[f32],
    records: &[EmbeddingRecord],
    allowed: Option<&BTreeSet<String>>,
    k: usize,
) -> Vec<(String, f64)> {
    let query_norm = query
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    let mut ranked: Vec<(String, f64)> = records
        .iter()
        .filter(|record| {
            allowed.is_none_or(|allowed| allowed.contains(record.embedding_id.as_str()))
        })
        .map(|record| {
            let dot_product = query
                .iter()
                .zip(&record.vector)
                .map(|(left, right)| f64::from(*left) * f64::from(*right))
                .sum::<f64>();
            let record_norm = record
                .vector
                .iter()
                .map(|value| f64::from(*value) * f64::from(*value))
                .sum::<f64>()
                .sqrt();
            (
                record.embedding_id.as_str().to_string(),
                dot_product / (query_norm * record_norm),
            )
        })
        .collect();
    ranked.sort_by(|(left_id, left_score), (right_id, right_score)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left_id.cmp(right_id))
    });
    ranked.truncate(k);
    ranked
}

fn oracle_ids(ranked: &[(String, f64)]) -> Vec<String> {
    ranked.iter().map(|(id, _)| id.clone()).collect()
}

fn hit_ids(hits: &[LexicalCandidate]) -> Vec<String> {
    hits.iter()
        .map(|hit| hit.candidate_id.as_str().to_string())
        .collect()
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

/// (exact-lane queries, ann-lane queries, exact completions) — the three
/// counters that make the decomposition observable from outside the crate.
fn lane_counters(adapter: &SemanticAdapter) -> Result<(u64, u64, u64), Box<dyn Error>> {
    Ok((
        counter(adapter, "semantic_dense_queries_exact_total")?,
        counter(adapter, "semantic_dense_queries_ann_total")?,
        counter(adapter, "semantic_dense_exact_completions_total")?,
    ))
}

/// Ticket item (a): the documented 255/256 row boundary.
///
/// 255 rows must seal an exact lane and 256 an approximate one, and the
/// served top-k ranking must equal the exhaustive cosine oracle in both
/// cases — through the actual query path (`SemanticSearcher::search`),
/// not through any test-only seam.
///
/// Measured behavior pinned here for the 256-row approximate lane: its
/// served top-10 equaled the exhaustive oracle's top-10 *as an ordered
/// list* on every measured query (the index re-ranks its candidates by
/// exact distance, `refine_factor = 2`), which is why ordered equality is
/// asserted rather than set overlap. A library bump that breaks it fails
/// this pin loudly.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test asserts measured invariants of the served ranking via assert macros; a violated invariant is a test failure, not a propagatable error"
)]
fn the_255_256_row_boundary_serves_the_exhaustive_oracle_through_both_lanes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let exact_generation = ManifestGeneration::new(1);
    let ann_generation = ManifestGeneration::new(2);
    seal_generation(&adapter, exact_generation, EXACT_ROWS)?;
    seal_generation(&adapter, ann_generation, ANN_ROWS)?;
    let exact_records = rows(EXACT_ROWS)?;
    let ann_records = rows(ANN_ROWS)?;
    let exact_searcher = open_generation(&adapter, exact_generation)?;
    let ann_searcher = open_generation(&adapter, ann_generation)?;

    // The boundary itself: one row below the floor serves an exact lane,
    // at the floor an approximate one with the sealed effort.
    assert!(matches!(
        exact_searcher.dense_lane().index,
        DenseIndexV1::Exact
    ));
    let DenseIndexV1::Approximate { effort, .. } = &ann_searcher.dense_lane().index else {
        panic!("256 rows must seal the approximate lane");
    };
    assert_eq!(effort.index_kind, "ivf_hnsw_sq");
    assert_eq!(effort.partitions, 1, "256 rows fill one partition");
    assert_eq!(
        effort.nprobes, 1,
        "one partition is probed: min(NPROBES, partitions)"
    );
    assert_eq!(effort.ef_floor, 64);
    assert_eq!(effort.ef_per_candidate, 2);
    assert_eq!(effort.refine_factor, 2);

    for query_seed in 0..QUERIES {
        let query = unit_vector(QUERY_SEED_BASE.saturating_add(query_seed));
        let expected_exact = exhaustive_cosine_oracle(&query, &exact_records, None, TOP_K as usize);
        let expected_ann = exhaustive_cosine_oracle(&query, &ann_records, None, TOP_K as usize);
        assert_eq!(expected_exact.len(), TOP_K as usize);
        assert_eq!(expected_ann.len(), TOP_K as usize);

        let exact_hits = exact_searcher.search(&query, TOP_K, &RequestBudgetV1::unbounded())?;
        assert_eq!(
            hit_ids(&exact_hits),
            oracle_ids(&expected_exact),
            "query {query_seed}: the exact lane must serve the exhaustive oracle's ordered top-k"
        );
        for (hit, (_id, expected_score)) in exact_hits.iter().zip(&expected_exact) {
            assert!(
                (f64::from(hit.score) - expected_score).abs() < SCORE_TOLERANCE,
                "query {query_seed}: exact-lane score {} vs oracle cosine {expected_score}",
                hit.score
            );
        }

        let ann_hits = ann_searcher.search(&query, TOP_K, &RequestBudgetV1::unbounded())?;
        assert_eq!(
            hit_ids(&ann_hits),
            oracle_ids(&expected_ann),
            "query {query_seed}: the approximate lane must serve the exhaustive oracle's ordered top-k"
        );
        for (hit, (_id, expected_score)) in ann_hits.iter().zip(&expected_ann) {
            assert!(
                (f64::from(hit.score) - expected_score).abs() < SCORE_TOLERANCE,
                "query {query_seed}: approximate-lane score {} vs oracle cosine {expected_score}",
                hit.score
            );
        }
    }
    Ok(())
}

/// Ticket item (b): the exact-completion contract for a query whose
/// approximate pass returns exactly `top_k` results.
///
/// The code contract (`run_vector_query`): a pass that returns at least
/// `top_k` rows is served as ranked — the exact lane is not consulted and
/// the `semantic_dense_exact_completions_total` counter must not move,
/// however the pass ranked its rows. A short pass (fewer than
/// `min(top_k, rows in scope)` rows) would be re-run through the exact
/// lane and counted as a completion.
///
/// RBR-07 finding: no short-result exact completion could be produced
/// through the public query surface at any measured shape. The sealed
/// effort pins `ef = max(64, 4 * top_k)`, so at a page query the beam
/// width exceeds the row count and the walk reaches every row: a page at
/// `top_k = 10_000` over 256 rows comes back all 256 rows long, and the
/// completion rail stays at zero (asserted below via the counters). The
/// incident shape itself — `top_k = 10_000` over 10_001 rows — is pinned
/// separately in `a_page_query_at_the_incident_scale_is_not_short`.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test asserts counter invariants via assert macros; a violated invariant is a test failure, not a propagatable error"
)]
fn a_full_length_approximate_pass_is_served_as_ranked_without_exact_completion() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    seal_generation(&adapter, generation, ANN_ROWS)?;
    let records = rows(ANN_ROWS)?;
    let searcher = open_generation(&adapter, generation)?;
    assert!(matches!(
        searcher.dense_lane().index,
        DenseIndexV1::Approximate { .. }
    ));
    let query = unit_vector(QUERY_SEED_BASE);

    // A pass that returns exactly `top_k` rows is served as ranked: one
    // approximate-lane query, no exact-lane query, no completion.
    let before_full = lane_counters(&adapter)?;
    let full = searcher.search(&query, TOP_K, &RequestBudgetV1::unbounded())?;
    let after_full = lane_counters(&adapter)?;
    assert_eq!(full.len(), usize::try_from(TOP_K)?);
    assert_eq!(after_full.0, before_full.0, "no exact-lane query may run");
    assert_eq!(
        after_full.1,
        before_full.1.saturating_add(1),
        "exactly one approximate-lane query runs"
    );
    assert_eq!(
        after_full.2, before_full.2,
        "a full pass must not be completed exactly"
    );
    // ... and what it served is the oracle's ordered top-k (measured; see
    // the recall test for the aggregate statement).
    let expected = exhaustive_cosine_oracle(&query, &records, None, usize::try_from(TOP_K)?);
    assert_eq!(hit_ids(&full), oracle_ids(&expected));

    // A page larger than the table: the approximate pass reaches every
    // row, so the pass is full-length in the scope sense and again no
    // completion may run.
    let before_page = lane_counters(&adapter)?;
    let page = searcher.search(&query, 10_000, &RequestBudgetV1::unbounded())?;
    let after_page = lane_counters(&adapter)?;
    assert_eq!(page.len(), usize::try_from(ANN_ROWS)?);
    assert_eq!(after_page.0, before_page.0);
    assert_eq!(after_page.1, before_page.1.saturating_add(1));
    assert_eq!(
        after_page.2, before_page.2,
        "a page that reached every row must not be completed exactly"
    );
    Ok(())
}

/// Ticket item (c): exact-vs-approximate top-k agreement.
///
/// Recall of the exhaustive oracle's top-k inside the approximate lane's
/// served top-k, over `QUERIES` deterministic query directions. Measured
/// 400/400 on this row set (the index's refine step re-ranks candidates
/// by exact distance and the graph walk at this scale reaches the true
/// neighbours), so recall == 1.0 is asserted as the pinned value; the
/// line the run log carries is printed for the ledger.
#[test]
#[expect(
    clippy::print_stdout,
    reason = "the RBR-07-RECALL line is the measurement record the ticket cites; it must land in the run log"
)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test asserts the pinned recall via assert macros; a violated pin is a test failure, not a propagatable error"
)]
fn approximate_top_k_recall_against_the_exhaustive_oracle_is_pinned_at_the_measured_value()
-> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    seal_generation(&adapter, generation, ANN_ROWS)?;
    let records = rows(ANN_ROWS)?;
    let searcher = open_generation(&adapter, generation)?;
    assert!(matches!(
        searcher.dense_lane().index,
        DenseIndexV1::Approximate { .. }
    ));

    // Fresh seeds, disjoint from the boundary test's, so the two tests do
    // not merely re-measure the same directions.
    let recall_seed_base = QUERY_SEED_BASE.saturating_add(100_000);
    let mut found = 0_u64;
    for query_seed in 0..QUERIES {
        let query = unit_vector(recall_seed_base.saturating_add(query_seed));
        let expected: BTreeSet<String> = oracle_ids(&exhaustive_cosine_oracle(
            &query,
            &records,
            None,
            TOP_K as usize,
        ))
        .into_iter()
        .collect();
        let hits = searcher.search(&query, TOP_K, &RequestBudgetV1::unbounded())?;
        assert_eq!(
            usize::try_from(hits.len())?,
            usize::try_from(TOP_K)?,
            "query {query_seed}: a full-length pass must come back"
        );
        found = found.saturating_add(u64::try_from(
            hit_ids(&hits)
                .iter()
                .filter(|id| expected.contains(*id))
                .count(),
        )?);
    }
    let total = QUERIES.saturating_mul(u64::from(TOP_K));
    let recall = f64::from(u32::try_from(found)?) / f64::from(u32::try_from(total)?);
    println!(
        "RBR-07-RECALL rows={ANN_ROWS} dim={DIMENSION} queries={QUERIES} k={TOP_K} recall_at_k={recall:.4}"
    );
    // RBR-07 finding: measured recall at this scale is exactly 1.0 (every
    // exact top-k member served by the approximate lane), so equality is
    // the pinned value — not an assumption borrowed from the ticket.
    assert_eq!(found, total, "pinned recall@{TOP_K} is 1.0 at this scale");
    Ok(())
}

/// Ticket item (d): the scoped-filter path.
///
/// `search_scoped` restricts the query to an allowed subset through the
/// storage-level id filter; the served ranking must equal an exhaustive
/// oracle computed over that subset alone, on both lanes.
///
/// RBR-07 finding: at every measured selectivity (40, 200 and 5 allowed
/// ids of 256 rows) the filtered approximate pass came back full-length
/// and matched the subset oracle exactly, so no exact completion ran
/// (counters asserted). The scoped path was never observed to return a
/// short result under the sealed effort.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test asserts scoped-ranking invariants via assert macros; a violated invariant is a test failure, not a propagatable error"
)]
fn scoped_search_matches_the_exhaustive_oracle_over_the_allowed_subset() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let exact_generation = ManifestGeneration::new(1);
    let ann_generation = ManifestGeneration::new(2);
    seal_generation(&adapter, exact_generation, EXACT_ROWS)?;
    seal_generation(&adapter, ann_generation, ANN_ROWS)?;
    let exact_records = rows(EXACT_ROWS)?;
    let ann_records = rows(ANN_ROWS)?;
    let exact_searcher = open_generation(&adapter, exact_generation)?;
    let ann_searcher = open_generation(&adapter, ann_generation)?;
    let query = unit_vector(QUERY_SEED_BASE.saturating_add(1));

    // An empty scope admits nothing and must not query any lane at all.
    let before_empty = lane_counters(&adapter)?;
    let empty = ann_searcher.search_scoped(
        &query,
        &BTreeSet::new(),
        TOP_K,
        &RequestBudgetV1::unbounded(),
    )?;
    assert!(empty.is_empty());
    assert_eq!(lane_counters(&adapter)?, before_empty);

    // A 40-row subset (the even seeds of 0..80) of both generations.
    let allowed: BTreeSet<String> = (0..80).step_by(2).map(row_id).collect();
    assert_eq!(allowed.len(), 40);
    let scope_top_k = 40_u32;
    for (label, searcher, records) in [
        ("exact", exact_searcher.as_ref(), &exact_records),
        ("approximate", ann_searcher.as_ref(), &ann_records),
    ] {
        let before = lane_counters(&adapter)?;
        let scoped =
            searcher.search_scoped(&query, &allowed, scope_top_k, &RequestBudgetV1::unbounded())?;
        let after = lane_counters(&adapter)?;
        let expected =
            exhaustive_cosine_oracle(&query, records, Some(&allowed), scope_top_k as usize);
        assert_eq!(
            hit_ids(&scoped),
            oracle_ids(&expected),
            "{label} lane: scoped search must serve the subset oracle's ordered ranking"
        );
        assert_eq!(scoped.len(), expected.len());
        for (hit, (_id, expected_score)) in scoped.iter().zip(&expected) {
            assert!(
                (f64::from(hit.score) - expected_score).abs() < SCORE_TOLERANCE,
                "{label} lane: scoped score {} vs oracle cosine {expected_score}",
                hit.score
            );
        }
        // No exact completion: the filtered pass was full-length (finding
        // above). Exactly one query ran, on the generation's own lane.
        assert_eq!(
            after.2, before.2,
            "{label} lane: no exact completion may run"
        );
        match label {
            "exact" => {
                assert_eq!(after.0, before.0.saturating_add(1));
                assert_eq!(after.1, before.1);
            }
            _ => {
                assert_eq!(after.0, before.0);
                assert_eq!(after.1, before.1.saturating_add(1));
            }
        }
    }

    // A tiny, highly selective scope on the approximate lane.
    let tiny: BTreeSet<String> = (0..5).map(row_id).collect();
    let before_tiny = lane_counters(&adapter)?;
    let scoped_tiny =
        ann_searcher.search_scoped(&query, &tiny, 5, &RequestBudgetV1::unbounded())?;
    let after_tiny = lane_counters(&adapter)?;
    let expected_tiny = exhaustive_cosine_oracle(&query, &ann_records, Some(&tiny), 5);
    assert_eq!(hit_ids(&scoped_tiny), oracle_ids(&expected_tiny));
    assert_eq!(after_tiny.2, before_tiny.2);
    Ok(())
}

/// Ticket item (b), the incident shape: a page at `top_k = 10_000` over
/// 10_001 rows — the QI-BB-025 short-pass scenario the exact-completion
/// rail was built for (a pass then came back 9_695 rows long and was read
/// as the whole scope).
///
/// RBR-07 finding: under the sealed effort the short pass no longer
/// reproduces. `ef = max(64, 4 * top_k)` = 80_000 exceeds the 10_001-row
/// graph, the walk reaches every row, and the pass comes back the full
/// 10_000 rows — the completion counter stays at zero and the exact lane
/// is never consulted (counters asserted). The served page is set-equal
/// to the exhaustive oracle's top-10_000 (all 10_000 ids overlap) but is
/// *not* ordered exactly like it — the pass's own ordering survives in
/// the deep tail, which is precisely why the rail exists and why its
/// non-activation is pinned here rather than assumed.
///
/// This seal trains the graph over 10_001 rows and is the slowest test in
/// this file (minutes in the debug lane profile); it is the only scale at
/// which the incident could reproduce, so it carries its cost.
#[test]
#[expect(
    clippy::print_stdout,
    reason = "the RBR-07-PAGE line records the measured page shape (length, ordering divergence) the finding cites"
)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test asserts page-shape invariants via assert macros; a violated invariant is a test failure, not a propagatable error"
)]
fn a_page_query_at_the_incident_scale_is_not_short_under_the_sealed_effort() -> TestResult {
    const INCIDENT_ROWS: u64 = 10_001;
    const INCIDENT_TOP_K: u32 = 10_000;
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    seal_generation(&adapter, generation, INCIDENT_ROWS)?;
    let records = rows(INCIDENT_ROWS)?;
    let searcher = open_generation(&adapter, generation)?;
    let DenseIndexV1::Approximate { effort, .. } = &searcher.dense_lane().index else {
        panic!("{INCIDENT_ROWS} rows must seal the approximate lane");
    };
    assert_eq!(effort.partitions, 1);

    let query = unit_vector(QUERY_SEED_BASE);
    let before = lane_counters(&adapter)?;
    let page = searcher.search(&query, INCIDENT_TOP_K, &RequestBudgetV1::unbounded())?;
    let after = lane_counters(&adapter)?;
    assert_eq!(
        page.len(),
        usize::try_from(INCIDENT_TOP_K)?,
        "the page must come back full-length under the sealed effort"
    );
    assert_eq!(after.0, before.0, "the exact lane must not be consulted");
    assert_eq!(after.1, before.1.saturating_add(1));
    assert_eq!(
        after.2, before.2,
        "a full-length page must not be completed exactly"
    );

    // The served page against the exhaustive oracle over all 10_001 rows.
    let expected = exhaustive_cosine_oracle(&query, &records, None, INCIDENT_TOP_K as usize);
    assert_eq!(expected.len(), usize::try_from(INCIDENT_TOP_K)?);
    let expected_set: BTreeSet<&str> = expected.iter().map(|(id, _)| id.as_str()).collect();
    let served = hit_ids(&page);
    let overlap = served
        .iter()
        .filter(|id| expected_set.contains(id.as_str()))
        .count();
    let reordered_positions = served
        .iter()
        .zip(expected.iter())
        .filter(|(served_id, (expected_id, _))| served_id.as_str() != expected_id.as_str())
        .count();
    println!(
        "RBR-07-PAGE rows={INCIDENT_ROWS} top_k={INCIDENT_TOP_K} served={} overlap_with_exact_top_k={overlap} positions_differing_from_exact_order={reordered_positions}",
        served.len()
    );
    assert_eq!(
        overlap,
        usize::try_from(INCIDENT_TOP_K)?,
        "the served page must be set-equal to the exact top-k"
    );
    // RBR-07 finding: measured 3_687 differing positions — set-equal, not
    // order-equal; the exact-completion rail that would re-order such a
    // page never activates because the pass is never short.
    assert!(
        reordered_positions > 0,
        "the page's order must be the pass's own order, not the exact one"
    );
    Ok(())
}
