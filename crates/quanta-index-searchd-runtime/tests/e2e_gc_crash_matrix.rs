//! QI-BB-003 완료 기준 #2 — the active generation, the candidate and the
//! predecessor survive a crash at every point of the physical GC protocol,
//! and a retried seal finishes what the crash interrupted.
//!
//! Each case runs the real daemon binary over one state root, history
//! keeping three generations per pair:
//!
//! 1. seal and activate 1, 2, then 3 — 2 is the predecessor of the active
//!    3;
//! 2. restart with `QUANTA_INDEX_GC_CRASH_POINT` naming one point and seal
//!    the candidate 4, which retires 1 — the daemon exits at that point,
//!    cutting the request off;
//! 3. restart clean: 2, 3 and 4 are whole on both tracks and answer pinned
//!    queries on both routes, and 1 is unknown;
//! 4. retry the seal of 4, as the producer whose request the crash cut off
//!    would: it is acked, 1 is gone from both tracks, both reclaim areas
//!    are empty, 2, 3 and 4 still serve, and the active 3 rolls back to its
//!    predecessor 2.
//!
//! A last case leaves the retired generation in the lexical reclaim area
//! while the daemon is down — what a crash between the move and the removal
//! leaves — and boot finishes it before anything else.

#![forbid(unsafe_code)]

#[path = "common/searchd_binary_process.rs"]
mod searchd_binary_process;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchScopeKey, SearchScopeSurface,
};
use quanta_index_core::{GenerationStorageKeyV1, RECLAIM_AREA_DIR_NAME};
use quanta_index_sdk::{
    ConnectOptions, LanguageCode, QuantaIndex, SdkError, SearchCorpusBatch,
    SearchCorpusGenerationIdentityV1,
};
use quanta_index_search_plane::gc_crash_point::{
    AFTER_CATALOG_TRANSACTION, AFTER_FENCE, AFTER_LEDGER_RECONCILE, AFTER_RETENTION_RECEIPT,
    BEFORE_RECORD_FORGET, BETWEEN_TRACK_RECLAIMS, GC_CRASH_EXIT_CODE, GC_CRASH_POINT_ENV,
};
use searchd_binary_process::{
    SOCKET_TIMEOUT, SearchdBinaryProcess, remove_socket_files, searchd_command, terminate_child,
    wait_for_sockets,
};

type TestResult = Result<(), Box<dyn Error>>;

const REPO: &str = "repo-gc-crash";
const REVISION: &str = "revision-gc-crash";
const KEEP: usize = 3;
/// The generations the crash must not touch, and the one it retires.
const RETAINED: [u64; 3] = [2, 3, 4];
const RETIRED: u64 = 1;
const TRACK_ROOTS: [&str; 2] = ["indexes/lexical", "indexes/semantic"];

fn digest(generation: u64) -> String {
    format!("manifest:gc-crash:g{generation}")
}

fn pin(generation: u64) -> GenerationPin {
    GenerationPin::new(
        RepoId::new(REPO),
        RevisionId::new(REVISION),
        ManifestGeneration::new(generation),
    )
}

/// Generation `generation`: one file whose text names the generation, so a
/// query for `needle{generation}` finds exactly its chunk.
fn batch(generation: u64) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let path = format!("src/g{generation}.rs");
    let text = format!("fn needle{generation}() {{}}");
    let end_byte = u32::try_from(text.len())?;
    Ok(SearchCorpusBatch::replace_generation(
        RepoId::new(REPO),
        RevisionId::new(REVISION),
        ManifestGeneration::new(generation),
        digest(generation),
    )
    .replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(path.clone()),
        },
        format!("scope:gc-crash:{generation}"),
        vec![ChunkRecord {
            chunk_id: ChunkId::new(format!("chunk-g{generation}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line: 1,
            text: text.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }],
        Vec::new(),
    ))
}

fn publish_and_activate(
    client: &QuantaIndex,
    generation: u64,
    expected_active: Option<SearchCorpusGenerationIdentityV1>,
) -> Result<SearchCorpusGenerationIdentityV1, Box<dyn Error>> {
    let (_receipt, activation) = client
        .search_corpus()
        .publish_and_activate(&batch(generation)?, expected_active)?;
    Ok(activation.active)
}

/// The directory `generation` has on `track_root`.
fn generation_dir(state_root: &Path, track_root: &str, generation: u64) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&RepoId::new(REPO), &RevisionId::new(REVISION))
        .generation_dir(
            &state_root.join(track_root),
            ManifestGeneration::new(generation),
        )
}

/// Entries the reclaim area of `track_root` holds.
fn reclaim_area_entries(state_root: &Path, track_root: &str) -> Result<usize, Box<dyn Error>> {
    let area = state_root.join(track_root).join(RECLAIM_AREA_DIR_NAME);
    if !area.is_dir() {
        return Ok(0);
    }
    Ok(std::fs::read_dir(area)?.count())
}

