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
//! The invalidation half is the same trick inverted: publish an auxiliary
//! batch that names the generation, delete the files, and the next query
//! must *fail* — the registry dropped the handle and the cold open ran.
//!
//! Single-flight coalescing is proven in the registry's unit tests; the
//! daemon's UDS front door is still serial (QI-BB-002), so a concurrent
//! front-door probe would prove nothing about the registry.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMetaEntry, RepoMetaIngestBatch, RevisionId, TextQuerySyntax,
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

/// A routed mutation that names the generation drops its residency: after
/// an auxiliary publish, the next query cold-opens and therefore fails on
/// the deleted files. Without invalidation it would have kept serving the
/// pre-mutation handle.
#[test]
fn an_auxiliary_publish_invalidates_the_resident_generation() -> TestResult {
    let mut rt = seeded_runtime()?;
    let first = ids(&rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K))?;
    if first.len() != 2 {
        return Err(format!("fixture served {} rows, expected 2", first.len()).into());
    }
    let generation = pinned_generation(&rt);
    rt.publish_repo_meta_batch(RepoMetaIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation,
        batch_digest: "e2e-snapshot-invalidate".to_string(),
        entries: vec![RepoMetaEntry {
            source_repo_id: RepoId::new("repo"),
            key: "license".to_string(),
            value: "apache-2.0".to_string(),
        }],
    })?;

    // Prove the mutation landed on disk in this generation before deleting.
    let dir = lexical_generation_dir(&rt)?;
    let meta_written = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().contains("repo-meta"));
    if !meta_written {
        return Err(format!("repo-meta snapshot was not written into {}", dir.display()).into());
    }
    std::fs::remove_dir_all(&dir)?;

    let after = rt.query_text(TextQuerySyntax::Native, QUERY, TOP_K);
    match after.typed_error {
        Some(_) => Ok(()),
        None => Err(format!(
            "query after an auxiliary publish was served from a stale resident handle: {:?}",
            after.candidate_ids
        )
        .into()),
    }
}
