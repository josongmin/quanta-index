//! QI-BB-027 W3 — a delta seal appends to the inherited ANN index instead
//! of retraining it, and says so on the query's explanation.
//!
//! Through the daemon's own front door: a first generation with more rows
//! than the index floor trains the policy index; a delta generation inside
//! the append budget assigns its rows to the inherited centroids as one
//! more segment. A semantic query pinned to the delta serves a delta-only
//! row through that index, and its planner trace names the training
//! generation and the appended share.
//!
//! Oracles are outside the adapter's own claims: the explanation trace as
//! the query returns it, the candidate the query ranks first, and a
//! hard-link-aware walk of both generations' index directories on disk —
//! the bytes the delta wrote under `_indices/` that share no inode with the
//! base must be a fraction of the base's, or the append rewrote the base.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use quanta_index_contract::{ManifestGeneration, PlannerStage};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_searchd_harness as e2e_harness;

use e2e_harness::{E2eQueryResult, E2eRuntime, E2eTextChunkSpec};

type TestResult = Result<(), Box<dyn Error>>;

/// Rows in the base generation: past the index floor of 256.
const BASE_FILES: u32 = 288;
/// Rows the delta adds: 32 of 288 is 111 per mille, inside the quarter.
const DELTA_FILES: u32 = 32;
/// Files per ingest batch: one batch is one ingest IPC round trip, and the
/// daemon's ingest socket answers within its I/O timeout only for batches
/// of this order on a debug build.
const FILES_PER_BATCH: u32 = 32;
const TOP_K: u32 = 10;

/// One file's text: five tokens no other file shares, so the daemon's
/// token-hashing embedder places every file in its own direction and the
/// file's own text is unambiguously its nearest neighbour.
fn file_content(generation: u64, index: u32) -> String {
    format!(
        "fn qa{generation}x{index}() {{ let qb{generation}x{index} = qc{generation}x{index}(qd{generation}x{index}, qe{generation}x{index}); }}"
    )
}

/// Ingest `count` files into the current generation, [`FILES_PER_BATCH`]
/// per batch, and return their candidate ids in ingest order.
fn ingest_files(
    rt: &mut E2eRuntime,
    generation: u64,
    count: u32,
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut ids = Vec::with_capacity(usize::try_from(count)?);
    for first in (0..count).step_by(usize::try_from(FILES_PER_BATCH)?) {
        let last = first.saturating_add(FILES_PER_BATCH).min(count);
        let contents: Vec<(String, String)> = (first..last)
            .map(|index| {
                (
                    format!("src/g{generation}/item_{index:03}.rs"),
                    file_content(generation, index),
                )
            })
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
        ids.extend(rt.ingest_text_files_one_batch(&files)?);
    }
    Ok(ids)
}

fn semantic_generation_dir(
    rt: &E2eRuntime,
    generation: ManifestGeneration,
) -> Result<PathBuf, Box<dyn Error>> {
    let semantic_root = std::fs::canonicalize(rt.state_root())?
        .join("indexes")
        .join("semantic");
    Ok(
        GenerationStorageKeyV1::for_repo_revision(&rt.repo(), &rt.revision())
            .generation_dir(&semantic_root, generation),
    )
}

/// Bytes of every index file below the generation's `dataset/` tree, keyed
/// by inode.
///
/// The semantic table keeps its index files under
/// `dataset/semantic.lance/_indices/`; keying by inode counts a file the
/// delta inherited by hard link once, and against the base.
fn index_bytes_by_inode(generation_dir: &Path) -> Result<BTreeMap<u64, u64>, Box<dyn Error>> {
    let mut bytes = BTreeMap::new();
    let mut pending = vec![generation_dir.join("dataset")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file()
                && entry
                    .path()
                    .components()
                    .any(|component| component.as_os_str() == "_indices")
            {
                let _prior = bytes.insert(metadata.ino(), metadata.len());
            }
        }
    }
    Ok(bytes)
}

fn served(result: &E2eQueryResult, what: &str) -> Result<(), Box<dyn Error>> {
    result.typed_error.as_ref().map_or_else(
        || Ok(()),
        |error| Err(format!("{what} was refused: {error}").into()),
    )
}

/// The plan-stage trace entry naming the dense lane, if any.
fn dense_lane_trace(result: &E2eQueryResult) -> Option<String> {
    result.explanation.as_ref().and_then(|explanation| {
        explanation
            .planner_trace
            .iter()
            .find(|entry| {
                entry.stage == PlannerStage::Plan && entry.detail.starts_with("dense.index=")
            })
            .map(|entry| entry.detail.clone())
    })
}

