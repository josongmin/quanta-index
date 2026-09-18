//! QI-BB-027 — the dense lane's index is part of the sealed contract.
//!
//! A seal records which index it built, with every parameter and what the
//! library reported; the open verifies the dataset against that record; a
//! query runs with exactly the sealed effort. The oracles here are
//! independent of the adapter's own claims: the library's index listing and
//! statistics read straight from the sealed dataset, an exhaustive exact
//! cosine ranking computed in this file, fault injection on the index files,
//! and legacy manifests rewritten byte for byte.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use quanta_index_contract::{
    BatchIngestMode, EmbeddingRecord, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneTrackKind, SemanticReplaceScope,
};
use quanta_index_core::{
    CoreError, DenseIndexEffortV1, DenseIndexLineageV1, DenseIndexTrainingV1, DenseIndexV1,
    DenseLaneAttestationV1, DenseLaneContractV1, GenerationIdentityValidatePort,
    GenerationStorageKeyV1, RequestBudgetV1, SemanticIndexOpenPort,
};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, legacy_chunk_embedding_record_v1,
    sealed_replace_batch_v1, search_scope_v1, tombstone_scope_v1,
};
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

const DIMENSION: usize = 16;
const INDEX_NAME: &str = "vector_ivf_hnsw_sq";
const SCOPE_MANIFEST: &str = "semantic-manifest.cbor";
const SEALED_MANIFEST: &str = "semantic-sealed-manifest.cbor";

fn repo() -> RepoId {
    RepoId::new("ann-contract-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("ann-contract-rev")
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
    tombstoned_paths: &[&str],
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
    batch.tombstone_scopes = tombstoned_paths
        .iter()
        .map(|path| tombstone_scope_v1(path))
        .collect();
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
        Err(CoreError::Typed { code, .. }) => Some(code.clone()),
        _ => None,
    }
}

/// The policy's sealed approximate lane with `lineage` behind it.
fn sealed_ann_lane(lineage: DenseIndexLineageV1) -> DenseLaneContractV1 {
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
        },
        attestation: DenseLaneAttestationV1::Sealed,
    }
}

/// The lineage a seal that trained at `generation` over `rows` rows
/// records.
fn trained_at(generation: u64, rows: u64) -> DenseIndexLineageV1 {
    DenseIndexLineageV1::Recorded(DenseIndexTrainingV1 {
        trained_at_generation: generation,
        trained_rows: rows,
        appended_rows: 0,
        deleted_rows: 0,
    })
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
    // 300 rows sealed an index; the delta tombstones one path and holds 200.
    seal_with_scopes(&adapter, delta, Some(base), Vec::new(), &["src/drop.rs"])?;

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
    let expected = sealed_ann_lane(DenseIndexLineageV1::Recorded(DenseIndexTrainingV1 {
        trained_at_generation: 1,
        trained_rows: 300,
        appended_rows: 60,
        deleted_rows: 0,
    }));
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

/// A base sealed before the lineage record serves as sealed with an
/// unrecorded lineage, and a delta over it retrains even inside the budget:
/// there is no record to append against.
#[test]
fn a_base_sealed_before_the_lineage_record_is_retrained_not_appended() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let base = ManifestGeneration::new(1);
    let delta = ManifestGeneration::new(2);
    seal_rows(&adapter, base, 300)?;
    downgrade_to_v8(&generation_dir(&root, base))?;

    let reopened = SemanticAdapter::with_state_root(root.clone())?;
    reopened.validate_generation_identity(&identity(base))?;
    let base_lane = reopened.open(&repo(), &revision(), base)?.dense_lane();
    if base_lane != sealed_ann_lane(DenseIndexLineageV1::Unrecorded) {
        return Err(format!(
            "a v8 generation is sealed and verified but its lineage is unrecorded: {base_lane:?}"
        )
        .into());
    }

    seal_with_scopes(
        &reopened,
        delta,
        Some(base),
        vec![scope(
            "src/new.rs",
            records("new", "src/new.rs", 1_000..1_030)?,
        )],
        &[],
    )?;
    let delta_lane = reopened.open(&repo(), &revision(), delta)?.dense_lane();
    if delta_lane != sealed_ann_lane(trained_at(2, 330)) {
        return Err(format!("a delta over a v8 base must retrain, got {delta_lane:?}").into());
    }
    if library_view(&generation_dir(&root, delta))?.stats != Some((330, 0, Some(1))) {
        return Err("the retrained index must cover every row in one segment".into());
    }
    Ok(())
}

