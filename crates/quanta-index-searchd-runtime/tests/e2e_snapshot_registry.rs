//! QI-BB-001 / QI-BB-017 — opened sealed generations stay resident across
//! queries, and only a routed mutation can make the daemon reopen one.
//!
//! The oracle is fault injection at the daemon front door, not a counter
//! the registry maintains about itself. After the first query has opened a
//! generation, the test deletes the on-disk state that a cold open must
//! read. A second query that still answers identically could only have been
//! served from a resident handle: a reopen would fail on the missing files.
//! For the lexical track that is the whole generation directory (Tantivy
//! maps its segments and the sidecars are decoded on open). For the semantic
//! track it is the sealed marker and manifest that `open` proves before it
//! touches the dataset; the dataset itself stays because `LanceDB` reads
//! fragments per scan rather than at open.
//!
//! The immutability half is the same trick kept: an auxiliary batch that
//! names a sealed generation is refused typed (QI-BB-030), so nothing was
//! mutated and the next lexical *and* semantic query must still be served
//! from the resident handles after the files are deleted — ingest never
//! drops residency; only GC retirement does.
//!
//! Activation and restart promote the handles they proved (QI-BB-017 #4):
//! with the on-disk state deleted right after either, the *first* query
//! still serves and the registry's miss counter has not moved.
//!
//! Single-flight coalescing, fenced retirement and budgeted waits are
//! proven in the registry's unit tests with barrier-controlled openers.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RevisionId,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, TextQuerySyntax,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eQueryResult, E2eRuntime};

type TestResult = Result<(), Box<dyn Error>>;

const QUERY: &str = "needle";
const TOP_K: u32 = 10;

