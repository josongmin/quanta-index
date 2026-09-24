//! QI-BB-029 완료 기준 #2 and QI-BB-003 완료 기준 #2 — a crash at every
//!
//! point from a seal's first track to the end of the GC it triggers leaves
//! a state the restarted daemon serves correctly, and the retried seal
//! converges on the same generation.
//!
//! Every case runs the real daemon binary over its own private state root
//! and crashes it with `QUANTA_INDEX_CRASH_POINT` naming one point
//! (`quanta_index_search_plane::crash_point`): the daemon exits 86 there
//! and never acks the seal. A table per protocol lists each point with the
//! disk state its crash must leave — the proof that the crash landed where
//! its name says — and a coverage test holds the tables to every point the
//! protocol declares.
//!
//! - The seal of generation 2 over an active 1. After the restart a pin to
//!   2 is not ready on either route — past the materialized head, it may
//!   yet become serveable — and boot names the half-sealed pair a lone
//!   sealed track makes.
//!   The retried seal converges on 2 — acked, activated, both generations
//!   serving on both routes — rebuilding only what the crash left unsealed:
//!   every file of a track the crash left sealed is the one it left. One
//!   case also removes the lexical sealed identity after the crash; the
//!   lexical seal writes the identity last, so that is exactly the state a
//!   crash inside the lexical seal leaves.
//! - The GC the seal of 4 triggers under a cap of 3, over 1, 2 and 3
//!   activated in turn: it retires 1. After the restart the predecessor 2,
//!   the active 3 and the candidate 4 are whole on both tracks and serve on
//!   both routes, and 1 is unknown. The retried seal finishes the GC — 1
//!   gone from both tracks, both reclaim areas empty — and 3 rolls back to
//!   its predecessor 2.
//!
//! A last case leaves the retired generation in the lexical reclaim area
//! while the daemon is down — what a crash between the move and the removal
//! leaves — and boot finishes it before anything else.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::searchd_binary_process::{
    SOCKET_TIMEOUT, SearchdBinaryProcess, remove_socket_files, searchd_command, terminate_child,
    wait_for_sockets,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusActiveHeadV1, SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchScopeKey,
    SearchScopeSurface,
};
use quanta_index_core::{GenerationStorageKeyV1, RECLAIM_AREA_DIR_NAME};
use quanta_index_sdk::{ConnectOptions, LanguageCode, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_search_plane::crash_point::{
    self, AFTER_CATALOG_TRANSACTION, AFTER_FENCE, AFTER_LEDGER_RECONCILE, AFTER_RETENTION_RECEIPT,
    AFTER_SEMANTIC_SEAL, BEFORE_AUTHORITY_RECORD, BEFORE_RECORD_FORGET, BETWEEN_TRACK_RECLAIMS,
    CRASH_EXIT_CODE, CRASH_POINT_ENV,
};

type TestResult = Result<(), Box<dyn Error>>;

const REPO: &str = "repo-crash-matrix";
const REVISION: &str = "revision-crash-matrix";
const TRACK_ROOTS: [&str; 2] = ["indexes/lexical", "indexes/semantic"];
/// The lexical seal's last write: its presence promotes the generation.
const LEXICAL_SEALED_IDENTITY: &str = "search-corpus-generation-identity.cbor";

fn digest(generation: u64) -> String {
    format!("manifest:crash-matrix:g{generation}")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn pin(generation: u64) -> GenerationPin {
    GenerationPin::new(
        RepoId::new(REPO).expect("test fixture ID satisfies canonical policy"),
        RevisionId::new(REVISION).expect("test fixture ID satisfies canonical policy"),
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
        RepoId::new(REPO)?,
        RevisionId::new(REVISION)?,
        ManifestGeneration::new(generation),
        digest(generation),
    )
    .replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new(path.clone()),
        },
        format!("scope:crash-matrix:{generation}"),
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
    expected_active: Option<SearchCorpusActiveHeadV1>,
) -> Result<SearchCorpusActiveHeadV1, Box<dyn Error>> {
    let (_receipt, activation) = client
        .search_corpus()
        .publish_and_activate(&batch(generation)?, expected_active)?;
    Ok(activation.active)
}

/// The directory `generation` has on `track_root`.
#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn generation_dir(state_root: &Path, track_root: &str, generation: u64) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(
        &RepoId::new(REPO).expect("test fixture ID satisfies canonical policy"),
        &RevisionId::new(REVISION).expect("test fixture ID satisfies canonical policy"),
    )
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

