//! QI-BB-027 — the dense lane's index is part of the sealed contract.
//!
//! A seal records which index it built, with every parameter and what the
//! library reported; the open verifies the dataset against that record; a
//! query runs with exactly the sealed effort. The oracles here are
//! independent of the adapter's own claims: the library's index listing and
//! statistics read straight from the sealed dataset, an exhaustive exact
//! cosine ranking computed in this file, fault injection on the index files,
//! and manifests of earlier formats rewritten byte for byte, which every
//! door must refuse typed rather than serve on what they happen to carry.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use quanta_index_contract::{
    BatchIngestMode, EmbeddingRecord, GenerationSnapshot, ManifestGeneration, OwnerDocKind, RepoId,
    RevisionId, SearchPlaneTrackKind, SemanticCorpusKindV1, SemanticReplaceScope,
    SemanticSourceScopeKeyV1, SemanticTombstoneScope,
};
use quanta_index_core::{
    CoreError, DenseIndexBuildV1, DenseIndexEffortV1, DenseIndexSegmentBuildV1,
    DenseIndexTrainingV1, DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1,
    GenerationIdentityValidatePort, GenerationQuarantineReasonV1, GenerationStorageKeyV1,
    IntegrityScrubBudgetV1, IntegrityScrubOutcomeV1, IntegrityScrubPort,
    QuarantineDiscardOutcomeV1, QuarantinedGenerationDiscardPort, RequestBudgetV1,
    SemanticIndexOpenPort,
};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, inventory_persisted_generations,
    legacy_chunk_embedding_record_v1, sealed_replace_batch_v1, search_scope_v1, tombstone_scope_v1,
};
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

const DIMENSION: usize = 16;
const INDEX_NAME: &str = "vector_ivf_hnsw_sq";
const SCOPE_MANIFEST: &str = "semantic-manifest.cbor";
const SEALED_MANIFEST: &str = "semantic-sealed-manifest.cbor";

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn repo() -> RepoId {
    RepoId::new("ann-contract-repo").expect("static fixture ID satisfies canonical policy")
}

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn revision() -> RevisionId {
    RevisionId::new("ann-contract-rev").expect("static fixture ID satisfies canonical policy")
}

fn dimension_u32() -> Result<u32, Box<dyn Error>> {
    Ok(u32::try_from(DIMENSION)?)
}

/// A deterministic direction for `seed`, unit-normalized so the stored row
/// equals it up to float rounding and this file can rank exactly.
fn unit_vector(seed: u64, dimension: usize) -> Vec<f32> {
    let mut state = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xD1B5_4A32_D192_ED03);
    let mut vector = Vec::with_capacity(dimension);
    for _ in 0..dimension {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let lane = u16::try_from(state & 0xFFFF).map_or(0.0_f32, f32::from);
        vector.push(lane / 32768.0_f32 - 1.0);
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    vector.iter().map(|value| value / norm).collect()
}

fn record(id: &str, path: &str, seed: u64) -> Result<EmbeddingRecord, Box<dyn Error>> {
    legacy_chunk_embedding_record_v1(id, path, unit_vector(seed, DIMENSION))
        .map_err(|err| -> Box<dyn Error> { err.into() })
}

fn records(
    prefix: &str,
    path: &str,
    seeds: std::ops::Range<u64>,
) -> Result<Vec<EmbeddingRecord>, Box<dyn Error>> {
    seeds
        .map(|seed| record(&format!("{prefix}-{seed}"), path, seed))
        .collect()
}

fn scope(path: &str, embeddings: Vec<EmbeddingRecord>) -> SemanticReplaceScope {
    SemanticReplaceScope {
        scope: search_scope_v1(path),
        scope_digest: format!("scope:{path}"),
        embeddings,
        cluster_memberships: Vec::new(),
    }
}

/// Seal `generation` with the given scopes; `base` makes it a delta.
fn seal_with_scopes(
    adapter: &SemanticAdapter,
    generation: ManifestGeneration,
    base: Option<ManifestGeneration>,
    scopes: Vec<SemanticReplaceScope>,
    tombstones: &[SemanticTombstoneScope],
) -> TestResult {
    let mut batch = sealed_replace_batch_v1(
        repo(),
        revision(),
        generation,
        "unused",
        Vec::new(),
        dimension_u32()?,
    );
    batch.replace_scopes = scopes;
    batch.tombstone_scopes = tombstones.to_vec();
    if let Some(base) = base {
        batch.base_generation = Some(base);
        batch.mode = BatchIngestMode::Delta;
    }
    build_resident_batch_v1(adapter, &batch)?;
    Ok(())
}

fn seal_rows(adapter: &SemanticAdapter, generation: ManifestGeneration, rows: u64) -> TestResult {
    let path = format!("src/g{}.rs", generation.get());
    seal_with_scopes(
        adapter,
        generation,
        None,
        vec![scope(&path, records("row", &path, 0..rows)?)],
        &[],
    )
}

fn generation_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo(), &revision()).generation_dir(root, generation)
}

fn identity(generation: ManifestGeneration) -> GenerationSnapshot {
    GenerationSnapshot {
        repo_id: repo(),
        revision_id: revision(),
        track: SearchPlaneTrackKind::Semantic,
        manifest_generation: generation,
        manifest_digest: format!("manifest:{}", generation.get()),
    }
}

fn typed_code<T>(result: &Result<T, CoreError>) -> Option<String> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.to_string()),
        _ => None,
    }
}

/// The policy's graph recipe, as the trained segment carries it.
const HNSW_M: u32 = 20;
const HNSW_EF_CONSTRUCTION: u32 = 300;
/// What the library's incremental builder builds an appended segment with:
/// its own defaults, which the seal must record verbatim.
const APPENDED_HNSW_M: u32 = 20;
const APPENDED_HNSW_EF_CONSTRUCTION: u32 = 150;

/// The policy's sealed approximate lane with `lineage` behind it and
/// `appended_segments` segments appended by the library's builder.
fn sealed_ann_lane_with(
    lineage: DenseIndexTrainingV1,
    appended_segments: usize,
) -> DenseLaneContractV1 {
    DenseLaneContractV1 {
        index: DenseIndexV1::Approximate {
            effort: DenseIndexEffortV1 {
                index_kind: "ivf_hnsw_sq".to_string(),
                partitions: 1,
                nprobes: 1,
                ef_floor: 64,
                ef_per_candidate: 2,
                refine_factor: 2,
            },
            lineage,
            build: DenseIndexBuildV1 {
                hnsw_m: HNSW_M,
                hnsw_ef_construction: HNSW_EF_CONSTRUCTION,
                appended_segments: vec![
                    DenseIndexSegmentBuildV1 {
                        hnsw_m: APPENDED_HNSW_M,
                        hnsw_ef_construction: APPENDED_HNSW_EF_CONSTRUCTION,
                    };
                    appended_segments
                ],
            },
        },
        attestation: DenseLaneAttestationV1::Sealed,
    }
}