#[test]
fn losing_or_damaging_the_index_files_refuses_both_doors_and_a_restart() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(1);
    seal_rows(&adapter, generation, 300)?;
    let generation_dir = generation_dir(&root, generation);
    let files = index_files(&generation_dir)?;
    let Some(first) = files.first() else {
        return Err("a 300-row seal writes index files".into());
    };

    // Damage one index file in place.
    let original = std::fs::read(first)?;
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
        if typed_code(&result).as_deref() != Some("GENERATION_SIDECAR_CORRUPT") {
            return Err(format!("{door} must refuse a damaged index file, got {result:?}").into());
        }
    }
    std::fs::write(first, &original)?;
    adapter.validate_generation_identity(&identity(generation))?;

    // Remove every index file; a restarted adapter refuses the same way.
    for file in &files {
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

#[test]
fn a_generation_sealed_before_the_contract_serves_unverified_on_what_it_carries() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let indexed = ManifestGeneration::new(1);
    let exact = ManifestGeneration::new(2);
    seal_rows(&adapter, indexed, 300)?;
    seal_rows(&adapter, exact, 12)?;
    downgrade_to_v7(&generation_dir(&root, indexed))?;
    downgrade_to_v7(&generation_dir(&root, exact))?;

    let reopened = SemanticAdapter::with_state_root(root)?;
    reopened.validate_generation_identity(&identity(indexed))?;
    let indexed_searcher = reopened.open(&repo(), &revision(), indexed)?;
    let lane = indexed_searcher.dense_lane();
    let DenseIndexV1::Approximate { effort, lineage } = &lane.index else {
        return Err(format!("a v7 generation with an index reports it: {lane:?}").into());
    };
    if lane.attestation != DenseLaneAttestationV1::LegacyUnverified
        || effort.index_kind != "ivf_hnsw_sq"
        || *lineage != DenseIndexLineageV1::Unrecorded
    {
        return Err(format!("a v7 index is served unverified: {lane:?}").into());
    }
    let hits = indexed_searcher.search(
        &unit_vector(11, DIMENSION),
        3,
        &RequestBudgetV1::unbounded(),
    )?;
    if hits.first().map(|hit| hit.candidate_id.as_str()) != Some("row-11") {
        return Err(format!("a v7 index still serves: {hits:?}").into());
    }

    let exact_lane = reopened.open(&repo(), &revision(), exact)?.dense_lane();
    if exact_lane
        != (DenseLaneContractV1 {
            index: DenseIndexV1::Exact,
            attestation: DenseLaneAttestationV1::LegacyUnverified,
        })
    {
        return Err(format!(
            "a v7 generation without an index is exact, unverified: {exact_lane:?}"
        )
        .into());
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

#[test]
#[expect(
    clippy::print_stdout,
    reason = "the QI-BB-027-EVIDENCE line is the measurement the ledger cites; it must land in the run log"
)]
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
    let build_started = Instant::now();
    build_resident_batch_v1(&adapter, &batch)?;
    let build_millis = build_started.elapsed().as_millis();
    let index_bytes: u64 = index_files(&generation_dir(temp.path(), generation))?
        .iter()
        .map(|file| std::fs::metadata(file).map(|meta| meta.len()))
        .collect::<Result<Vec<u64>, _>>()?
        .iter()
        .sum();

    let searcher = adapter.open(&repo(), &revision(), generation)?;
    if searcher.dense_lane() != sealed_ann_lane(trained_at(1, ROWS)) {
        return Err(format!(
            "the corpus must be served through the sealed index: {:?}",
            searcher.dense_lane()
        )
        .into());
    }
    let mut recall_sum = 0.0_f64;
    let mut latencies_micros = Vec::with_capacity(usize::try_from(QUERIES)?);
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
        let started = Instant::now();
        let hits = searcher.search(&query, u32::try_from(K)?, &RequestBudgetV1::unbounded())?;
        latencies_micros.push(started.elapsed().as_micros());
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
    latencies_micros.sort_unstable();
    let percentile = |percent: usize| -> Option<u128> {
        let last = latencies_micros.len().checked_sub(1)?;
        latencies_micros
            .get(last.checked_mul(percent)?.checked_div(100)?)
            .copied()
    };
    println!(
        "QI-BB-027-EVIDENCE rows={ROWS} dim={WIDE} queries={QUERIES} k={K} recall_at_k={recall:.4} p50_us={:?} p95_us={:?} p99_us={:?} build_ms={build_millis} index_bytes={index_bytes}",
        percentile(50),
        percentile(95),
        percentile(99)
    );
    if recall < RECALL_FLOOR {
        return Err(format!("recall@{K} {recall:.4} fell below the floor {RECALL_FLOOR}").into());
    }
    Ok(())
}
