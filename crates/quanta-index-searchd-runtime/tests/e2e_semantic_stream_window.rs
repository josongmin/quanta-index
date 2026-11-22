//! Pinned Legacy reference: one semantic batch streams through four bounded
//! windows, and a fresh daemon serves every row after reopening the generation.
//!
//! This is a compatibility oracle for an external old-Q executable, not Native
//! ANN qualification. Expected window and candidate identities come from the
//! fixed fixture, independently of the daemon's measured counters.

#![forbid(unsafe_code)]

use std::{error::Error, path::Path};

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, MetricsSnapshotV1, RepoId, RevisionId,
};
use quanta_index_core::{SEMANTIC_STREAM_WINDOW_SCOPES, SemanticStreamWindowPolicy, count_as_f64};
use quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION;
use quanta_index_searchd_harness::{
    legacy_reference::{
        LegacySearchdBinary, LegacySearchdCommand, SearchdBinaryProcess, publish_legacy_source,
    },
    private_tempdir,
};

type TestResult = Result<(), Box<dyn Error>>;

const FILES: u32 = 32;
const ROWS_PER_WINDOW: u32 = 8;
const EXPECTED_WINDOWS: u32 = FILES.div_ceil(ROWS_PER_WINDOW);
const _: () = assert!(EXPECTED_WINDOWS >= 3);
const TOP_K: u32 = 3;

fn file_content(index: u32) -> String {
    format!("fn sw{index}a() {{ let sw{index}b = sw{index}c(sw{index}d, sw{index}e); }}")
}

fn file_path(index: u32) -> String {
    format!("src/stream/item_{index:03}.rs")
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

fn configured_command<'binary>(
    binary: &'binary LegacySearchdBinary,
    state_root: &Path,
    bound: u64,
) -> Result<LegacySearchdCommand<'binary>, Box<dyn Error>> {
    let mut command = binary.benchmark_command(state_root, 8)?;
    let _configured = command
        .env(
            "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_SCOPES",
            SEMANTIC_STREAM_WINDOW_SCOPES.to_string(),
        )
        .env(
            "QUANTA_INDEX_SEMANTIC_STREAM_WINDOW_VECTOR_BYTES",
            bound.to_string(),
        );
    Ok(command)
}

#[test]
#[expect(
    clippy::print_stdout,
    reason = "the QI-BB-021-STREAM-EVIDENCE line is the measurement the ledger cites"
)]
fn a_batch_streams_through_bounded_windows_and_every_row_survives_a_restart() -> TestResult {
    let bound = SemanticStreamWindowPolicy::vector_bytes(
        usize::try_from(ROWS_PER_WINDOW)?,
        SEARCH_OWNED_SEMANTIC_DIMENSION,
    )?;
    let _policy = SemanticStreamWindowPolicy::new(SEMANTIC_STREAM_WINDOW_SCOPES, bound)?;
    let expected_windows = u64::from(EXPECTED_WINDOWS);
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let pin = GenerationPin::new(repo.clone(), revision.clone(), ManifestGeneration::new(1));
    let files: Vec<(String, String, u64)> = (0..FILES)
        .map(|index| (file_path(index), file_content(index), u64::from(index) + 1))
        .collect();
    let borrowed: Vec<(&str, &str, u64)> = files
        .iter()
        .map(|(path, content, id)| (path.as_str(), content.as_str(), *id))
        .collect();
    let ids: Vec<String> = files
        .iter()
        .map(|(path, _, id)| format!("e2e-{id}-{path}"))
        .collect();
    if ids.len() != usize::try_from(FILES)? {
        return Err("semantic fixture did not create 32 candidate IDs".into());
    }

    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let producer = SearchdBinaryProcess::start_with_pinned_command(
        &state_root,
        configured_command(&binary, &state_root, bound)?,
    )?;
    let published = (|| -> TestResult {
        let client = producer.connect()?;
        let (head, receipt) =
            publish_legacy_source(&client, &repo, &revision, 1, None, &borrowed, None)?;
        if receipt.generation != pin.manifest_generation
            || receipt.accepted_replace_scopes != FILES
            || receipt.accepted_semantic_replace_scopes != FILES
            || !receipt.sealed
            || head.generation.lexical.manifest_generation != pin.manifest_generation
        {
            return Err(
                format!("one-batch 32-file semantic publication differs: {receipt:?}").into(),
            );
        }
        let snapshot = client.observability().metrics_snapshot()?;
        let windows = counter(&snapshot, "semantic_ingest_windows_total")?;
        let peak = gauge(&snapshot, "semantic_ingest_resident_vector_bytes_peak")?;
        println!(
            "QI-BB-021-STREAM-EVIDENCE scopes={FILES} windows={windows} peak_resident_vector_bytes={peak} bound={bound} generation={}",
            pin.manifest_generation.get()
        );
        if windows != expected_windows {
            return Err(format!(
                "{FILES} one-row owner scopes under an {ROWS_PER_WINDOW}-row window are {expected_windows} windows, the scrape counted {windows}"
            ).into());
        }
        if peak.to_bits() != count_as_f64(bound).to_bits() {
            return Err(format!(
                "a full window holds exactly the bound resident: peak={peak} bound={bound}"
            )
            .into());
        }
        Ok(())
    })();
    let producer_stopped = producer.stop();
    published.and(producer_stopped)?;
    binary.verify()?;

    // Resolve the persisted active generation in a different OS process.
    let reopened = SearchdBinaryProcess::start_with_pinned_command(
        &state_root,
        configured_command(&binary, &state_root, bound)?,
    )?;
    let searched = (|| -> TestResult {
        let client = reopened.connect()?;
        for (index, id) in ids.iter().enumerate() {
            let index = u32::try_from(index)?;
            let response = client
                .semantic()
                .query()
                .text(file_content(index))
                .active(repo.clone(), revision.clone())
                .top_k(TOP_K)
                .execute()?;
            if response.generation != pin {
                return Err(
                    format!("semantic query for file {index} selected another generation").into(),
                );
            }
            let top = response.results.first();
            if top.map(|candidate| candidate.candidate_id.as_str()) != Some(id.as_str())
                || top.is_none_or(|candidate| {
                    !candidate.score.is_finite() || (candidate.score - 1.0).abs() > 1e-5
                })
            {
                let ranked: Vec<(String, f32)> = response
                    .results
                    .iter()
                    .map(|candidate| (candidate.candidate_id.clone(), candidate.score))
                    .collect();
                return Err(format!(
                    "file {index} must rank first at cosine 1 for its own text after the restart; top-{TOP_K}: {ranked:?}"
                ).into());
            }
        }
        Ok(())
    })();
    let reopened_stopped = reopened.stop();
    searched.and(reopened_stopped)?;
    binary.verify()
}