fn refused_with(
    outcome: Result<impl std::fmt::Debug, SdkError>,
    route: &str,
    generation: u64,
    expected: &str,
) -> TestResult {
    match outcome {
        Err(SdkError::Remote { code, .. }) if code.as_wire_str() == expected => Ok(()),
        other => Err(format!(
            "the {route} route pinned to generation {generation} answers {other:?}, not {expected}"
        )
        .into()),
    }
}

/// Both pinned routes refuse `generation`, each with its code (lexical,
/// semantic).
fn refused(client: &QuantaIndex, generation: u64, codes: [&str; 2]) -> TestResult {
    refused_with(
        client
            .lexical()
            .query()
            .native("needle")
            .pinned(pin(generation))
            .top_k(5)
            .execute(),
        "lexical",
        generation,
        codes[0],
    )?;
    refused_with(
        client
            .semantic()
            .query()
            .text("needle")
            .pinned(pin(generation))
            .top_k(5)
            .execute(),
        "semantic",
        generation,
        codes[1],
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

/// Publish `generation` under a daemon over `state_root` that keeps `keep`
/// generations per pair and exits at `point`; the state root is left as
/// the crash left it.
fn publish_through_a_crash(
    state_root: &Path,
    point: &str,
    keep: usize,
    generation: u64,
) -> TestResult {
    let mut command = searchd_command(state_root, keep);
    let _configured = command.env(CRASH_POINT_ENV, point);
    let mut child = command.spawn()?;
    wait_for_sockets(state_root, &mut child)?;
    let crashing = QuantaIndex::connect(ConnectOptions::from_state_root(state_root))?;
    if let Ok(receipt) = crashing.search_corpus().publish(&batch(generation)?) {
        terminate_child(&mut child)?;
        return Err(format!(
            "the seal of {generation} was acked although the daemon exits during it at {point}: {receipt:?}"
        )
        .into());
    }
    let status = wait_for_exit(state_root, &mut child)?;
    if status.code() != Some(CRASH_EXIT_CODE) {
        return Err(format!("the daemon exited {status} rather than at {point}").into());
    }
    Ok(())
}

/// The boot gauge `name` reads exactly `expected`, compared bit for bit: a
/// whole count is exact in f64.
fn boot_gauge_reads(client: &QuantaIndex, name: &str, expected: u32) -> TestResult {
    let snapshot = client.observability().metrics_snapshot()?;
    let value = snapshot
        .gauges
        .iter()
        .find(|gauge| gauge.name == name)
        .map(|gauge| gauge.value);
    if value.map(f64::to_bits) != Some(f64::from(expected).to_bits()) {
        return Err(format!("boot gauge {name} reads {value:?}, not {expected}").into());
    }
    Ok(())
}

/// Every file under `dir` by its path relative to `dir`, with its inode and
/// modification time: a file a rebuild wrote anew has a new stamp.
fn file_stamps(dir: &Path) -> Result<BTreeMap<PathBuf, (u64, SystemTime)>, Box<dyn Error>> {
    let mut stamps = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let metadata = entry.metadata()?;
                let _previous = stamps.insert(
                    entry.path().strip_prefix(dir)?.to_path_buf(),
                    (metadata.ino(), metadata.modified()?),
                );
            }
        }
    }
    Ok(stamps)
}

