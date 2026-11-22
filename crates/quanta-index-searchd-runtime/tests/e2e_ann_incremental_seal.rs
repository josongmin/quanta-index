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

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, PlannerStage, QueryConstraintSetV1, RepoId,
    RepoRelativePath, RevisionId, SemanticQueryRequest, SemanticQueryResponse,
    SemanticWorkBoundedQueryRequestV1,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, SearchdBinaryProcess, legacy_source_delta_from_chunks,
    legacy_source_replacement_from_chunks, publish_legacy_source_batch, request_when_ready,
};
use quanta_index_searchd_harness::private_tempdir;
use quanta_index_searchd_harness::scale_runtime_policy::DEFAULT_HISTORY_MAX_GENERATIONS;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// Rows in the base generation: past the index floor of 256.
const BASE_FILES: u32 = 288;
/// Rows the delta adds: 32 of 288 is 111 per mille, inside the quarter.
const DELTA_FILES: u32 = 32;
// The old harness used request IDs for chunk IDs. Between the two waves it
// sent the Source publication, head read, activation CAS and semantic query.
const FIRST_DELTA_CHUNK_ID: u32 = BASE_FILES + 5;
const TOP_K: u32 = 10;

/// One file's text: five tokens no other file shares, so the daemon's
/// token-hashing embedder places every file in its own direction and the
/// file's own text is unambiguously its nearest neighbour.
fn file_content(generation: u64, index: u32) -> String {
    format!(
        "fn qa{generation}x{index}() {{ let qb{generation}x{index} = qc{generation}x{index}(qd{generation}x{index}, qe{generation}x{index}); }}"
    )
}

fn fixture_chunks(
    generation: u64,
    first_chunk_id: u32,
    count: u32,
) -> TestResult<Vec<ChunkRecord>> {
    let language =
        LanguageCode::new("rust").map_err(|error| format!("fixture language: {error}"))?;
    (0..count)
        .map(|index| {
            let path = format!("src/g{generation}/item_{index:03}.rs");
            let text = file_content(generation, index);
            Ok(ChunkRecord {
                chunk_id: ChunkId::new(format!("e2e-{}-{path}", first_chunk_id + index)),
                repo_relative_path: RepoRelativePath::new(path),
                language: language.clone(),
                start_byte: 0,
                end_byte: u32::try_from(text.len())?,
                start_line: 1,
                end_line: 1,
                text: text.into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            })
        })
        .collect()
}

fn semantic_generation_dir(root: &Path, pin: &GenerationPin) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&pin.repo_id, &pin.revision_id)
        .generation_dir(&root.join("indexes/semantic"), pin.manifest_generation)
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

fn semantic_request(pin: &GenerationPin, index: u32) -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: file_content(pin.manifest_generation.get(), index),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin.clone()),
        generation_selector: None,
        lexical_scope: None,
        top_k: TOP_K,
    }
}

/// The plan-stage trace entry naming the dense lane, if any.
fn dense_lane_trace(result: &SemanticQueryResponse) -> Option<&str> {
    result
        .explanation
        .planner_trace
        .iter()
        .find(|entry| entry.stage == PlannerStage::Plan && entry.detail.starts_with("dense.index="))
        .map(|entry| entry.detail.as_str())
}

#[test]
#[expect(
    clippy::print_stderr,
    reason = "preserve the scenario error while reporting a separate cleanup failure"
)]
fn a_delta_seal_appends_to_the_inherited_ann_index_and_the_trace_says_so() -> TestResult {
    let root = private_tempdir()?;
    let state_root = std::fs::canonicalize(root.path())?;
    let binary = LegacySearchdBinary::from_environment()?;
    let process = SearchdBinaryProcess::start_with_pinned_command(
        &state_root,
        binary.benchmark_command(&state_root, DEFAULT_HISTORY_MAX_GENERATIONS)?,
    )?;
    let scenario = process
        .connect()
        .and_then(|client| verify_append_case(&client, &state_root));
    let cleanup = process.stop();
    if let Err(primary) = scenario {
        if let Err(secondary) = cleanup {
            eprintln!("ANN append cleanup also failed: {secondary:#}");
        }
        return Err(primary);
    }
    cleanup?;
    binary.verify()
}

