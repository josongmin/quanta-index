//! QI-BB-022 — `score_candidate` is an exact, per-row cosine lookup.
//!
//! Two sealed generations in one adapter, one below the approximate-index
//! floor (an exact lane) and one at it (an approximate lane). The oracle
//! for every score is a dot product computed in this file over the same
//! unit vectors the rows were sealed with; the oracle for the lane is the
//! adapter's scrape, which counts the queries each lane handed to the
//! library: a lookup on the approximate generation must be counted by the
//! exact lane, never the approximate one, so a hybrid explain reconciles a
//! carried dense score against the stored vector and not against a
//! neighbour search's recall.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{EmbeddingRecord, ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    CoreError, DenseIndexV1, MetricSourcePort, MetricValueV1, REQUEST_DEADLINE_EXCEEDED_CODE,
    RequestBudgetV1, SemanticIndexOpenPort,
};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, legacy_chunk_embedding_record_v1,
    sealed_replace_batch_v1,
};

type TestResult = Result<(), Box<dyn Error>>;

const DIMENSION: usize = 16;
/// Below the approximate-index floor: an exact lane.
const EXACT_ROWS: u64 = 64;
/// At the floor: an approximate lane.
const ANN_ROWS: u64 = 256;
/// The library's refine step returns the stored vector's own cosine, so
/// the lookup and the oracle agree to float rounding.
const SCORE_TOLERANCE: f32 = 1e-5;

fn repo() -> RepoId {
    RepoId::new("score-candidate-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("score-candidate-rev")
}

fn exact_generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn ann_generation() -> ManifestGeneration {
    ManifestGeneration::new(2)
}

/// A deterministic direction for `seed`, unit-normalized so the stored row
/// equals it up to float rounding and its own query scores it at cosine 1.
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

fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right.iter()).map(|(a, b)| a * b).sum()
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

/// How many queries each lane handed to the library so far.
fn lane_queries(adapter: &SemanticAdapter) -> Result<(u64, u64), Box<dyn Error>> {
    Ok((
        counter(adapter, "semantic_dense_queries_exact_total")?,
        counter(adapter, "semantic_dense_queries_ann_total")?,
    ))
}

/// A stored row scores exactly its own cosine against any query, on both
/// lanes, and the lookup is counted by the exact lane even on the
/// approximate generation.
#[test]
fn a_stored_row_scores_its_exact_cosine_through_the_exact_lane_on_both_generations() -> TestResult {
    let (_temp, adapter) = adapter_with_both_lanes()?;
    for (generation, row_count, expected_index) in [
        (exact_generation(), EXACT_ROWS, "exact"),
        (ann_generation(), ANN_ROWS, "approximate"),
    ] {
        let searcher = adapter.open(&repo(), &revision(), generation)?;
        let lane_is_approximate = matches!(
            searcher.dense_lane().index,
            DenseIndexV1::Approximate { .. }
        );
        if lane_is_approximate != (expected_index == "approximate") {
            return Err(format!(
                "generation {} must serve through an {expected_index} lane: {:?}",
                generation.get(),
                searcher.dense_lane()
            )
            .into());
        }
        let before = lane_queries(&adapter)?;
        // A fresh query direction, unrelated to any row: the oracle is the
        // dot product with the row's own unit vector.
        let query = unit_vector(7_000_000 + row_count);
        for seed in [0, row_count.div_euclid(2), row_count - 1] {
            let candidate_id = format!("row-{seed}");
            let scored =
                searcher.score_candidate(&candidate_id, &query, &RequestBudgetV1::unbounded())?;
            let Some(score) = scored else {
                return Err(format!(
                    "generation {}: {candidate_id} is stored but scored None",
                    generation.get()
                )
                .into());
            };
            let expected = dot(&unit_vector(seed), &query);
            if (score - expected).abs() > SCORE_TOLERANCE {
                return Err(format!(
                    "generation {}: {candidate_id} scored {score} but its stored vector's cosine is {expected}",
                    generation.get()
                )
                .into());
            }
            // The row's own direction scores cosine 1.
            let own = searcher.score_candidate(
                &candidate_id,
                &unit_vector(seed),
                &RequestBudgetV1::unbounded(),
            )?;
            if own.is_none_or(|own| (own - 1.0).abs() > SCORE_TOLERANCE) {
                return Err(format!(
                    "generation {}: {candidate_id} against its own vector scored {own:?}, not 1",
                    generation.get()
                )
                .into());
            }
        }
        let after = lane_queries(&adapter)?;
        // Three rows, two lookups each: six exact-lane queries, and the
        // approximate lane untouched even where the generation has one.
        if after.0 != before.0.saturating_add(6) || after.1 != before.1 {
            return Err(format!(
                "generation {}: lookups must run through the exact lane: before={before:?} after={after:?}",
                generation.get()
            )
            .into());
        }
    }
    Ok(())
}