/// Run every case on its own state root; the failures of all of them, each
/// under its crash point.
fn every_case<C>(
    cases: &[C],
    point: fn(&C) -> &'static str,
    run: fn(&C) -> TestResult,
) -> TestResult {
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|case| match run(case) {
            Ok(()) => None,
            Err(error) => Some(format!("{}: {error}", point(case))),
        })
        .collect();
    if failures.is_empty() {
        return Ok(());
    }
    Err(failures.join("\n").into())
}

/// The history cap of the seal cases: nothing is retired.
const SEAL_KEEP: usize = 8;
/// What each route (lexical, semantic) answers a pin past the materialized
/// head: not ready — it may yet become serveable — the semantic route in
/// its own exact readiness code.
const NOT_YET_SERVEABLE: [&str; 2] = ["NOT_READY", "SEMANTIC_GENERATION_NOT_MATERIALIZED"];

/// A crash during a seal: where, and what the disk and boot show for it.
struct SealCase {
    point: &'static str,
    /// Remove the lexical sealed identity after the crash: the state a
    /// crash inside the lexical seal leaves (see the module doc).
    unseal_lexical: bool,
    /// Whether generation 2 has a directory on each track (lexical,
    /// semantic) after the crash.
    dirs: [bool; 2],
    /// The tracks the crash left sealed: the retry keeps every file of
    /// theirs and builds the others anew.
    sealed: [bool; 2],
    /// The pairs boot names half-sealed: a sealed track whose partner is
    /// not sealed.
    half_sealed_pairs: u32,
}

const SEAL_CASES: [SealCase; 3] = [
    SealCase {
        point: AFTER_SEMANTIC_SEAL,
        unseal_lexical: false,
        dirs: [false, true],
        sealed: [false, true],
        half_sealed_pairs: 1,
    },
    SealCase {
        point: BEFORE_AUTHORITY_RECORD,
        unseal_lexical: false,
        dirs: [true, true],
        sealed: [true, true],
        half_sealed_pairs: 0,
    },
    SealCase {
        point: BEFORE_AUTHORITY_RECORD,
        unseal_lexical: true,
        dirs: [true, true],
        sealed: [false, true],
        half_sealed_pairs: 1,
    },
];

/// One seal crash case, from the crash to the converged retry.
fn a_seal_crash(case: &SealCase) -> TestResult {
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = std::fs::canonicalize(root.path())?;
    let first = SearchdBinaryProcess::start_with_history_max_generations(&state_root, SEAL_KEEP)?;
    let client = first.connect()?;
    let one = publish_and_activate(&client, 1, None)?;
    drop(client);
    first.stop()?;
    publish_through_a_crash(&state_root, case.point, SEAL_KEEP, 2)?;
    if case.unseal_lexical {
        std::fs::remove_file(
            generation_dir(&state_root, "indexes/lexical", 2).join(LEXICAL_SEALED_IDENTITY),
        )?;
    }
    // (track root, whether the crash left it sealed, its files as left)
    let mut left = Vec::new();
    for ((track_root, dir), sealed) in TRACK_ROOTS.into_iter().zip(case.dirs).zip(case.sealed) {
        let path = generation_dir(&state_root, track_root, 2);
        if path.is_dir() != dir {
            return Err(format!("the crash leaves generation 2 on {track_root}: {dir}").into());
        }
        if dir {
            left.push((track_root, sealed, file_stamps(&path)?));
        }
    }

    let restarted =
        SearchdBinaryProcess::start_with_history_max_generations(&state_root, SEAL_KEEP)?;
    let client = restarted.connect()?;
    boot_gauge_reads(&client, "boot_half_sealed_pairs", case.half_sealed_pairs)?;
    refused(&client, 2, NOT_YET_SERVEABLE)?;
    serves(&client, 1)?;

    let _two = publish_and_activate(&client, 2, Some(one))?;
    serves(&client, 1)?;
    serves(&client, 2)?;
    for (track_root, sealed, before) in left {
        let after = file_stamps(&generation_dir(&state_root, track_root, 2))?;
        let kept = before
            .iter()
            .all(|(path, stamp)| after.get(path) == Some(stamp));
        if kept != sealed {
            return Err(format!(
                "the retry {} the {track_root} track the crash left {}",
                if kept { "kept" } else { "rewrote" },
                if sealed { "sealed" } else { "unsealed" },
            )
            .into());
        }
    }
    drop(client);
    restarted.stop()
}