/// The policy's sealed approximate lane as one trained segment.
fn sealed_ann_lane(lineage: DenseIndexTrainingV1) -> DenseLaneContractV1 {
    sealed_ann_lane_with(lineage, 0)
}

/// The lineage a seal that trained at `generation` over `rows` rows
/// records.
fn trained_at(generation: u64, rows: u64) -> DenseIndexTrainingV1 {
    DenseIndexTrainingV1 {
        trained_at_generation: generation,
        trained_rows: rows,
        appended_rows: 0,
        deleted_rows: 0,
    }
}

fn sealed_exact_lane() -> DenseLaneContractV1 {
    DenseLaneContractV1 {
        index: DenseIndexV1::Exact,
        attestation: DenseLaneAttestationV1::Sealed,
    }
}

/// What the library itself says about the sealed dataset's vector indices.
struct LibraryView {
    /// Distinct index names; the library lists one entry per segment.
    names: Vec<String>,
    stats: Option<(usize, usize, Option<u32>)>,
}

/// Read the index listing and statistics straight from the sealed dataset,
/// bypassing the adapter: the oracle for what the seal recorded.
#[expect(
    clippy::disallowed_methods,
    reason = "test-only direct library inspection of the sealed dataset; the sync seam is the test's own runtime"
)]
fn library_view(generation_dir: &Path) -> Result<LibraryView, Box<dyn Error>> {
    let uri = generation_dir
        .join("dataset")
        .to_string_lossy()
        .into_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let connection = lancedb::connect(&uri)
            .execute()
            .await
            .map_err(|err| format!("connect: {err}"))?;
        let table = connection
            .open_table("semantic")
            .execute()
            .await
            .map_err(|err| format!("open_table: {err}"))?;
        let configs = table
            .list_indices()
            .await
            .map_err(|err| format!("list_indices: {err}"))?;
        let mut names: Vec<String> = Vec::new();
        for config in configs
            .iter()
            .filter(|config| config.columns.iter().any(|column| column == "vector"))
        {
            let name = if config.index_type == lancedb::index::IndexType::IvfHnswSq {
                config.name.clone()
            } else {
                format!("{}:{}", config.name, config.index_type)
            };
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let stats = match table
            .index_stats(INDEX_NAME)
            .await
            .map_err(|err| format!("index_stats: {err}"))?
        {
            Some(stats) => Some((
                stats.num_indexed_rows,
                stats.num_unindexed_rows,
                stats.num_indices,
            )),
            None => None,
        };
        Ok::<LibraryView, Box<dyn Error>>(LibraryView { names, stats })
    })
}

