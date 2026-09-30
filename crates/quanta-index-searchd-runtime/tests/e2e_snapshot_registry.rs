//! QI-BB-001 / QI-BB-017 — opened sealed generations stay resident across
//! queries while durable authority and physical identity remain valid.
//!
//! A repeated query over an intact generation must hit the resident handle.
//! Physical identity loss is a separate case: maintenance evicts both
//! resident handles and queries must not serve their stale contents.
//!
//! An auxiliary batch naming a sealed generation is refused typed (QI-BB-030).
//! With the physical generation still intact, it must not drop residency.
//!
//! Activation and runtime restart promote the handles they proved (QI-BB-017 #4):
//! the first query serves with no additional registry miss.
//!
//! Single-flight coalescing, fenced retirement and budgeted waits are
//! proven in the registry's unit tests with barrier-controlled openers.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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

/// A second lexical query over the same intact generation is a registry hit.
#[test]
fn second_lexical_query_reuses_resident_handle() -> TestResult {
    let mut rt = seeded_runtime()?;
    let first = ids(&rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K))?;
    if first.len() != 2 {
        return Err(format!("fixture served {} rows, expected 2", first.len()).into());
    }

    let before = registry_counters(&mut rt)?;
    let second = ids(&rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K))?;
    if second != first {
        return Err(format!(
            "resident lexical handle answered differently: first={first:?} second={second:?}"
        )
        .into());
    }
    assert_registry_hit(&before, &registry_counters(&mut rt)?, "lexical")?;
    Ok(())
}

/// A second semantic query over the same intact generation is a registry hit.
#[test]
fn second_semantic_query_reuses_resident_handle() -> TestResult {
    let mut rt = seeded_runtime()?;
    let first = ids(&rt.query_semantic(QUERY, TOP_K, None))?;
    if first.is_empty() {
        return Err("fixture served no semantic rows".into());
    }

    let before = registry_counters(&mut rt)?;
    let second = ids(&rt.query_semantic(QUERY, TOP_K, None))?;
    if second != first {
        return Err(format!(
            "resident semantic handle answered differently: first={first:?} second={second:?}"
        )
        .into());
    }
    assert_registry_hit(&before, &registry_counters(&mut rt)?, "semantic")?;
    Ok(())
}

/// An auxiliary publish that names a sealed generation is refused typed
/// and leaves its residency alone (QI-BB-030): nothing may land in a
/// sealed generation, so there is no mutation to invalidate for.
///
/// The refusal is proved from the outside: the publish answers
/// `GENERATION_IMMUTABLE`, no overlay file appears on disk, and the next
/// query is a resident hit while the physical generation remains intact.
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
    // A well-formed publish with its canonical digest: the refusal under
    // test is the sealed generation's, not the digest gate's.
    let response = rt.ingest_once(e2e_harness::stamped_ingest_request(
        SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(RepoMetaIngestBatch {
            repo_id: rt.repo(),
            revision_id: rt.revision(),
            generation,
            batch_digest: "e2e-snapshot-invalidate".to_string(),
            entries: vec![RepoMetaEntry {
                source_repo_id: RepoId::new("repo")
                    .expect("static fixture ID satisfies canonical policy"),
                key: "license".to_string(),
                value: "apache-2.0".to_string(),
            }],
        }),
    )?)?;
    let SearchPlaneIngestIpcResponse::Error(error) = response else {
        return Err(format!(
            "an auxiliary publish into a sealed generation must be refused typed GENERATION_IMMUTABLE, got {response:?}"
        )
        .into());
    };
    if error.code.as_wire_str() != "GENERATION_IMMUTABLE" {
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
    let before = registry_counters(&mut rt)?;
    let after = rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K);
    if let Some(error) = after.typed_error {
        return Err(format!(
            "a refused publish was followed by a lexical query refusal: {}: {}",
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
    let after_semantic = ids(&rt.query_semantic(QUERY, TOP_K, None))?;
    if after_semantic != first_semantic {
        return Err(format!(
            "the semantic handle was dropped by a refused publish: first={first_semantic:?} after={after_semantic:?}"
        )
        .into());
    }
    let after_counters = registry_counters(&mut rt)?;
    for track in ["lexical", "semantic"] {
        assert_registry_hit(&before, &after_counters, track)?;
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

fn assert_registry_hit(
    before: &BTreeMap<String, u64>,
    after: &BTreeMap<String, u64>,
    track: &str,
) -> TestResult {
    let hits = format!("snapshot_registry_{track}_hits_total");
    let misses = format!("snapshot_registry_{track}_misses_total");
    if counter(after, &hits)? != counter(before, &hits)?.saturating_add(1)
        || counter(after, &misses)? != counter(before, &misses)?
    {
        return Err(format!(
            "{track}: expected one resident hit without a cold open: {before:?} -> {after:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn missing_physical_seals_evict_resident_handles_before_the_next_query() -> TestResult {
    let mut rt = seeded_runtime()?;
    let _lexical = ids(&rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K))?;
    let _semantic = ids(&rt.query_semantic(QUERY, TOP_K, None))?;
    let before = registry_counters(&mut rt)?;
    std::fs::remove_dir_all(lexical_generation_dir(&rt)?)?;
    remove_semantic_open_proofs(&semantic_generation_dir(&rt)?)?;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = registry_counters(&mut rt)?;
        let invalidated = ["lexical", "semantic"].into_iter().all(|track| {
            let name = format!("snapshot_registry_{track}_inventory_invalidations_total");
            matches!((counter(&now, &name), counter(&before, &name)), (Ok(current), Ok(prior)) if current > prior)
        });
        if invalidated {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "physical loss did not invalidate both residents: {before:?} -> {now:?}"
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let lexical = rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K);
    let semantic = rt.query_semantic(QUERY, TOP_K, None);
    if lexical.typed_error.is_none() || semantic.typed_error.is_none() {
        return Err(format!(
            "a physically missing generation was served: lexical_error={:?} lexical_rows={:?} semantic_error={:?} semantic_rows={:?}",
            lexical.typed_error, lexical.candidate_ids, semantic.typed_error, semantic.candidate_ids,
        )
        .into());
    }
    Ok(())
}

/// The first query on each track is served from a promoted handle.
///
/// Both tracks answer while their registries report a hit, no miss and one
/// promotion. Physical-loss invalidation is proved separately above.
fn assert_first_queries_are_promoted_hits(rt: &mut E2eRuntime) -> TestResult {
    let before = registry_counters(rt)?;
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