/// A row the generation does not store is `None`, not a neighbour.
///
/// The lookup is by id, so a query near a stored row still answers `None`
/// for an id that is not there, and the id is never a probe for anything
/// else.
#[test]
fn an_id_the_generation_does_not_store_scores_none_even_when_a_neighbour_matches() -> TestResult {
    let (_temp, adapter) = adapter_with_both_lanes()?;
    let searcher = adapter.open(&repo(), &revision(), ann_generation())?;
    // The query is exactly `row-3`'s vector; `row-3` is stored, the ghost
    // is not, and the other generation's rows are not this generation's.
    let query = unit_vector(3);
    let stored = searcher.score_candidate("row-3", &query, &RequestBudgetV1::unbounded())?;
    if stored.is_none_or(|score| (score - 1.0).abs() > SCORE_TOLERANCE) {
        return Err(format!("row-3 against its own vector scored {stored:?}").into());
    }
    for ghost in ["row-99999", "never-ingested", "row-3 "] {
        let answer = searcher.score_candidate(ghost, &query, &RequestBudgetV1::unbounded())?;
        if answer.is_some() {
            return Err(format!("{ghost:?} is not stored but scored {answer:?}").into());
        }
    }
    Ok(())
}

/// The lookup observes the request budget like every dense read: an
/// expired budget is refused typed, inside the exact lane, and no query
/// reaches the library.
#[test]
fn an_expired_budget_refuses_the_lookup_before_the_query_is_issued() -> TestResult {
    let (_temp, adapter) = adapter_with_both_lanes()?;
    let searcher = adapter.open(&repo(), &revision(), ann_generation())?;
    let before = lane_queries(&adapter)?;
    let expired = RequestBudgetV1::until(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(5))
            .ok_or("the clock is more than five milliseconds old")?,
    );
    let refused = searcher.score_candidate("row-1", &unit_vector(1), &expired);
    match refused {
        Err(CoreError::Typed { code, message })
            if code == REQUEST_DEADLINE_EXCEEDED_CODE && message.contains("semantic:exact") =>
        {
            let after = lane_queries(&adapter)?;
            if after != before {
                return Err(format!(
                    "a refused lookup must not reach the library: before={before:?} after={after:?}"
                )
                .into());
            }
            Ok(())
        }
        other => Err(
            format!("expected a typed deadline refusal in the exact lane, got {other:?}").into(),
        ),
    }
}

/// A query of the wrong dimension is refused typed, as every dense read
/// refuses it, rather than answering `None` for a stored row.
#[test]
fn a_query_of_the_wrong_dimension_is_refused_not_answered_none() -> TestResult {
    let (_temp, adapter) = adapter_with_both_lanes()?;
    let searcher = adapter.open(&repo(), &revision(), exact_generation())?;
    let narrow: Vec<f32> = unit_vector(1).into_iter().take(DIMENSION - 1).collect();
    match searcher.score_candidate("row-1", &narrow, &RequestBudgetV1::unbounded()) {
        Err(CoreError::Typed { code, .. }) if code == "SEM_DIM_MISMATCH" => Ok(()),
        other => Err(format!("expected SEM_DIM_MISMATCH, got {other:?}").into()),
    }
}