#[expect(
    clippy::print_stdout,
    reason = "the independently counted ANN append evidence belongs in the test output"
)]
fn verify_append_case(client: &QuantaIndex, state_root: &Path) -> TestResult {
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    // g1: past the floor, so the seal trains the policy index.
    let base_batch = legacy_source_replacement_from_chunks(
        &repo,
        &revision,
        1,
        fixture_chunks(1, 1, BASE_FILES)?,
    )?;
    let (base_head, _) = publish_legacy_source_batch(client, &base_batch, None)?;
    let base = base_head.generation.semantic.manifest_generation;
    let base_pin = GenerationPin::new(repo.clone(), revision.clone(), base);
    let base_query = request_when_ready(|| {
        client
            .semantic()
            .query_request(semantic_request(&base_pin, 17))
    })?;
    let base_trace =
        dense_lane_trace(&base_query).ok_or("the base query has no dense-lane trace")?;
    // The trained segment carries the policy's recipe and no segment was
    // appended, and the trace says so (QI-BB-027).
    let expected_base_trace = format!(
        "ann.trained_at=g{}; ann.appended=0/{BASE_FILES}; ann.deleted=0; ann.hnsw_m=20; ann.hnsw_ef_construction=300; ann.appended_segments=0; ann.appended_segments_m/ef_construction=none",
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
    let base_index_bytes = index_bytes_by_inode(&semantic_generation_dir(state_root, &base_pin))?;
    let base_bytes: u64 = base_index_bytes.values().sum();
    if base_bytes == 0 {
        return Err("the base seal wrote no index files; the fixture is below the floor".into());
    }

    // g2: a delta inside the append budget.
    let probe = 9_u32;
    let delta_chunks = fixture_chunks(2, FIRST_DELTA_CHUNK_ID, DELTA_FILES)?;
    let probe_id = delta_chunks
        .get(usize::try_from(probe)?)
        .ok_or("delta fixture returned too few chunks")?
        .chunk_id
        .clone();
    let delta_batch = legacy_source_delta_from_chunks(&repo, &revision, 2, &[], &[], delta_chunks)?;
    let (delta_head, _) = publish_legacy_source_batch(client, &delta_batch, Some(base_head))?;
    let delta = delta_head.generation.semantic.manifest_generation;
    let delta_pin = GenerationPin::new(repo, revision, delta);
    if delta.get() != base.get().saturating_add(1) {
        return Err(format!("the delta must follow the base: {base:?} -> {delta:?}").into());
    }

    // A query pinned to the delta serves a delta-only row first, through
    // an index the trace reports as appended to the base's train.
    let delta_query = request_when_ready(|| {
        client
            .semantic()
            .query_request(semantic_request(&delta_pin, probe))
    })?;
    let top = delta_query.results.first();
    if top.map(|candidate| candidate.candidate_id.as_str()) != Some(probe_id.as_str())
        || top.is_none_or(|candidate| (candidate.score - 1.0).abs() > 1e-5)
    {
        let ranked: Vec<(String, f32)> = delta_query
            .results
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
    // The appended segment was built by the library's incremental builder
    // under its own parameters (m=20, ef_construction=150), never the
    // trained recipe; the attestation names them (QI-BB-027).
    let expected_delta_trace = format!(
        "ann.trained_at=g{}; ann.appended={DELTA_FILES}/{BASE_FILES}; ann.deleted=0; ann.hnsw_m=20; ann.hnsw_ef_construction=300; ann.appended_segments=1; ann.appended_segments_m/ef_construction=20/150",
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

    // An explicit bounded exact scan must not claim that this ANN generation
    // sealed an exact index, or mutate the next ordinary query's lane.
    let bounded = request_when_ready(|| {
        client
            .semantic()
            .query_work_bounded_v1(SemanticWorkBoundedQueryRequestV1 {
                query: semantic_request(&delta_pin, probe),
                max_work_units: 1_000_000,
                canonical_ties: false,
            })
    })?;
    if !bounded.query.explanation.planner_trace.iter().any(|entry| {
        entry.detail == "dense.index=exact; dense.attestation=exact_scan_of_sealed_rows"
    }) || bounded
        .query
        .results
        .first()
        .map(|row| row.candidate_id.as_str())
        != Some(probe_id.as_str())
    {
        return Err(
            format!("exact bypass misreported its sealed ANN authority: {bounded:?}").into(),
        );
    }
    let ordinary_after = request_when_ready(|| {
        client
            .semantic()
            .query_request(semantic_request(&delta_pin, probe))
    })?;
    if dense_lane_trace(&ordinary_after) != Some(delta_trace) {
        return Err("bounded exact scan changed the next ordinary ANN contract".into());
    }

    // Cost oracle: the delta shares every base index file by inode and
    // its own new index bytes are a fraction of the base's.
    let delta_index_bytes = index_bytes_by_inode(&semantic_generation_dir(state_root, &delta_pin))?;
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