fn index_files(generation_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    let mut pending = vec![generation_dir.join("dataset")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else if entry
                .path()
                .components()
                .any(|component| component.as_os_str() == "_indices")
            {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

#[test]
fn the_seal_and_the_dataset_agree_at_the_255_256_boundary() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let below = ManifestGeneration::new(1);
    let at = ManifestGeneration::new(2);
    seal_rows(&adapter, below, 255)?;
    seal_rows(&adapter, at, 256)?;

    let below_lane = adapter.open(&repo(), &revision(), below)?.dense_lane();
    if below_lane != sealed_exact_lane() {
        return Err(format!("255 rows must seal an exact lane, got {below_lane:?}").into());
    }
    let below_view = library_view(&generation_dir(temp.path(), below))?;
    if !below_view.names.is_empty() || below_view.stats.is_some() {
        return Err(format!(
            "an exact seal must leave no vector index, the library lists {:?}",
            below_view.names
        )
        .into());
    }
    if !index_files(&generation_dir(temp.path(), below))?.is_empty() {
        return Err("an exact seal must write no index files".into());
    }

    let at_searcher = adapter.open(&repo(), &revision(), at)?;
    let at_lane = at_searcher.dense_lane();
    if at_lane != sealed_ann_lane(trained_at(2, 256)) {
        return Err(format!("256 rows must seal the policy index, got {at_lane:?}").into());
    }
    let at_view = library_view(&generation_dir(temp.path(), at))?;
    if at_view.names != vec![INDEX_NAME.to_string()] {
        return Err(format!(
            "the library must list exactly the sealed index, lists {:?}",
            at_view.names
        )
        .into());
    }
    if at_view.stats != Some((256, 0, Some(1))) {
        return Err(format!(
            "the sealed index must cover every row in one segment, stats {:?}",
            at_view.stats
        )
        .into());
    }
    // The lane serves exact scores through the index: the self-vector is the
    // top hit at cosine 1, which the refine step guarantees even though the
    // index scores quantized vectors.
    let hits = at_searcher.search(&unit_vector(7, DIMENSION), 3, &RequestBudgetV1::unbounded())?;
    let Some(top) = hits.first() else {
        return Err("the indexed lane must serve hits".into());
    };
    if top.candidate_id != "row-7" || (top.score - 1.0).abs() > 1e-5 {
        return Err(format!(
            "the self-vector must rank first at cosine 1 through the index, got {top:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn a_delta_that_shrinks_below_the_floor_drops_the_inherited_index() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let delta = ManifestGeneration::new(2);
    seal_with_scopes(
        &adapter,
        base,
        None,
        vec![
            scope("src/keep.rs", records("keep", "src/keep.rs", 0..200)?),
            scope("src/drop.rs", records("drop", "src/drop.rs", 200..300)?),
        ],
        &[],
    )?;
    // 300 rows sealed an index; the delta deletes the 100 drop owners and holds 200.
    let tombstones = (200..300)
        .map(|seed| {
            tombstone_scope_v1(SemanticSourceScopeKeyV1 {
                corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                owner_kind: OwnerDocKind::Chunk,
                owner_id: format!("owner-drop-{seed}"),
            })
        })
        .collect::<Vec<_>>();
    seal_with_scopes(&adapter, delta, Some(base), Vec::new(), &tombstones)?;

    let delta_lane = adapter.open(&repo(), &revision(), delta)?.dense_lane();
    if delta_lane != sealed_exact_lane() {
        return Err(format!("200 rows must seal an exact lane, got {delta_lane:?}").into());
    }
    let delta_view = library_view(&generation_dir(temp.path(), delta))?;
    if !delta_view.names.is_empty() {
        return Err(format!(
            "the delta must drop the inherited index, the library still lists {:?}",
            delta_view.names
        )
        .into());
    }
    // The base is untouched: it still serves through its own index.
    let base_lane = adapter.open(&repo(), &revision(), base)?.dense_lane();
    if base_lane != sealed_ann_lane(trained_at(1, 300)) {
        return Err(format!("the base must keep its sealed index, got {base_lane:?}").into());
    }
    let base_view = library_view(&generation_dir(temp.path(), base))?;
    if base_view.stats != Some((300, 0, Some(1))) {
        return Err(format!("the base index must be intact, stats {:?}", base_view.stats).into());
    }
    Ok(())
}

/// A delta whose new rows exceed the append budget (100 of 300 is a third,
/// the budget is a quarter) retrains: one index, one segment, trained by
/// this seal over every row.
#[test]
fn a_delta_beyond_the_append_budget_retrains_one_index_covering_every_row() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let delta = ManifestGeneration::new(2);
    seal_with_scopes(
        &adapter,
        base,
        None,
        vec![scope(
            "src/base.rs",
            records("base", "src/base.rs", 0..300)?,
        )],
        &[],
    )?;
    seal_with_scopes(
        &adapter,
        delta,
        Some(base),
        vec![scope(
            "src/new.rs",
            records("new", "src/new.rs", 1_000..1_100)?,
        )],
        &[],
    )?;

    let searcher = adapter.open(&repo(), &revision(), delta)?;
    if searcher.dense_lane() != sealed_ann_lane(trained_at(2, 400)) {
        return Err(format!(
            "400 rows past the budget must retrain the policy index, got {:?}",
            searcher.dense_lane()
        )
        .into());
    }
    let view = library_view(&generation_dir(temp.path(), delta))?;
    if view.names != vec![INDEX_NAME.to_string()] {
        return Err(format!(
            "the delta must carry exactly one vector index, not the base's beside its own: {:?}",
            view.names
        )
        .into());
    }
    if view.stats != Some((400, 0, Some(1))) {
        return Err(format!(
            "the delta index must cover the inherited and the new rows, stats {:?}",
            view.stats
        )
        .into());
    }
    let hits = searcher.search(
        &unit_vector(1_042, DIMENSION),
        3,
        &RequestBudgetV1::unbounded(),
    )?;
    if hits.first().map(|hit| hit.candidate_id.as_str()) != Some("new-1042") {
        return Err(
            format!("a delta-only row must be served through the rebuilt index: {hits:?}").into(),
        );
    }
    Ok(())
}

/// Bytes of every file under a dataset's `_indices/` tree, keyed by inode,
/// so a hard-linked file inherited from the base counts once and only on
/// the base.
fn index_bytes_by_inode(generation_dir: &Path) -> Result<BTreeMap<u64, u64>, Box<dyn Error>> {
    let mut bytes = BTreeMap::new();
    for file in index_files(generation_dir)? {
        let metadata = std::fs::metadata(&file)?;
        let _prior = bytes.insert(metadata.ino(), metadata.len());
    }
    Ok(bytes)
}

/// A delta inside the append budget appends to the inherited index.
///
/// 60 of 300 is a fifth, inside the quarter. The lane reports the base's
/// train with the new rows counted, the library covers every row in one
/// more segment, the new rows are served, the base is untouched, and the
/// bytes the delta wrote under `_indices/` are a fraction of the base's.
#[test]
#[expect(
    clippy::print_stdout,
    reason = "the QI-BB-027-APPEND-EVIDENCE line is the measurement the ledger cites; it must land in the run log"
)]
fn a_delta_inside_the_append_budget_appends_to_the_inherited_index() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let delta = ManifestGeneration::new(2);
    seal_with_scopes(
        &adapter,
        base,
        None,
        vec![scope(
            "src/base.rs",
            records("base", "src/base.rs", 0..300)?,
        )],
        &[],
    )?;
    let base_index_bytes = index_bytes_by_inode(&generation_dir(temp.path(), base))?;
    seal_with_scopes(
        &adapter,
        delta,
        Some(base),
        vec![scope(
            "src/new.rs",
            records("new", "src/new.rs", 1_000..1_060)?,
        )],
        &[],
    )?;

    let searcher = adapter.open(&repo(), &revision(), delta)?;
    // The attestation names the appended segment's actual build parameters
    // (the library's incremental builder's), never the trained recipe.
    let expected = sealed_ann_lane_with(
        DenseIndexTrainingV1 {
            trained_at_generation: 1,
            trained_rows: 300,
            appended_rows: 60,
            deleted_rows: 0,
        },
        1,
    );
    if searcher.dense_lane() != expected {
        return Err(format!(
            "360 rows inside the budget must append to the base's index, got {:?}",
            searcher.dense_lane()
        )
        .into());
    }
    let view = library_view(&generation_dir(temp.path(), delta))?;
    if view.names != vec![INDEX_NAME.to_string()] {
        return Err(format!(
            "the delta must carry exactly one vector index by name: {:?}",
            view.names
        )
        .into());
    }
    if view.stats != Some((360, 0, Some(2))) {
        return Err(format!(
            "the appended index must cover every row in two segments, stats {:?}",
            view.stats
        )
        .into());
    }
    // Both segments serve their own rows at the top with exact cosine
    // scores: the refine step re-ranks candidates from every segment on
    // the original vectors, not on one segment's quantized codes.
    for (query_seed, expected) in [(1_042_u64, "new-1042"), (7_u64, "base-7")] {
        let hits = searcher.search(
            &unit_vector(query_seed, DIMENSION),
            3,
            &RequestBudgetV1::unbounded(),
        )?;
        let Some(top) = hits.first() else {
            return Err(format!("the appended index must serve hits for {expected}").into());
        };
        if top.candidate_id != expected || (top.score - 1.0).abs() > 1e-5 {
            return Err(format!(
                "{expected} must rank first at cosine 1 through the appended index, got {hits:?}"
            )
            .into());
        }
    }

    // The base is untouched and still its own train.
    let base_lane = adapter.open(&repo(), &revision(), base)?.dense_lane();
    if base_lane != sealed_ann_lane(trained_at(1, 300)) {
        return Err(format!("the base must keep its sealed index, got {base_lane:?}").into());
    }
    if library_view(&generation_dir(temp.path(), base))?.stats != Some((300, 0, Some(1))) {
        return Err("the base index must be intact".into());
    }

    // Cost oracle: the delta shares the base's index files by inode and
    // wrote only its own segment, a fraction of the base's index bytes.
    let delta_index_bytes = index_bytes_by_inode(&generation_dir(temp.path(), delta))?;
    let base_bytes: u64 = base_index_bytes.values().sum();
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
        "QI-BB-027-APPEND-EVIDENCE base_index_bytes={base_bytes} delta_new_index_bytes={delta_new_bytes} appended_rows=60 shared_index_inodes={shared_inodes}"
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
            "an append of 60 rows wrote {delta_new_bytes} new index bytes against a {base_bytes}-byte base index; it rewrote the base"
        )
        .into());
    }
    Ok(())
}