/// The history cap of the GC cases: the seal of 4 retires 1.
const GC_KEEP: usize = 3;
/// The generations the GC must not touch — the predecessor, the active
/// generation and the candidate — and the one it retires.
const RETAINED: [u64; 3] = [2, 3, 4];
const RETIRED: u64 = 1;

/// A crash during the GC the seal of 4 triggers: where, and which tracks
/// (lexical, semantic) still hold the retired generation right after it.
struct GcCase {
    point: &'static str,
    retired_left: [bool; 2],
}

const GC_CASES: [GcCase; 6] = [
    GcCase {
        point: AFTER_RETENTION_RECEIPT,
        retired_left: [true, true],
    },
    GcCase {
        point: AFTER_CATALOG_TRANSACTION,
        retired_left: [true, true],
    },
    GcCase {
        point: AFTER_LEDGER_RECONCILE,
        retired_left: [true, true],
    },
    GcCase {
        point: AFTER_FENCE,
        retired_left: [true, true],
    },
    GcCase {
        point: BETWEEN_TRACK_RECLAIMS,
        retired_left: [false, true],
    },
    GcCase {
        point: BEFORE_RECORD_FORGET,
        retired_left: [false, false],
    },
];

/// The predecessor and the active generation the crash leaves behind.
struct ActiveLine {
    predecessor: SearchCorpusActiveHeadV1,
    active: SearchCorpusActiveHeadV1,
}

/// Seal 1, 2 and 3 active in turn, then seal 4 under a daemon that exits at
/// `point`; the state root is left as the crash left it.
fn seal_through_a_gc_crash(state_root: &Path, point: &str) -> Result<ActiveLine, Box<dyn Error>> {
    // Sealing 1–3 retires nothing under the first daemon's cap of 8.
    let first = SearchdBinaryProcess::start(state_root)?;
    let client = first.connect()?;
    let one = publish_and_activate(&client, 1, None)?;
    let predecessor = publish_and_activate(&client, 2, Some(one))?;
    let active = publish_and_activate(&client, 3, Some(predecessor.clone()))?;
    drop(client);
    first.stop()?;
    publish_through_a_crash(state_root, point, GC_KEEP, 4)?;
    Ok(ActiveLine {
        predecessor,
        active,
    })
}

/// Every retained generation has its directory on both tracks and serves
/// on both routes; the retired one is unknown.
fn retained_whole_and_retired_unknown(state_root: &Path, client: &QuantaIndex) -> TestResult {
    for track_root in TRACK_ROOTS {
        for generation in RETAINED {
            if !generation_dir(state_root, track_root, generation).is_dir() {
                return Err(format!(
                    "retained generation {generation} lost its {track_root} directory"
                )
                .into());
            }
        }
    }
    for generation in RETAINED {
        serves(client, generation)?;
    }
    refused(client, RETIRED, ["UNKNOWN_GENERATION"; 2])
}