fn seeded_runtime() -> Result<E2eRuntime, Box<dyn Error>> {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo", "src/needle.rs", "fn needle() { let needle = 1; }")?;
    rt.ingest_text("repo", "src/other.rs", "fn other() { let needle = 2; }")?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

/// The generation the harness pins queries to: the last sealed one.
fn pinned_generation(rt: &E2eRuntime) -> ManifestGeneration {
    ManifestGeneration::new(rt.current_generation().get().saturating_sub(1))
}

fn generation_dir(
    state_root: &Path,
    track_root: &str,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
) -> Result<PathBuf, Box<dyn Error>> {
    // The daemon derives adapter roots from the leased, canonical state root.
    let canonical = std::fs::canonicalize(state_root)?;
    Ok(GenerationStorageKeyV1::for_repo_revision(repo, revision)
        .generation_dir(&canonical.join(track_root), generation))
}

fn lexical_generation_dir(rt: &E2eRuntime) -> Result<PathBuf, Box<dyn Error>> {
    generation_dir(
        rt.state_root(),
        "indexes/lexical",
        &rt.repo(),
        &rt.revision(),
        pinned_generation(rt),
    )
}

fn semantic_generation_dir(rt: &E2eRuntime) -> Result<PathBuf, Box<dyn Error>> {
    generation_dir(
        rt.state_root(),
        "indexes/semantic",
        &rt.repo(),
        &rt.revision(),
        pinned_generation(rt),
    )
}

fn ids(result: &E2eQueryResult) -> Result<Vec<String>, Box<dyn Error>> {
    if let Some(error) = &result.typed_error {
        return Err(format!("query was refused: {error}").into());
    }
    Ok(result.candidate_ids.clone())
}

/// Remove what a semantic cold open must read before it can serve.
fn remove_semantic_open_proofs(dir: &Path) -> Result<(), Box<dyn Error>> {
    for name in ["MARKER_SEALED", "semantic-manifest.cbor"] {
        let path = dir.join(name);
        if !path.is_file() {
            return Err(format!("expected semantic open proof at {}", path.display()).into());
        }
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// The second lexical query is served without any file being read: the
/// whole generation directory is gone and the answer is unchanged.
#[test]
fn second_lexical_query_reads_no_generation_file() -> TestResult {
    let mut rt = seeded_runtime()?;
    let first = ids(&rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K))?;
    if first.len() != 2 {
        return Err(format!("fixture served {} rows, expected 2", first.len()).into());
    }

    let dir = lexical_generation_dir(&rt)?;
    if !dir.is_dir() {
        return Err(format!("lexical generation dir not found at {}", dir.display()).into());
    }
    std::fs::remove_dir_all(&dir)?;

    let second = ids(&rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K))?;
    if second != first {
        return Err(format!(
            "resident lexical handle answered differently: first={first:?} second={second:?}"
        )
        .into());
    }
    Ok(())
}

/// The second semantic query does not re-prove the generation: its sealed
/// marker and manifest are gone and the answer is unchanged.
#[test]
fn second_semantic_query_does_not_reopen_the_generation() -> TestResult {
    let mut rt = seeded_runtime()?;
    let first = ids(&rt.query_semantic(QUERY, TOP_K, None))?;
    if first.is_empty() {
        return Err("fixture served no semantic rows".into());
    }

    remove_semantic_open_proofs(&semantic_generation_dir(&rt)?)?;

    let second = ids(&rt.query_semantic(QUERY, TOP_K, None))?;
    if second != first {
        return Err(format!(
            "resident semantic handle answered differently: first={first:?} second={second:?}"
        )
        .into());
    }
    Ok(())
}

/// An auxiliary publish that names a sealed generation is refused typed
/// and leaves its residency alone (QI-BB-030): nothing may land in a
/// sealed generation, so there is no mutation to invalidate for.
///
/// The refusal is proved from the outside: the publish answers
/// `GENERATION_IMMUTABLE`, no overlay file appears on disk, and the next
/// query is still served from the resident handle — the generation
/// directory is deleted first, so a cold open would fail instead.
#[test]
fn an_auxiliary_publish_into_a_sealed_generation_is_refused_and_keeps_residency() -> TestResult {
    let mut rt = seeded_runtime()?;
    let first = ids(&rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K))?;
    if first.len() != 2 {
        return Err(format!("fixture served {} rows, expected 2", first.len()).into());
    }
    let first_semantic = ids(&rt.query_semantic(QUERY, TOP_K, None))?;
    if first_semantic.is_empty() {
        return Err("fixture served no semantic rows".into());
    }
    let generation = pinned_generation(&rt);
    let response = rt.ingest_once(SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(
        RepoMetaIngestBatch {
            repo_id: rt.repo(),
            revision_id: rt.revision(),
            generation,
            batch_digest: "e2e-snapshot-invalidate".to_string(),
            entries: vec![RepoMetaEntry {
                source_repo_id: RepoId::new("repo"),
                key: "license".to_string(),
                value: "apache-2.0".to_string(),
            }],
        },
    ))?;
    let SearchPlaneIngestIpcResponse::Error(error) = response else {
        return Err(format!(
            "an auxiliary publish into a sealed generation must be refused typed GENERATION_IMMUTABLE, got {response:?}"
        )
        .into());
    };
    if error.code != "GENERATION_IMMUTABLE" {
        return Err(format!(
            "an auxiliary publish into a sealed generation must be refused typed GENERATION_IMMUTABLE, got {}: {}",
            error.code, error.message
        )
        .into());
    }

    // Nothing landed on disk in the sealed generation.
    let dir = lexical_generation_dir(&rt)?;
    for entry in std::fs::read_dir(&dir)? {
        if entry?.file_name().to_string_lossy().contains("repo-meta") {
            return Err(
                format!("a refused publish wrote an overlay into {}", dir.display()).into(),
            );
        }
    }
    std::fs::remove_dir_all(&dir)?;

    let after = rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K);
    if let Some(error) = after.typed_error {
        return Err(format!(
            "a refused publish dropped the resident handle; the query cold-opened and failed: {}: {}",
            error.code, error.message
        )
        .into());
    }
    if ids(&after)? != first {
        return Err(format!(
            "the resident handle served {:?}, expected {first:?}",
            after.candidate_ids
        )
        .into());
    }

    // The semantic handle was never dropped either: with its open proofs
    // gone, only the resident handle can still answer.
    remove_semantic_open_proofs(&semantic_generation_dir(&rt)?)?;
    let after_semantic = ids(&rt.query_semantic(QUERY, TOP_K, None))?;
    if after_semantic != first_semantic {
        return Err(format!(
            "the semantic handle was dropped by a refused publish: first={first_semantic:?} after={after_semantic:?}"
        )
        .into());
    }
    Ok(())
}