/// Removing an appended segment's rows must reseal the surviving source.
///
/// The already sealed base and delta must remain unchanged. The library may
/// retire the now-empty appended segment.
#[test]
fn deleting_every_row_of_an_appended_segment_reseals_the_survivors() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let appended = ManifestGeneration::new(2);
    let deleted = ManifestGeneration::new(3);
    seal_with_scopes(
        &adapter,
        base,
        None,
        vec![scope(
            "src/base.rs",
            records("base", "src/base.rs", 0..300)?,
        )],
        &[],
    )?;
    seal_with_scopes(
        &adapter,
        appended,
        Some(base),
        vec![scope(
            "src/new.rs",
            records("new", "src/new.rs", 1_000..1_060)?,
        )],
        &[],
    )?;
    if library_view(&generation_dir(temp.path(), appended))?.stats != Some((360, 0, Some(2))) {
        return Err("fixture must seal one trained and one appended segment".into());
    }
    let tombstones = (1_000..1_060)
        .map(|seed| {
            tombstone_scope_v1(SemanticSourceScopeKeyV1 {
                corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                owner_kind: OwnerDocKind::Chunk,
                owner_id: format!("owner-new-{seed}"),
            })
        })
        .collect::<Vec<_>>();
    seal_with_scopes(&adapter, deleted, Some(appended), Vec::new(), &tombstones)?;

    let view = library_view(&generation_dir(temp.path(), deleted))?;
    if view.stats != Some((300, 0, Some(1))) {
        return Err(format!(
            "the survivor index must cover 300 rows in one segment, got {:?}",
            view.stats
        )
        .into());
    }
    let served = adapter.open(&repo(), &revision(), deleted)?;
    if served.dense_lane() != sealed_ann_lane(trained_at(3, 300)) {
        return Err(format!(
            "the survivor index must have a fresh seal, got {:?}",
            served.dense_lane()
        )
        .into());
    }
    let hits = served.search(&unit_vector(7, DIMENSION), 3, &RequestBudgetV1::unbounded())?;
    if hits
        .first()
        .is_none_or(|hit| hit.candidate_id != "base-7" || (hit.score - 1.0).abs() > 1e-5)
    {
        return Err(format!("retained self-vector was not served: {hits:?}").into());
    }
    if library_view(&generation_dir(temp.path(), appended))?.stats != Some((360, 0, Some(2))) {
        return Err("sealed two-segment parent changed after successor delete".into());
    }
    if library_view(&generation_dir(temp.path(), base))?.stats != Some((300, 0, Some(1))) {
        return Err("sealed one-segment base changed after successor delete".into());
    }
    Ok(())
}

/// Replacing an appended scope with byte-identical rows is still a
/// physical delete and append. The final canonical row fingerprints match
/// the base, while Lance may retire the emptied appended segment.
#[test]
fn identical_replacement_of_an_appended_segment_can_reseal() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let appended = ManifestGeneration::new(2);
    let replaced = ManifestGeneration::new(3);
    seal_with_scopes(
        &adapter,
        base,
        None,
        vec![scope(
            "src/base.rs",
            records("base", "src/base.rs", 0..300)?,
        )],
        &[],
    )?;
    let same_rows = records("new", "src/new.rs", 1_000..1_060)?;
    seal_with_scopes(
        &adapter,
        appended,
        Some(base),
        vec![scope("src/new.rs", same_rows.clone())],
        &[],
    )?;
    if library_view(&generation_dir(temp.path(), appended))?.stats != Some((360, 0, Some(2))) {
        return Err("fixture must first seal two ANN segments".into());
    }
    seal_with_scopes(
        &adapter,
        replaced,
        Some(appended),
        vec![scope("src/new.rs", same_rows)],
        &[],
    )?;
    let view = library_view(&generation_dir(temp.path(), replaced))?;
    if view.stats != Some((360, 0, Some(1))) {
        return Err(format!(
            "identical replacement did not reindex all live rows: {:?}",
            view.stats
        )
        .into());
    }
    let served = adapter.open(&repo(), &revision(), replaced)?;
    if served.dense_lane() != sealed_ann_lane(trained_at(3, 360)) {
        return Err(format!(
            "identical replacement retained stale lineage: {:?}",
            served.dense_lane()
        )
        .into());
    }
    for (seed, id) in [(7, "base-7"), (1_042, "new-1042")] {
        let hits = served.search(
            &unit_vector(seed, DIMENSION),
            3,
            &RequestBudgetV1::unbounded(),
        )?;
        if hits
            .first()
            .is_none_or(|hit| hit.candidate_id != id || (hit.score - 1.0).abs() > 1e-5)
        {
            return Err(format!("retained {id} was not served: {hits:?}").into());
        }
    }
    if library_view(&generation_dir(temp.path(), appended))?.stats != Some((360, 0, Some(2))) {
        return Err("sealed parent changed during identical replacement".into());
    }
    Ok(())
}

/// Mutation and seal may arrive in separate batches.
///
/// After an appended segment is fully tombstoned and new rows are pending, the later seal must
/// validate the original sealed base and train over all current live rows.
#[test]
fn multibatch_delete_and_append_retrains_after_segment_contraction() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let appended = ManifestGeneration::new(2);
    let successor = ManifestGeneration::new(3);
    seal_with_scopes(
        &adapter,
        base,
        None,
        vec![scope(
            "src/base.rs",
            records("base", "src/base.rs", 0..300)?,
        )],
        &[],
    )?;
    seal_with_scopes(
        &adapter,
        appended,
        Some(base),
        vec![scope(
            "src/new.rs",
            records("new", "src/new.rs", 1_000..1_060)?,
        )],
        &[],
    )?;
    let tombstones = (1_000..1_060)
        .map(|seed| {
            tombstone_scope_v1(SemanticSourceScopeKeyV1 {
                corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                owner_kind: OwnerDocKind::Chunk,
                owner_id: format!("owner-new-{seed}"),
            })
        })
        .collect::<Vec<_>>();
    let mut first = sealed_replace_batch_v1(
        repo(),
        revision(),
        successor,
        "src/mixed.rs",
        records("mixed", "src/mixed.rs", 2_000..2_010)?,
        dimension_u32()?,
    );
    first.base_generation = Some(appended);
    first.mode = BatchIngestMode::Delta;
    first.tombstone_scopes = tombstones;
    first.seal = false;
    build_resident_batch_v1(&adapter, &first)?;
    if library_view(&generation_dir(temp.path(), successor))?.stats != Some((300, 10, Some(1))) {
        return Err(
            "unsealed mixed fixture must have one surviving segment and ten pending rows".into(),
        );
    }
    let mut last = sealed_replace_batch_v1(
        repo(),
        revision(),
        successor,
        "src/seal.rs",
        Vec::new(),
        dimension_u32()?,
    );
    last.base_generation = Some(appended);
    last.mode = BatchIngestMode::Delta;
    last.replace_scopes.clear();
    build_resident_batch_v1(&adapter, &last)?;

    let view = library_view(&generation_dir(temp.path(), successor))?;
    if view.stats != Some((310, 0, Some(1))) {
        return Err(format!(
            "mixed successor did not retrain 310 live rows: {:?}",
            view.stats
        )
        .into());
    }
    let served = adapter.open(&repo(), &revision(), successor)?;
    if served.dense_lane() != sealed_ann_lane(trained_at(3, 310)) {
        return Err(format!(
            "mixed successor retained stale lineage: {:?}",
            served.dense_lane()
        )
        .into());
    }
    for (seed, id) in [(7, "base-7"), (2_005, "mixed-2005")] {
        let hits = served.search(
            &unit_vector(seed, DIMENSION),
            3,
            &RequestBudgetV1::unbounded(),
        )?;
        if hits
            .first()
            .is_none_or(|hit| hit.candidate_id != id || (hit.score - 1.0).abs() > 1e-5)
        {
            return Err(format!("retained {id} was not served: {hits:?}").into());
        }
    }
    if library_view(&generation_dir(temp.path(), appended))?.stats != Some((360, 0, Some(2))) {
        return Err("sealed parent changed during mixed successor retrain".into());
    }
    Ok(())
}