/// Both pinned routes answer `generation`'s own needle with its chunk.
fn serves(client: &QuantaIndex, generation: u64) -> TestResult {
    let lexical = client
        .lexical()
        .query()
        .native(format!("needle{generation}"))
        .pinned(pin(generation))
        .top_k(5)
        .execute()?;
    let found = lexical
        .results
        .iter()
        .any(|hit| hit.candidate_id == format!("chunk-g{generation}"));
    if lexical.generation != pin(generation) || !found {
        return Err(format!(
            "the lexical route does not serve generation {generation}: {lexical:?}"
        )
        .into());
    }
    let semantic = client
        .semantic()
        .query()
        .text(format!("needle{generation}"))
        .pinned(pin(generation))
        .top_k(5)
        .execute()?;
    if semantic.generation != pin(generation) || semantic.results.is_empty() {
        return Err(format!(
            "the semantic route does not serve generation {generation}: {semantic:?}"
        )
        .into());
    }
    Ok(())
}

fn refused_unknown(outcome: Result<impl std::fmt::Debug, SdkError>, route: &str) -> TestResult {
    match outcome {
        Err(SdkError::Remote { code, .. }) if code == "UNKNOWN_GENERATION" => Ok(()),
        other => Err(
            format!("the {route} route pinned to a retired generation answers {other:?}").into(),
        ),
    }
}

/// Both pinned routes refuse `generation` as unknown.
fn unknown(client: &QuantaIndex, generation: u64) -> TestResult {
    refused_unknown(
        client
            .lexical()
            .query()
            .native("needle")
            .pinned(pin(generation))
            .top_k(5)
            .execute(),
        "lexical",
    )?;
    refused_unknown(
        client
            .semantic()
            .query()
            .text("needle")
            .pinned(pin(generation))
            .top_k(5)
            .execute(),
        "semantic",
    )
}

/// Wait for a daemon that must exit on its own; its exit status.
fn wait_for_exit(state_root: &Path, child: &mut Child) -> Result<ExitStatus, Box<dyn Error>> {
    let start = Instant::now();
    while start.elapsed() < SOCKET_TIMEOUT {
        if let Some(status) = child.try_wait()? {
            remove_socket_files(state_root)?;
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(10));
    }
    terminate_child(child)?;
    remove_socket_files(state_root)?;
    Err("the daemon never reached its crash point".into())
}

/// The predecessor and the active generation the crash leaves behind.
struct ActiveLine {
    predecessor: SearchCorpusGenerationIdentityV1,
    active: SearchCorpusGenerationIdentityV1,
}

/// Seal 1, 2 and 3 active in turn, then seal 4 under a daemon that exits at
/// `point`; the state root is left as the crash left it.
fn seal_through_a_crash(state_root: &Path, point: &str) -> Result<ActiveLine, Box<dyn Error>> {
    // Sealing 1–3 retires nothing under the first daemon's cap of 8.
    let first = SearchdBinaryProcess::start(state_root)?;
    let client = first.connect()?;
    let one = publish_and_activate(&client, 1, None)?;
    let predecessor = publish_and_activate(&client, 2, Some(one))?;
    let active = publish_and_activate(&client, 3, Some(predecessor.clone()))?;
    drop(client);
    first.stop()?;

    let mut command = searchd_command(state_root, KEEP);
    let _configured = command.env(GC_CRASH_POINT_ENV, point);
    let mut child = command.spawn()?;
    wait_for_sockets(state_root, &mut child)?;
    let crashing = QuantaIndex::connect(ConnectOptions::from_state_root(state_root))?;
    if let Ok(receipt) = crashing.search_corpus().publish(&batch(4)?) {
        terminate_child(&mut child)?;
        return Err(format!(
            "the seal of 4 was acked although the daemon exits during it at {point}: {receipt:?}"
        )
        .into());
    }
    let status = wait_for_exit(state_root, &mut child)?;
    if status.code() != Some(GC_CRASH_EXIT_CODE) {
        return Err(format!("the daemon exited {status} rather than at {point}").into());
    }
    Ok(ActiveLine {
        predecessor,
        active,
    })
}

/// Every retained generation has its directory on both tracks and serves
/// on both routes; the retired one is unknown.
fn retained_whole_and_retired_unknown(
    state_root: &Path,
    client: &QuantaIndex,
    point: &str,
) -> TestResult {
    for track_root in TRACK_ROOTS {
        for generation in RETAINED {
            if !generation_dir(state_root, track_root, generation).is_dir() {
                return Err(format!(
                    "{point}: retained generation {generation} lost its {track_root} directory"
                )
                .into());
            }
        }
    }
    for generation in RETAINED {
        serves(client, generation)?;
    }
    unknown(client, RETIRED)
}

