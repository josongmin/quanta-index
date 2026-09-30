//! QI-BB-021 follow-up #2 — the semantic track streams a batch through
//! bounded windows, and the sealed generation serves every row after a
//! restart.
//!
//! Through the daemon's own front door: the daemon runs under a stream
//! window narrowed to eight rows of vectors, and one ingest batch carries
//! thirty-two files of one chunk each, so the semantic build takes the batch
//! as four windows instead of one. The scrape must count those windows and
//! report a resident peak no wider than the window bound; after a seal, an
//! activation and an in-process daemon runtime restart, every one of the thirty-two rows must
//! come back as the first candidate for its own text.
//!
//! Oracles are outside the accounting under test: the expected window count
//! and byte bound are computed here from the fixture's row count and the
//! daemon's embedding dimension, and the row coverage comes from queries.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::MetricsSnapshotV1;
use quanta_index_core::{SEMANTIC_STREAM_WINDOW_SCOPES, SemanticStreamWindowPolicy, count_as_f64};
use quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION;
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eQueryResult, E2eRuntime, E2eTextChunkSpec};

type TestResult = Result<(), Box<dyn Error>>;

/// Files in the one ingest batch: one chunk each, so one owner scope each.
const FILES: u32 = 32;
/// Rows of vectors one window may hold resident.
const ROWS_PER_WINDOW: u32 = 8;
/// Windows the one batch must stream through.
const EXPECTED_WINDOWS: u32 = FILES.div_ceil(ROWS_PER_WINDOW);
const _: () = assert!(
    EXPECTED_WINDOWS >= 3,
    "the fixture streams through at least three windows"
);
const TOP_K: u32 = 3;

/// One file's text: five tokens no other file shares, so the daemon's
/// token-hashing embedder places every file in its own direction and the
/// file's own text is unambiguously its nearest neighbour.
fn file_content(index: u32) -> String {
    format!("fn sw{index}a() {{ let sw{index}b = sw{index}c(sw{index}d, sw{index}e); }}")
}

fn file_path(index: u32) -> String {
    format!("src/stream/item_{index:03}.rs")
}

/// Ingest every file in ONE batch and return their candidate ids in ingest
/// order.
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

fn gauge(snapshot: &MetricsSnapshotV1, name: &str) -> Result<f64, Box<dyn Error>> {
    snapshot
        .gauges
        .iter()
        .find(|gauge| gauge.name == name)
        .map(|gauge| gauge.value)
        .ok_or_else(|| format!("gauge `{name}` is in the scrape").into())
}

fn served(result: &E2eQueryResult, what: &str) -> Result<(), Box<dyn Error>> {
    result.typed_error.as_ref().map_or_else(
        || Ok(()),
        |error| Err(format!("{what} was refused: {error}").into()),
    )
}

#[test]
#[expect(
    clippy::print_stdout,
    reason = "the QI-BB-021-STREAM-EVIDENCE line is the measurement the ledger cites; it must land in the run log"
)]
fn a_batch_streams_through_bounded_windows_and_every_row_survives_a_restart() -> TestResult {
    let bound = SemanticStreamWindowPolicy::vector_bytes(
        usize::try_from(ROWS_PER_WINDOW)?,
        SEARCH_OWNED_SEMANTIC_DIMENSION,
    )?;
    let policy = SemanticStreamWindowPolicy::new(SEMANTIC_STREAM_WINDOW_SCOPES, bound)?;
    let expected_windows = u64::from(EXPECTED_WINDOWS);
    let mut rt = E2eRuntime::boot_with_semantic_stream_window_policy(policy)?;

    let ids = ingest_all_files_one_batch(&mut rt)?;
    if ids.len() != usize::try_from(FILES)? {
        return Err(format!("ingest returned {} ids for {FILES} files", ids.len()).into());
    }
    let generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;

    // The scrape counts the windows the one batch streamed through and the
    // widest window it held; the seal batch carries no rows and adds none.
    let snapshot = rt.metrics_snapshot()?;
    let windows = counter(&snapshot, "semantic_ingest_windows_total")?;
    let peak = gauge(&snapshot, "semantic_ingest_resident_vector_bytes_peak")?;
    println!(
        "QI-BB-021-STREAM-EVIDENCE scopes={FILES} windows={windows} peak_resident_vector_bytes={peak} bound={bound} generation={}",
        generation.get()
    );
    if windows != expected_windows {
        return Err(format!(
            "{FILES} one-row owner scopes under an {ROWS_PER_WINDOW}-row window are {expected_windows} windows, the scrape counted {windows}"
        )
        .into());
    }
    // Both sides are integers below 2^53, each with one exact `f64`.
    if peak.to_bits() != count_as_f64(bound).to_bits() {
        return Err(format!(
            "a full window holds exactly the bound resident: peak={peak} bound={bound}"
        )
        .into());
    }

    // A runtime restart in this process: the sealed generation is reopened from durable state, and
    // every row streamed in is served as its own nearest neighbour.
    let mut rt = rt.reopen();
    for (index, id) in ids.iter().enumerate() {
        let index = u32::try_from(index)?;
        let query = rt.query_semantic(&file_content(index), TOP_K, None);
        served(&query, &format!("semantic query for file {index}"))?;
        let top = query.candidates.first();
        if top.map(|candidate| candidate.candidate_id.as_str()) != Some(id.as_str())
            || top.is_none_or(|candidate| (candidate.score - 1.0).abs() > 1e-5)
        {
            let ranked: Vec<(String, f32)> = query
                .candidates
                .iter()
                .map(|candidate| (candidate.candidate_id.clone(), candidate.score))
                .collect();
            return Err(format!(
                "file {index} must rank first at cosine 1 for its own text after the restart; top-{TOP_K}: {ranked:?}"
            )
            .into());
        }
    }
    Ok(())
}