/// Base row authority is checked on an ordinary append as well as on
/// contraction; the same forged base cannot be inherited through either path.
#[test]
fn ordinary_delta_refuses_a_forged_base_row_root() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let successor = ManifestGeneration::new(2);
    seal_with_scopes(
        &adapter,
        base,
        None,
        vec![scope(
            "src/base.rs",
            records("base", "src/base.rs", 0..300)?,
        )],
        &[],
    )?;
    forge_row_root(&generation_dir(temp.path(), base))?;
    let _cheap_open = adapter.open(&repo(), &revision(), base)?;
    let refused = seal_with_scopes(
        &adapter,
        successor,
        Some(base),
        vec![scope(
            "src/new.rs",
            records("new", "src/new.rs", 1_000..1_010)?,
        )],
        &[],
    );
    if refused.as_ref().err().is_none_or(|error| {
        !error
            .to_string()
            .contains("sealed base row commitment differs")
    }) {
        return Err(format!("ordinary delta accepted a forged base root: {refused:?}").into());
    }
    if generation_dir(temp.path(), successor)
        .join("MARKER_SEALED")
        .exists()
    {
        return Err("a refused ordinary delta published a sealed marker".into());
    }
    Ok(())
}

/// The cheap base open accepts a re-committed sidecar with a false row root.
///
/// A segment-contraction retrain must recompute the sealed
/// base's canonical row commitment before accepting it as training authority.
#[test]
fn contracted_successor_refuses_a_forged_base_row_root() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let appended = ManifestGeneration::new(2);
    let successor = ManifestGeneration::new(3);
    seal_with_scopes(
        &adapter,
        base,
        None,
        vec![scope(
            "src/base.rs",
            records("base", "src/base.rs", 0..300)?,
        )],
        &[],
    )?;
    seal_with_scopes(
        &adapter,
        appended,
        Some(base),
        vec![scope(
            "src/new.rs",
            records("new", "src/new.rs", 1_000..1_060)?,
        )],
        &[],
    )?;
    if library_view(&generation_dir(temp.path(), appended))?.stats != Some((360, 0, Some(2))) {
        return Err("fixture must have two indexed segments before forging the root".into());
    }
    let appended_dir = generation_dir(temp.path(), appended);
    forge_row_root(&appended_dir)?;
    let _cheap_open = adapter.open(&repo(), &revision(), appended)?;

    let tombstones = (1_000..1_060)
        .map(|seed| {
            tombstone_scope_v1(SemanticSourceScopeKeyV1 {
                corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                owner_kind: OwnerDocKind::Chunk,
                owner_id: format!("owner-new-{seed}"),
            })
        })
        .collect::<Vec<_>>();
    let refused = seal_with_scopes(&adapter, successor, Some(appended), Vec::new(), &tombstones);
    if refused.as_ref().err().is_none_or(|error| {
        !error
            .to_string()
            .contains("sealed base row commitment differs")
    }) {
        return Err(
            format!("forged base row root did not refuse successor retrain: {refused:?}").into(),
        );
    }
    if generation_dir(temp.path(), successor)
        .join("MARKER_SEALED")
        .exists()
    {
        return Err("a refused successor published a sealed marker".into());
    }
    Ok(())
}

/// Rewrite a current manifest as a format-8 manifest (the index contract
/// without its lineage record) and re-commit it in the sealed manifest, so
/// the generation is exactly what a pre-W3 seal left behind.
fn downgrade_to_v8(generation_dir: &Path) -> TestResult {
    let manifest_path = generation_dir.join(SCOPE_MANIFEST);
    let bytes = std::fs::read(&manifest_path)?;
    let mut value: ciborium::value::Value = ciborium::from_reader(&bytes[..])?;
    let ciborium::value::Value::Map(entries) = &mut value else {
        return Err("scope manifest is not a map".into());
    };
    let mut version_seen = false;
    let mut lineage_seen = false;
    for (key, field) in entries.iter_mut() {
        match key.as_text() {
            Some("format_version") => {
                *field = ciborium::value::Value::Integer(8.into());
                version_seen = true;
            }
            Some("vector_index") => {
                let ciborium::value::Value::Map(seal) = field else {
                    return Err("vector_index is not a map".into());
                };
                let Some((_, ann)) = seal
                    .iter_mut()
                    .find(|(key, _)| key.as_text() == Some("ann"))
                else {
                    return Err("vector_index has no ann record".into());
                };
                let ciborium::value::Value::Map(ann) = ann else {
                    return Err("ann is not a map".into());
                };
                let before = ann.len();
                ann.retain(|(key, _)| key.as_text() != Some("lineage"));
                lineage_seen = ann.len() != before;
            }
            _ => {}
        }
    }
    if !version_seen || !lineage_seen {
        return Err("manifest carried no format_version or no lineage to strip".into());
    }
    let mut legacy = Vec::new();
    ciborium::into_writer(&value, &mut legacy)?;
    std::fs::write(&manifest_path, &legacy)?;
    recommit_scope_manifest(generation_dir, &legacy)
}

/// Re-commit a canonical but false row root so the cheap open still passes.
fn forge_row_root(generation_dir: &Path) -> TestResult {
    let manifest_path = generation_dir.join(SCOPE_MANIFEST);
    let mut manifest: ciborium::value::Value =
        ciborium::from_reader(&std::fs::read(&manifest_path)?[..])?;
    let ciborium::value::Value::Map(entries) = &mut manifest else {
        return Err("scope manifest is not a map".into());
    };
    let root = entries
        .iter_mut()
        .find(|(key, _)| key.as_text() == Some("semantic_row_root_digest"))
        .map(|(_, value)| value)
        .ok_or("scope manifest has no semantic row root")?;
    let forged = format!("sha256:{}", "0".repeat(64));
    if root.as_text() == Some(forged.as_str()) {
        return Err("fixture row root unexpectedly equals the forged digest".into());
    }
    *root = ciborium::value::Value::Text(forged);
    let mut manifest_bytes = Vec::new();
    ciborium::into_writer(&manifest, &mut manifest_bytes)?;
    std::fs::write(&manifest_path, &manifest_bytes)?;
    recommit_scope_manifest(generation_dir, &manifest_bytes)?;
    Ok(())
}