/// The registry counters of the scrape, by name.
fn registry_counters(rt: &mut E2eRuntime) -> Result<BTreeMap<String, u64>, Box<dyn Error>> {
    Ok(rt
        .metrics_snapshot()?
        .counters
        .iter()
        .filter(|counter| counter.name.starts_with("snapshot_registry_"))
        .map(|counter| (counter.name.clone(), counter.value))
        .collect())
}

fn counter(counters: &BTreeMap<String, u64>, name: &str) -> Result<u64, Box<dyn Error>> {
    counters
        .get(name)
        .copied()
        .ok_or_else(|| format!("counter `{name}` is in the scrape: {counters:?}").into())
}

/// The first query on each track is served from a promoted handle.
///
/// Deletes everything a cold open of the pinned generation would read on
/// both tracks, then proves the first query of each is served without one:
/// it answers, and the registries report no miss and one promotion.
fn assert_first_queries_are_promoted_hits(rt: &mut E2eRuntime) -> TestResult {
    let before = registry_counters(rt)?;
    std::fs::remove_dir_all(lexical_generation_dir(rt)?)?;
    remove_semantic_open_proofs(&semantic_generation_dir(rt)?)?;

    let text = ids(&rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K))?;
    if text.len() != 2 {
        return Err(format!(
            "the promoted lexical handle served {} rows, expected 2",
            text.len()
        )
        .into());
    }
    let semantic = ids(&rt.query_semantic(QUERY, TOP_K, None))?;
    if semantic.is_empty() {
        return Err("the promoted semantic handle served no rows".into());
    }
    let after = registry_counters(rt)?;
    for track in ["lexical", "semantic"] {
        let misses = format!("snapshot_registry_{track}_misses_total");
        if counter(&after, &misses)? != counter(&before, &misses)? {
            return Err(format!(
                "{track}: the first query ran a cold open: {before:?} -> {after:?}"
            )
            .into());
        }
        if counter(
            &after,
            &format!("snapshot_registry_{track}_promotions_total"),
        )? != 1
        {
            return Err(format!("{track}: exactly one promotion is expected: {after:?}").into());
        }
        let hits = format!("snapshot_registry_{track}_hits_total");
        if counter(&after, &hits)? != counter(&before, &hits)?.saturating_add(1) {
            return Err(
                format!("{track}: the first query is one hit: {before:?} -> {after:?}").into(),
            );
        }
    }
    Ok(())
}

/// The first query after activation is served from the handle activation
/// proved: the generation's files are gone before the query and the
/// registry reports a hit, no miss.
#[test]
fn the_first_query_after_activation_is_served_from_the_promoted_handle() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo", "src/needle.rs", "fn needle() { let needle = 1; }")?;
    rt.ingest_text("repo", "src/other.rs", "fn other() { let needle = 2; }")?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    assert_first_queries_are_promoted_hits(&mut rt)
}

/// The first query after a restart is served from the handle boot's
/// rehydrate proved, the same way.
#[test]
fn the_first_query_after_restart_is_served_from_the_promoted_handle() -> TestResult {
    let mut rt = seeded_runtime()?.reopen();
    rt.start()?;
    assert_first_queries_are_promoted_hits(&mut rt)
}