/// One GC crash case, from the crash to the retried seal.
///
/// The crash leaves the retired generation on exactly the tracks the case
/// names and nothing in either reclaim area. The predecessor, the active
/// generation and the candidate survive whole, the retired generation is
/// unknown, a retried seal finishes the GC, and the predecessor is still a
/// rollback target.
fn a_gc_crash(case: &GcCase) -> TestResult {
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = std::fs::canonicalize(root.path())?;
    let line = seal_through_a_gc_crash(&state_root, case.point)?;
    for (track_root, left) in TRACK_ROOTS.into_iter().zip(case.retired_left) {
        if generation_dir(&state_root, track_root, RETIRED).is_dir() != left
            || reclaim_area_entries(&state_root, track_root)? != 0
        {
            return Err(format!(
                "the crash leaves retired generation {RETIRED} on {track_root}: {left}, with an empty reclaim area"
            )
            .into());
        }
    }

    let restarted = SearchdBinaryProcess::start_with_history_max_generations(&state_root, GC_KEEP)?;
    let client = restarted.connect()?;
    retained_whole_and_retired_unknown(&state_root, &client)?;

    let _retried = client.search_corpus().publish(&batch(4)?)?;
    for track_root in TRACK_ROOTS {
        if generation_dir(&state_root, track_root, RETIRED).exists() {
            return Err(format!(
                "the retried seal left retired generation {RETIRED} on {track_root}"
            )
            .into());
        }
        let left = reclaim_area_entries(&state_root, track_root)?;
        if left != 0 {
            return Err(format!("{left} entries stay in the {track_root} reclaim area").into());
        }
    }
    retained_whole_and_retired_unknown(&state_root, &client)?;
    let rolled_back =
        client
            .generations()
            .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: line.active,
                target: line.predecessor.generation.clone(),
            })?;
    if rolled_back.active.generation != line.predecessor.generation {
        return Err(format!("the rollback activated {rolled_back:?}").into());
    }
    drop(client);
    restarted.stop()
}

#[test]
fn the_matrix_has_a_case_for_every_crash_point() -> TestResult {
    let cased: BTreeSet<&str> = SEAL_CASES
        .iter()
        .map(|case| case.point)
        .chain(GC_CASES.iter().map(|case| case.point))
        .collect();
    let declared: BTreeSet<&str> = crash_point::ALL.into_iter().collect();
    if cased != declared {
        return Err(format!(
            "points without a case: {:?}; cases without a declared point: {:?}",
            declared.difference(&cased).collect::<Vec<_>>(),
            cased.difference(&declared).collect::<Vec<_>>(),
        )
        .into());
    }
    Ok(())
}

#[test]
fn every_seal_crash_converges_on_the_same_generation() -> TestResult {
    every_case(&SEAL_CASES, |case| case.point, a_seal_crash)
}

#[test]
fn every_gc_crash_loses_nothing_retained() -> TestResult {
    every_case(&GC_CASES, |case| case.point, a_gc_crash)
}

/// A retired generation left in the lexical reclaim area — moved out of its
/// namespace, not yet removed — is finished at boot, counted in the boot
/// gauge, and never listed as a quarantine finding.
#[test]
fn an_interrupted_removal_is_finished_at_boot() -> TestResult {
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = std::fs::canonicalize(root.path())?;
    let _line = seal_through_a_gc_crash(&state_root, AFTER_FENCE)?;
    let area = state_root
        .join("indexes/lexical")
        .join(RECLAIM_AREA_DIR_NAME);
    std::fs::create_dir_all(&area)?;
    let key = GenerationStorageKeyV1::for_repo_revision(
        &RepoId::new(REPO).expect("test fixture ID satisfies canonical policy"),
        &RevisionId::new(REVISION).expect("test fixture ID satisfies canonical policy"),
    );
    std::fs::rename(
        generation_dir(&state_root, "indexes/lexical", RETIRED),
        area.join(key.reclaim_entry_name(ManifestGeneration::new(RETIRED))),
    )?;

    let restarted = SearchdBinaryProcess::start_with_history_max_generations(&state_root, GC_KEEP)?;
    let client = restarted.connect()?;
    boot_gauge_reads(&client, "boot_lexical_interrupted_reclaims_finished", 1)?;
    boot_gauge_reads(&client, "boot_lexical_interrupted_reclaims_unfinished", 0)?;
    if reclaim_area_entries(&state_root, "indexes/lexical")? != 0 {
        return Err("boot leaves the interrupted removal in the reclaim area".into());
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
    retained_whole_and_retired_unknown(&state_root, &client)?;
    drop(client);
    restarted.stop()
}