/// Re-commit `manifest_bytes` in the sealed manifest's scope commitment.
fn recommit_scope_manifest(generation_dir: &Path, manifest_bytes: &[u8]) -> TestResult {
    let sealed_path = generation_dir.join(SEALED_MANIFEST);
    let sealed_bytes = std::fs::read(&sealed_path)?;
    let mut sealed: ciborium::value::Value = ciborium::from_reader(&sealed_bytes[..])?;
    let ciborium::value::Value::Array(row) = &mut sealed else {
        return Err("sealed manifest is not an array".into());
    };
    let Some(commitment) = row.get_mut(2) else {
        return Err("sealed manifest has no scope commitment".into());
    };
    let digest: [u8; 32] = Sha256::digest(manifest_bytes).into();
    *commitment = ciborium::value::Value::Array(vec![
        ciborium::value::Value::Integer(u64::try_from(manifest_bytes.len())?.into()),
        ciborium::value::Value::Bytes(digest.to_vec()),
    ]);
    let mut resealed = Vec::new();
    ciborium::into_writer(&sealed, &mut resealed)?;
    std::fs::write(&sealed_path, &resealed)?;
    Ok(())
}

/// The documented typed behaviour of a sealed generation whose ANN files
/// are lost or damaged (QI-BB-027 완료 기준 #2, QI-BB-017).
///
/// A file that is missing or resized is refused at both doors and after a restart,
/// typed as `GENERATION_SIDECAR_CORRUPT`, by the cheap layout check; a
/// same-length rewrite passes the cheap doors by design and is the scrub's
/// to find, after which the generation is quarantined durably and every
/// door — before and after a restart — refuses it typed as
/// `GENERATION_QUARANTINED`, the inventory lists it as content-corrupt, and
/// the quarantine discard is the way it leaves the disk.
#[test]
fn losing_or_damaging_the_index_files_refuses_both_doors_and_a_restart() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(1);
    seal_rows(&adapter, generation, 300)?;
    let damaged_dir = generation_dir(&root, generation);
    let files = index_files(&damaged_dir)?;
    let Some(first) = files.first() else {
        return Err("a 300-row seal writes index files".into());
    };
    let unbounded = IntegrityScrubBudgetV1 {
        max_bytes: u64::MAX,
    };

    // An intact generation scrubs clean and leaves a completion receipt.
    let clean = adapter.scrub(&identity(generation), None, unbounded)?;
    if clean.outcome != IntegrityScrubOutcomeV1::Completed || clean.files_verified == 0 {
        return Err(format!("an intact generation must scrub clean, got {clean:?}").into());
    }

    // Truncate one index file: the layout check refuses at both doors.
    let original = std::fs::read(first)?;
    std::fs::write(
        first,
        original
            .get(..original.len().saturating_sub(1))
            .ok_or("the index file is not empty")?,
    )?;
    for (door, result) in [
        (
            "validate",
            adapter.validate_generation_identity(&identity(generation)),
        ),
        (
            "open",
            adapter
                .open(&repo(), &revision(), generation)
                .map(|_searcher| ()),
        ),
    ] {
        if typed_code(&result).as_deref() != Some("GENERATION_SIDECAR_CORRUPT") {
            return Err(
                format!("{door} must refuse a truncated index file, got {result:?}").into(),
            );
        }
    }
    std::fs::write(first, &original)?;
    adapter.validate_generation_identity(&identity(generation))?;

    // Damage one index file in place, same length: the layout check cannot
    // tell (it reads no dataset byte), but the library cannot load the
    // index's footer, so both doors refuse typed as `ANN_INDEX_MISSING`
    // rather than serving an exact scan in its place …
    let mut damaged = original.clone();
    let Some(last) = damaged.last_mut() else {
        return Err("index file is empty".into());
    };
    *last ^= 0xFF;
    std::fs::write(first, &damaged)?;
    for (door, result) in [
        (
            "validate",
            adapter.validate_generation_identity(&identity(generation)),
        ),
        (
            "open",
            adapter
                .open(&repo(), &revision(), generation)
                .map(|_searcher| ()),
        ),
    ] {
        if typed_code(&result).as_deref() != Some("ANN_INDEX_MISSING") {
            return Err(format!(
                "{door} must refuse an index the library cannot load, typed, got {result:?}"
            )
            .into());
        }
    }
    // … and the scrub finds the byte, quarantines the generation, and
    // reports the entry the inventory now lists.
    let found = adapter.scrub(&identity(generation), None, unbounded)?;
    let IntegrityScrubOutcomeV1::Corrupt { quarantined } = &found.outcome else {
        return Err(format!("the scrub must find the damaged index file, got {found:?}").into());
    };
    if quarantined.path != damaged_dir
        || quarantined.reason != GenerationQuarantineReasonV1::ContentCorrupt
        || !quarantined.detail.contains("_indices")
    {
        return Err(format!("the quarantine names the damaged file: {quarantined:?}").into());
    }
    let restarted = SemanticAdapter::with_state_root(root.clone())?;
    for (door, result) in [
        (
            "validate",
            adapter.validate_generation_identity(&identity(generation)),
        ),
        (
            "open",
            adapter
                .open(&repo(), &revision(), generation)
                .map(|_searcher| ()),
        ),
        (
            "validate after restart",
            restarted.validate_generation_identity(&identity(generation)),
        ),
        (
            "open after restart",
            restarted
                .open(&repo(), &revision(), generation)
                .map(|_searcher| ()),
        ),
        (
            "scrub again",
            restarted
                .scrub(&identity(generation), None, unbounded)
                .map(|_report| ()),
        ),
    ] {
        if typed_code(&result).as_deref() != Some("GENERATION_QUARANTINED") {
            return Err(format!(
                "{door} must refuse a quarantined generation typed, got {result:?}"
            )
            .into());
        }
    }
    let inventory = inventory_persisted_generations(&root)?;
    if !inventory.sealed.is_empty()
        || inventory
            .quarantined
            .iter()
            .map(|entry| (entry.path.clone(), entry.reason))
            .collect::<Vec<_>>()
            != vec![(
                damaged_dir.clone(),
                GenerationQuarantineReasonV1::ContentCorrupt,
            )]
    {
        return Err(format!(
            "the inventory must quarantine the generation as content-corrupt: {inventory:?}"
        )
        .into());
    }
    // Restoring the bytes does not lift the quarantine: the receipt is
    // durable and only the quarantine discard removes it.
    std::fs::write(first, &original)?;
    let still = restarted
        .open(&repo(), &revision(), generation)
        .map(|_searcher| ());
    if typed_code(&still).as_deref() != Some("GENERATION_QUARANTINED") {
        return Err(
            format!("restoring the bytes must not lift the quarantine, got {still:?}").into(),
        );
    }
    let Some(entry) = inventory.quarantined.first() else {
        return Err("one quarantine entry".into());
    };
    let discarded = restarted.discard_quarantined_generation(entry)?;
    if !matches!(discarded, QuarantineDiscardOutcomeV1::Discarded { bytes } if bytes > 0)
        || damaged_dir.exists()
    {
        return Err(format!("the discard removes the generation, got {discarded:?}").into());
    }

    // A generation missing every index file is refused after a restart.
    let generation = ManifestGeneration::new(2);
    seal_rows(&restarted, generation, 300)?;
    for file in index_files(&generation_dir(&root, generation))? {
        std::fs::remove_file(file)?;
    }
    let restarted = SemanticAdapter::with_state_root(root)?;
    let after_restart = restarted
        .open(&repo(), &revision(), generation)
        .map(|_searcher| ());
    if typed_code(&after_restart).as_deref() != Some("GENERATION_SIDECAR_CORRUPT") {
        return Err(format!(
            "a restart must refuse a generation missing its index files, got {after_restart:?}"
        )
        .into());
    }
    let validate = restarted.validate_generation_identity(&identity(generation));
    if typed_code(&validate).as_deref() != Some("GENERATION_SIDECAR_CORRUPT") {
        return Err(format!("activation must refuse it too, got {validate:?}").into());
    }
    Ok(())
}