/// One crash point, from the crash to the retried seal.
///
/// The crash leaves the retired generation on exactly the tracks
/// `retired_left` names (lexical, semantic) — the disk state that proves
/// the point interrupted the protocol where its name says — and nothing in
/// either reclaim area. The predecessor, the active generation and the
/// candidate survive whole, the retired generation is unknown, a retried
/// seal finishes the GC, and the predecessor is still a rollback target.
fn a_crash_at(point: &str, retired_left: [bool; 2]) -> TestResult {
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = std::fs::canonicalize(root.path())?;
    let line = seal_through_a_crash(&state_root, point)?;
    for (track_root, left) in TRACK_ROOTS.into_iter().zip(retired_left) {
        if generation_dir(&state_root, track_root, RETIRED).is_dir() != left
            || reclaim_area_entries(&state_root, track_root)? != 0
        {
            return Err(format!(
                "{point}: the crash leaves retired generation {RETIRED} on {track_root}: {left}, with an empty reclaim area"
            )
            .into());
        }
    }

    let restarted = SearchdBinaryProcess::start_with_history_max_generations(&state_root, KEEP)?;
    let client = restarted.connect()?;
    retained_whole_and_retired_unknown(&state_root, &client, point)?;

    let _retried = client.search_corpus().publish(&batch(4)?)?;
    for track_root in TRACK_ROOTS {
        if generation_dir(&state_root, track_root, RETIRED).exists() {
            return Err(format!(
                "{point}: the retried seal left retired generation {RETIRED} on {track_root}"
            )
            .into());
        }
        let left = reclaim_area_entries(&state_root, track_root)?;
        if left != 0 {
            return Err(
                format!("{point}: {left} entries stay in the {track_root} reclaim area").into(),
            );
        }
    }
    retained_whole_and_retired_unknown(&state_root, &client, point)?;
    let rolled_back =
        client
            .generations()
            .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: line.active,
                target: line.predecessor.clone(),
            })?;
    if rolled_back.active != line.predecessor {
        return Err(format!("{point}: the rollback activated {rolled_back:?}").into());
    }
    drop(client);
    restarted.stop()
}

#[test]
fn a_crash_after_the_retention_receipt_loses_nothing_retained() -> TestResult {
    a_crash_at(AFTER_RETENTION_RECEIPT, [true, true])
}

#[test]
fn a_crash_after_the_catalog_transaction_loses_nothing_retained() -> TestResult {
    a_crash_at(AFTER_CATALOG_TRANSACTION, [true, true])
}

#[test]
fn a_crash_after_the_ledger_reconcile_loses_nothing_retained() -> TestResult {
    a_crash_at(AFTER_LEDGER_RECONCILE, [true, true])
}

#[test]
fn a_crash_after_the_fence_loses_nothing_retained() -> TestResult {
    a_crash_at(AFTER_FENCE, [true, true])
}

#[test]
fn a_crash_between_the_track_reclaims_loses_nothing_retained() -> TestResult {
    a_crash_at(BETWEEN_TRACK_RECLAIMS, [false, true])
}

#[test]
fn a_crash_before_the_record_forget_loses_nothing_retained() -> TestResult {
    a_crash_at(BEFORE_RECORD_FORGET, [false, false])
}

/// A retired generation left in the lexical reclaim area — moved out of its
/// namespace, not yet removed — is finished at boot, counted in the boot
/// gauge, and never listed as a quarantine finding.
#[test]
fn an_interrupted_removal_is_finished_at_boot() -> TestResult {
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = std::fs::canonicalize(root.path())?;
    let _line = seal_through_a_crash(&state_root, AFTER_FENCE)?;
    let lexical_root = state_root.join("indexes/lexical");
    let area = lexical_root.join(RECLAIM_AREA_DIR_NAME);
    std::fs::create_dir_all(&area)?;
    let key =
        GenerationStorageKeyV1::for_repo_revision(&RepoId::new(REPO), &RevisionId::new(REVISION));
    std::fs::rename(
        generation_dir(&state_root, "indexes/lexical", RETIRED),
        area.join(key.reclaim_entry_name(ManifestGeneration::new(RETIRED))),
    )?;

    let restarted = SearchdBinaryProcess::start_with_history_max_generations(&state_root, KEEP)?;
    let client = restarted.connect()?;
    let snapshot = client.observability().metrics_snapshot()?;
    // Whole counts, exact in f64: compared bit for bit.
    let gauge_bits = |name: &str| {
        snapshot
            .gauges
            .iter()
            .find(|gauge| gauge.name == name)
            .map(|gauge| gauge.value.to_bits())
    };
    let finished = gauge_bits("boot_lexical_interrupted_reclaims_finished");
    let unfinished = gauge_bits("boot_lexical_interrupted_reclaims_unfinished");
    let as_expected = finished == Some(1.0_f64.to_bits()) && unfinished == Some(0.0_f64.to_bits());
    if !as_expected || reclaim_area_entries(&state_root, "indexes/lexical")? != 0 {
        return Err(format!("boot finishes the one interrupted removal: finished={finished:?} unfinished={unfinished:?}").into());
    }
    let quarantined = client.quarantine().inventory()?;
    if quarantined
        .lexical
        .iter()
        .chain(&quarantined.semantic)
        .any(|entry| entry.path.contains(RECLAIM_AREA_DIR_NAME))
    {
        return Err(
            format!("the reclaim area is not a quarantine finding: {quarantined:?}").into(),
        );
    }
    retained_whole_and_retired_unknown(&state_root, &client, AFTER_FENCE)?;
    drop(client);
    restarted.stop()
}