#[test]
#[expect(
    clippy::print_stdout,
    reason = "the QI-BB-027-APPEND-EVIDENCE line is the measurement the ledger cites; it must land in the run log"
)]
fn a_delta_seal_appends_to_the_inherited_ann_index_and_the_trace_says_so() -> TestResult {
    let mut rt = E2eRuntime::boot()?;

    // g1: past the floor, so the seal trains the policy index.
    let base_ids = ingest_files(&mut rt, 1, BASE_FILES)?;
    if base_ids.len() != usize::try_from(BASE_FILES)? {
        return Err(format!(
            "base ingest returned {} ids for {BASE_FILES} files",
            base_ids.len()
        )
        .into());
    }
    let base = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let base_query = rt.query_semantic(&file_content(1, 17), TOP_K, None);
    served(&base_query, "semantic query pinned to the base")?;
    let base_trace =
        dense_lane_trace(&base_query).ok_or("the base query has no dense-lane trace")?;
    let expected_base_trace = format!(
        "ann.trained_at=g{}; ann.appended=0/{BASE_FILES}; ann.deleted=0",
        base.get()
    );
    if !base_trace.starts_with("dense.index=ivf_hnsw_sq; dense.attestation=sealed;")
        || !base_trace.ends_with(&expected_base_trace)
    {
        return Err(format!(
            "the base must serve a freshly trained sealed index, trace: {base_trace}"
        )
        .into());
    }
    let base_index_bytes = index_bytes_by_inode(&semantic_generation_dir(&rt, base)?)?;
    let base_bytes: u64 = base_index_bytes.values().sum();
    if base_bytes == 0 {
        return Err("the base seal wrote no index files; the fixture is below the floor".into());
    }

    // g2: a delta inside the append budget.
    let delta_ids = ingest_files(&mut rt, 2, DELTA_FILES)?;
    let delta = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    if delta.get() != base.get().saturating_add(1) {
        return Err(format!("the delta must follow the base: {base:?} -> {delta:?}").into());
    }

    // A query pinned to the delta serves a delta-only row first, through
    // an index the trace reports as appended to the base's train.
    let probe = 9_u32;
    let probe_id = delta_ids
        .get(usize::try_from(probe)?)
        .ok_or("delta ingest returned too few ids")?;
    let delta_query = rt.query_semantic(&file_content(2, probe), TOP_K, None);
    served(&delta_query, "semantic query pinned to the delta")?;
    let top = delta_query.candidates.first();
    if top.map(|candidate| candidate.candidate_id.as_str()) != Some(probe_id.as_str())
        || top.is_none_or(|candidate| (candidate.score - 1.0).abs() > 1e-5)
    {
        let ranked: Vec<(String, f32)> = delta_query
            .candidates
            .iter()
            .map(|candidate| (candidate.candidate_id.clone(), candidate.score))
            .collect();
        return Err(format!(
            "a delta-only row must rank first at cosine 1 for its own text through the appended index; top-{TOP_K}: {ranked:?}"
        )
        .into());
    }
    let delta_trace =
        dense_lane_trace(&delta_query).ok_or("the delta query has no dense-lane trace")?;
    let expected_delta_trace = format!(
        "ann.trained_at=g{}; ann.appended={DELTA_FILES}/{BASE_FILES}; ann.deleted=0",
        base.get()
    );
    if !delta_trace.starts_with("dense.index=ivf_hnsw_sq; dense.attestation=sealed;")
        || !delta_trace.ends_with(&expected_delta_trace)
    {
        return Err(format!(
            "the delta must serve the base's train with {DELTA_FILES} rows appended, trace: {delta_trace}"
        )
        .into());
    }

    // Cost oracle: the delta shares every base index file by inode and
    // its own new index bytes are a fraction of the base's.
    let delta_index_bytes = index_bytes_by_inode(&semantic_generation_dir(&rt, delta)?)?;
    let shared_inodes = delta_index_bytes
        .keys()
        .filter(|inode| base_index_bytes.contains_key(*inode))
        .count();
    let delta_new_bytes: u64 = delta_index_bytes
        .iter()
        .filter(|(inode, _)| !base_index_bytes.contains_key(*inode))
        .map(|(_, len)| *len)
        .sum();
    println!(
        "QI-BB-027-APPEND-EVIDENCE base_index_bytes={base_bytes} delta_new_index_bytes={delta_new_bytes} appended_rows={DELTA_FILES} base_rows={BASE_FILES} shared_index_inodes={shared_inodes}"
    );
    if shared_inodes != base_index_bytes.len() {
        return Err(format!(
            "the delta must inherit every base index file by hard link, shares {shared_inodes} of {}",
            base_index_bytes.len()
        )
        .into());
    }
    if delta_new_bytes == 0 {
        return Err("the append must write its own segment".into());
    }
    if delta_new_bytes.saturating_mul(2) >= base_bytes {
        return Err(format!(
            "an append of {DELTA_FILES} rows wrote {delta_new_bytes} new index bytes against a {base_bytes}-byte base index; it rewrote the base"
        )
        .into());
    }
    Ok(())
}