/// Rewrite a current manifest as a format-7 manifest (no vector index seal)
/// and re-commit it in the sealed manifest, so the generation is exactly
/// what a pre-QI-BB-027 seal left behind.
fn downgrade_to_v7(generation_dir: &Path) -> TestResult {
    let manifest_path = generation_dir.join(SCOPE_MANIFEST);
    let bytes = std::fs::read(&manifest_path)?;
    let mut value: ciborium::value::Value = ciborium::from_reader(&bytes[..])?;
    let ciborium::value::Value::Map(entries) = &mut value else {
        return Err("scope manifest is not a map".into());
    };
    let mut version_seen = false;
    entries.retain(|(key, _)| key.as_text() != Some("vector_index"));
    for (key, field) in entries.iter_mut() {
        if key.as_text() == Some("format_version") {
            *field = ciborium::value::Value::Integer(7.into());
            version_seen = true;
        }
    }
    if !version_seen || entries.len() != 21 {
        return Err(format!("unexpected manifest shape: {} fields", entries.len()).into());
    }
    let mut legacy = Vec::new();
    ciborium::into_writer(&value, &mut legacy)?;
    std::fs::write(&manifest_path, &legacy)?;
    recommit_scope_manifest(generation_dir, &legacy)
}

/// Every earlier format is refused typed at every door, never served on
/// what it happens to carry (QI-BB-027, breaking-first).
///
/// The cases: a format-8
/// generation (index contract without lineage), a format-7 one (no index
/// contract at all), and a delta that names either as its base. The boot
/// inventory sets them aside under the format reason instead of seeding
/// them, so nothing downstream can pin, activate or build on them.
#[test]
fn a_generation_sealed_under_an_earlier_format_is_refused_typed_at_every_door() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let format_8 = ManifestGeneration::new(1);
    let format_7 = ManifestGeneration::new(2);
    let current = ManifestGeneration::new(3);
    seal_rows(&adapter, format_8, 300)?;
    seal_rows(&adapter, format_7, 12)?;
    seal_rows(&adapter, current, 20)?;
    downgrade_to_v8(&generation_dir(&root, format_8))?;
    downgrade_to_v7(&generation_dir(&root, format_7))?;

    let reopened = SemanticAdapter::with_state_root(root.clone())?;
    for (label, generation, format) in [("format 8", format_8, 8), ("format 7", format_7, 7)] {
        let validated = reopened.validate_generation_identity(&identity(generation));
        let opened = reopened
            .open(&repo(), &revision(), generation)
            .map(|_searcher| ());
        for (door, outcome) in [("validate", validated), ("open", opened)] {
            match outcome {
                Err(CoreError::Typed { code, message })
                    if code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported
                        && message.contains(&format!("format version {format}"))
                        && message.contains("rebuild") => {}
                other => {
                    return Err(format!(
                        "{label}: {door} must refuse typed with the rebuild instruction, got {other:?}"
                    )
                    .into());
                }
            }
        }
        // A delta over an earlier-format base cannot be sealed on it.
        let delta = ManifestGeneration::new(generation.get() + 10);
        let sealed = seal_with_scopes(
            &reopened,
            delta,
            Some(generation),
            vec![scope(
                "src/new.rs",
                records("new", "src/new.rs", 1_000..1_010)?,
            )],
            &[],
        );
        match sealed {
            Err(error)
                if error
                    .to_string()
                    .contains("GENERATION_MANIFEST_FORMAT_UNSUPPORTED")
                    || error
                        .to_string()
                        .contains(&format!("format version {format}")) => {}
            other => {
                return Err(format!(
                    "{label}: a delta over the base must be refused, got {other:?}"
                )
                .into());
            }
        }
    }
    // The current-format generation beside them still serves.
    reopened.validate_generation_identity(&identity(current))?;
    let hits = reopened.open(&repo(), &revision(), current)?.search(
        &unit_vector(3, DIMENSION),
        1,
        &RequestBudgetV1::unbounded(),
    )?;
    if hits.first().map(|hit| hit.candidate_id.as_str()) != Some("row-3") {
        return Err(format!("the current generation must serve: {hits:?}").into());
    }
    // The inventory quarantines both under the format reason and seeds
    // only the current one.
    let inventory = inventory_persisted_generations(&root)?;
    let seeded: Vec<u64> = inventory
        .sealed
        .iter()
        .map(|record| record.generation.get())
        .collect();
    if seeded != vec![current.get()] {
        return Err(format!("only the current generation is seeded, got {seeded:?}").into());
    }
    let mut quarantined: Vec<(PathBuf, GenerationQuarantineReasonV1)> = inventory
        .quarantined
        .iter()
        .map(|entry| (entry.path.clone(), entry.reason))
        .collect();
    quarantined.sort();
    let mut expected = vec![
        (
            generation_dir(&root, format_8),
            GenerationQuarantineReasonV1::FormatUnsupported,
        ),
        (
            generation_dir(&root, format_7),
            GenerationQuarantineReasonV1::FormatUnsupported,
        ),
    ];
    expected.sort();
    if quarantined != expected {
        return Err(format!("unexpected quarantine set: {quarantined:?}").into());
    }
    for entry in &inventory.quarantined {
        if !entry.detail.contains("rebuild") {
            return Err(format!("the quarantine detail names the remedy: {entry:?}").into());
        }
    }
    Ok(())
}

/// Exact top-k by cosine over `rows`, ties broken by id, computed here
/// without the library.
fn exact_top_k(rows: &[(String, Vec<f32>)], query: &[f32], k: usize) -> Vec<(String, f32)> {
    let mut scored: Vec<(String, f32)> = rows
        .iter()
        .map(|(id, vector)| {
            let score = vector
                .iter()
                .zip(query.iter())
                .map(|(a, b)| a * b)
                .sum::<f32>();
            (id.clone(), score)
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    scored.truncate(k);
    scored
}

/// An approximate page is never short while its scope holds more rows
/// (QI-BB-025).
///
/// Over 600 rows sealed through the index, every `top_k` from one to past
/// the row count returns exactly `min(top_k, 600)` distinct rows, and a
/// scoped search over an allowlist of 50 of them returns exactly those 50
/// however large `top_k` is. The pages past the row count and past the
/// allowlist come back short of `top_k`, and the scope's own count proves
/// them complete; a pass the count proves short is answered by the exact
/// lane, which `e2e_top_k_truth_table` drives over 10,001 rows, where the
/// graph walk reaches only part of them.
#[test]
fn an_approximate_page_is_never_short_while_the_scope_holds_more() -> TestResult {
    const ROWS: u64 = 600;
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    seal_rows(&adapter, generation, ROWS)?;
    let searcher = adapter.open(&repo(), &revision(), generation)?;
    if searcher.dense_lane() != sealed_ann_lane(trained_at(1, ROWS)) {
        return Err(format!("600 rows must seal the index: {:?}", searcher.dense_lane()).into());
    }
    let query = unit_vector(9_999, DIMENSION);
    let unbounded = RequestBudgetV1::unbounded();
    for top_k in [1_u32, 10, 300, 599, 600, 601, 1_000] {
        let hits = searcher.search(&query, top_k, &unbounded)?;
        let distinct: BTreeSet<&str> = hits.iter().map(|hit| hit.candidate_id.as_str()).collect();
        let expected = usize::try_from(u64::from(top_k).min(ROWS))?;
        if hits.len() != expected || distinct.len() != expected {
            return Err(format!(
                "top_k={top_k} returned {} rows ({} distinct), expected {expected}",
                hits.len(),
                distinct.len()
            )
            .into());
        }
    }
    let allowed: BTreeSet<String> = (0..50_u64)
        .map(|seed| format!("row-{}", seed * 7))
        .collect();
    for top_k in [10_u32, 50, 51, 1_000] {
        let hits = searcher.search_scoped(&query, &allowed, top_k, &unbounded)?;
        let returned: BTreeSet<String> = hits.into_iter().map(|hit| hit.candidate_id).collect();
        let expected = usize::try_from(top_k.min(50))?;
        if returned.len() != expected || !returned.is_subset(&allowed) {
            return Err(format!(
                "a scoped top_k={top_k} returned {} rows, expected {expected} from the allowlist",
                returned.len()
            )
            .into());
        }
    }
    Ok(())
}

/// The seal-time recall floor: the sealed effort keeps recall@10 at or
/// above 0.95 against an exhaustive oracle and returns exact cosine scores.
///
/// The measurement itself — recall, latency percentiles, build time, index
/// bytes — is the ANN rail's current-head artifact
/// (`just rust-verify-quality-ann`, QI-BB-027 #3); this gate holds the floor
/// on every run.
#[test]
fn the_sealed_effort_keeps_recall_against_an_exact_oracle_and_returns_exact_scores() -> TestResult {
    // A wider space than the other fixtures: random directions in 64
    // dimensions have no cluster structure for the graph to exploit, which
    // is the hardest case for an approximate index at this row count.
    const WIDE: usize = 64;
    const ROWS: u64 = 4_096;
    const QUERIES: u64 = 64;
    const K: usize = 10;
    const RECALL_FLOOR: f64 = 0.95;
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    let path = "src/corpus.rs";
    let rows: Vec<(String, Vec<f32>)> = (0..ROWS)
        .map(|seed| (format!("row-{seed}"), unit_vector(seed, WIDE)))
        .collect();
    let embeddings = rows
        .iter()
        .map(|(id, vector)| legacy_chunk_embedding_record_v1(id, path, vector.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut batch = sealed_replace_batch_v1(
        repo(),
        revision(),
        generation,
        path,
        embeddings,
        u32::try_from(WIDE)?,
    );
    batch.manifest_digest = format!("manifest:{}", generation.get());
    build_resident_batch_v1(&adapter, &batch)?;

    let searcher = adapter.open(&repo(), &revision(), generation)?;
    if searcher.dense_lane() != sealed_ann_lane(trained_at(1, ROWS)) {
        return Err(format!(
            "the corpus must be served through the sealed index: {:?}",
            searcher.dense_lane()
        )
        .into());
    }
    let mut recall_sum = 0.0_f64;
    for query_seed in 0..QUERIES {
        // Half the queries sit near a row (a paraphrase), half are fresh.
        let query = if query_seed % 2 == 0 {
            let near = unit_vector(query_seed.wrapping_mul(61) % ROWS, WIDE);
            let noise = unit_vector(1_000_000 + query_seed, WIDE);
            let mixed: Vec<f32> = near
                .iter()
                .zip(noise.iter())
                .map(|(a, b)| a + 0.35 * b)
                .collect();
            let norm = mixed.iter().map(|v| v * v).sum::<f32>().sqrt();
            mixed.iter().map(|v| v / norm).collect()
        } else {
            unit_vector(2_000_000 + query_seed, WIDE)
        };
        let expected = exact_top_k(&rows, &query, K);
        let hits = searcher.search(&query, u32::try_from(K)?, &RequestBudgetV1::unbounded())?;
        if hits.len() != K {
            return Err(format!("query {query_seed} returned {} of {K} hits", hits.len()).into());
        }
        for hit in &hits {
            let Some((_, exact_score)) =
                rows.iter()
                    .find(|(id, _)| *id == hit.candidate_id)
                    .map(|(id, vector)| {
                        (
                            id,
                            vector
                                .iter()
                                .zip(query.iter())
                                .map(|(a, b)| a * b)
                                .sum::<f32>(),
                        )
                    })
            else {
                return Err(format!("hit {} is not a row", hit.candidate_id).into());
            };
            if (hit.score - exact_score).abs() > 1e-4 {
                return Err(format!(
                    "the refine step must return exact cosine scores: {} scored {} but is {exact_score}",
                    hit.candidate_id, hit.score
                )
                .into());
            }
        }
        let found = hits
            .iter()
            .filter(|hit| expected.iter().any(|(id, _)| *id == hit.candidate_id))
            .count();
        recall_sum += f64::from(u32::try_from(found)?) / f64::from(u32::try_from(K)?);
    }
    let recall = recall_sum / f64::from(u32::try_from(QUERIES)?);
    if recall < RECALL_FLOOR {
        return Err(format!("recall@{K} {recall:.4} fell below the floor {RECALL_FLOOR}").into());
    }
    Ok(())
}
