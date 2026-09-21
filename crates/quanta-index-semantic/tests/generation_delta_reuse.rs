//! W3 semantic lane — a delta generation inherits its base dataset by hard
//! link instead of copying it (the semantic half of QI-BB-006, on the terms
//! G0-S established).
//!
//! The G0-S probe proved the mechanics at the `LanceDB` level. This test pins
//! the *production* adapter path: `build_batch` with `base_generation` must
//! leave every base byte untouched, share the base's immutable objects with
//! the delta, keep the one generation-local bookkeeping file private to each
//! generation, and still serve both generations correctly.
//!
//! Oracles are independent of the adapter: inode / length / SHA-256 per file,
//! taken from the filesystem before and after, and vector search answers that
//! the fixture knows in advance.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use quanta_index_contract::{
    BatchIngestMode, EmbeddingRecord, ExactRepoRelativePathV1, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RevisionId, SemanticIngestBatch,
};
use quanta_index_core::{GenerationStorageKeyV1, RequestBudgetV1, SemanticIndexOpenPort};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, legacy_chunk_embedding_record_v1, model_contract_v1,
    sealed_replace_batch_v1,
};
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn Error>>;

const DIMENSION: u32 = 8;
const DIMENSION_USIZE: usize = 8;
const BASE_RECORDS: u16 = 512;
const BASE_PATH: &str = "src/base.rs";
const DELTA_PATH: &str = "src/delta.rs";
/// The file `LanceDB` rewrites on every commit; it must be private per
/// generation while everything else may be shared.
const VERSION_HINT: &str = "latest_version_hint.json";

fn repo_id() -> RepoId {
    RepoId::new("repo-delta-reuse").expect("static fixture ID satisfies canonical policy")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-delta-reuse").expect("static fixture ID satisfies canonical policy")
}

fn dataset_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo_id(), &revision_id())
        .generation_dir(root, generation)
        .join("dataset")
}

/// A deterministic pseudo-random unit vector per record.
///
/// Random directions in 8 dimensions are well separated (expected cosine
/// between two of them is near zero), so the exact vector of a record is
/// unambiguously its own nearest neighbor even through a scalar-quantized
/// ANN index. A rotation in a 2-D plane would not be: consecutive records
/// there differ by less than the quantizer's step.
fn unit_vector(step: u16) -> Vec<f32> {
    let mut state: u64 =
        0x9E37_79B9_7F4A_7C15 ^ u64::from(step).wrapping_mul(0x2545_F491_4F6C_DD1D);
    let mut vector = Vec::with_capacity(DIMENSION_USIZE);
    for _ in 0..DIMENSION_USIZE {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Top 16 bits -> [-1, 1).
        let bits = u16::try_from(state >> 48).map_or(0_i32, i32::from);
        let centered = bits.wrapping_sub(32_768);
        vector.push(f32::from(i16::try_from(centered).map_or(0_i16, |value| value)) / 32_768.0);
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn record(id: &str, path: &str, step: u16) -> Result<EmbeddingRecord, Box<dyn Error>> {
    Ok(legacy_chunk_embedding_record_v1(
        id,
        path,
        unit_vector(step),
    )?)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileFacts {
    inode: u64,
    len: u64,
    digest: String,
}

fn file_digest(path: &Path) -> Result<String, Box<dyn Error>> {
    let digest = Sha256::digest(std::fs::read(path)?);
    let mut encoded = String::with_capacity(digest.len().saturating_mul(2));
    for byte in digest {
        write!(&mut encoded, "{byte:02x}")?;
    }
    Ok(encoded)
}

/// Recursive inventory keyed by path relative to `root`.
fn inventory(root: &Path) -> Result<BTreeMap<String, FileFacts>, Box<dyn Error>> {
    let mut facts = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(root)
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|err| format!("inventory path escape: {err}"))?;
            let _prior = facts.insert(
                relative,
                FileFacts {
                    inode: metadata.ino(),
                    len: metadata.len(),
                    digest: file_digest(&entry.path())?,
                },
            );
        }
    }
    Ok(facts)
}

fn total_bytes(facts: &BTreeMap<String, FileFacts>) -> u64 {
    facts
        .values()
        .fold(0, |sum, file| sum.saturating_add(file.len))
}

fn build_base(adapter: &SemanticAdapter, base: ManifestGeneration) -> TestResult {
    let embeddings = (0..BASE_RECORDS)
        .map(|step| record(&format!("base-{step}"), BASE_PATH, step))
        .collect::<Result<Vec<_>, _>>()?;
    build_resident_batch_v1(
        adapter,
        &sealed_replace_batch_v1(
            repo_id(),
            revision_id(),
            base,
            BASE_PATH,
            embeddings,
            DIMENSION,
        ),
    )?;
    Ok(())
}

fn build_delta(
    adapter: &SemanticAdapter,
    base: ManifestGeneration,
    delta: ManifestGeneration,
) -> TestResult {
    let mut batch = sealed_replace_batch_v1(
        repo_id(),
        revision_id(),
        delta,
        DELTA_PATH,
        vec![record("delta-only", DELTA_PATH, 9_000)?],
        DIMENSION,
    );
    batch.base_generation = Some(base);
    batch.mode = BatchIngestMode::Delta;
    batch.manifest_digest = "manifest:delta".to_string();
    batch.batch_digest = "batch:delta".to_string();
    build_resident_batch_v1(adapter, &batch)?;
    Ok(())
}

/// Candidate ids a generation serves for `query` when the search is scoped
/// to exactly `path`.
///
/// The scope makes the oracle deterministic: the ANN index is approximate
/// and a top-1 over hundreds of rows is not a stable assertion, but the row
/// set behind an exact-path filter is, and so is its emptiness.
fn scoped_hit_ids(
    adapter: &SemanticAdapter,
    generation: ManifestGeneration,
    path: &str,
    query: &[f32],
    top_k: u32,
) -> Result<Vec<String>, Box<dyn Error>> {
    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
        ExactRepoRelativePathV1::new(path).map_err(str::to_string)?,
    );
    let hits =
        searcher.search_constrained(query, &constraints, top_k, &RequestBudgetV1::unbounded())?;
    Ok(hits.iter().map(|hit| hit.candidate_id.clone()).collect())
}

/// A delta generation shares the base's immutable objects and writes only
/// its own new bytes; the base is byte-for-byte untouched afterwards.
#[test]
#[expect(
    clippy::print_stdout,
    reason = "the QI-BB-006-SEMANTIC-EVIDENCE line is the measurement the ledger cites; it must land in the run log"
)]
fn delta_generation_inherits_base_dataset_by_link_without_touching_base_bytes() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let base = ManifestGeneration::new(1);
    let delta = ManifestGeneration::new(2);

    if usize::try_from(DIMENSION)? != DIMENSION_USIZE {
        return Err("fixture dimension constants disagree".into());
    }
    build_base(&adapter, base)?;
    let base_before = inventory(&dataset_dir(&root, base))?;
    let base_bytes = total_bytes(&base_before);
    if base_before.is_empty() {
        return Err("base dataset inventory is empty; fixture built nothing".into());
    }

    build_delta(&adapter, base, delta)?;

    // (1) Base immutability, including its own version hint: the delta's
    // first commit must not have repointed the base.
    let base_after = inventory(&dataset_dir(&root, base))?;
    if base_after != base_before {
        let changed: Vec<&String> = base_before
            .iter()
            .filter(|(name, facts)| base_after.get(*name) != Some(facts))
            .map(|(name, _)| name)
            .collect();
        let added: Vec<&String> = base_after
            .keys()
            .filter(|name| !base_before.contains_key(*name))
            .collect();
        return Err(format!(
            "delta build touched the base dataset: changed={changed:?} added={added:?}"
        )
        .into());
    }

    // (2) Sharing: the delta's dataset reuses base inodes for the immutable
    // objects, and the fresh bytes it wrote are a fraction of the base.
    let delta_facts = inventory(&dataset_dir(&root, delta))?;
    let base_inodes: std::collections::BTreeSet<u64> =
        base_before.values().map(|facts| facts.inode).collect();
    let mut shared_files = 0_usize;
    let mut fresh_bytes = 0_u64;
    let mut fresh_entries: Vec<(String, u64)> = Vec::new();
    for (name, facts) in &delta_facts {
        if base_inodes.contains(&facts.inode) {
            shared_files = shared_files.saturating_add(1);
        } else {
            fresh_bytes = fresh_bytes.saturating_add(facts.len);
            fresh_entries.push((name.clone(), facts.len));
        }
    }
    println!(
        "QI-BB-006-SEMANTIC-EVIDENCE base_files={} base_bytes={base_bytes} delta_files={} shared_files={shared_files} fresh_bytes={fresh_bytes}",
        base_before.len(),
        delta_facts.len()
    );
    println!("QI-BB-006-SEMANTIC-FRESH {fresh_entries:?}");
    if shared_files == 0 {
        return Err(
            "delta dataset shares no inode with the base: it was copied, not linked".into(),
        );
    }
    if fresh_bytes.saturating_mul(2) > base_bytes {
        return Err(format!(
            "delta wrote {fresh_bytes} fresh bytes against a {base_bytes}-byte base; a one-record delta must not rewrite the base"
        )
        .into());
    }

    // (3) The version hint is generation-local: present in both, never the
    // same inode.
    let base_hint = base_before
        .iter()
        .find(|(name, _)| name.ends_with(VERSION_HINT))
        .map(|(_, facts)| facts.inode)
        .ok_or("base dataset has no version hint")?;
    let delta_hint = delta_facts
        .iter()
        .find(|(name, _)| name.ends_with(VERSION_HINT))
        .map(|(_, facts)| facts.inode)
        .ok_or("delta dataset has no version hint")?;
    if base_hint == delta_hint {
        return Err(
            "delta's version hint is a hard link to the base's; a commit could repoint the base"
                .into(),
        );
    }

    // (4) Both generations still answer correctly from their own view.
    let delta_query = unit_vector(9_000);
    let base_query = unit_vector(7);
    let delta_own = scoped_hit_ids(&adapter, delta, DELTA_PATH, &delta_query, 5)?;
    if delta_own != ["delta-only"] {
        return Err(
            format!("delta generation does not serve its own new record: {delta_own:?}").into(),
        );
    }
    let delta_inherited = scoped_hit_ids(&adapter, delta, BASE_PATH, &base_query, 5)?;
    if delta_inherited.len() != 5 || !delta_inherited.iter().all(|id| id.starts_with("base-")) {
        return Err(
            format!("delta generation lost inherited base records: {delta_inherited:?}").into(),
        );
    }
    let base_leak = scoped_hit_ids(&adapter, base, DELTA_PATH, &delta_query, 5)?;
    if !base_leak.is_empty() {
        return Err(format!(
            "base generation can see the delta's record; generations are not isolated: {base_leak:?}"
        )
        .into());
    }
    Ok(())
}

/// A second batch into the same unsealed generation goes through the same
/// staging path from the generation's own dataset. It must not leave the
/// generation's earlier objects copied twice or corrupt them.
#[test]
fn same_generation_second_batch_reuses_its_own_objects() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(3);

    let first = {
        let mut batch = sealed_replace_batch_v1(
            repo_id(),
            revision_id(),
            generation,
            BASE_PATH,
            (0..BASE_RECORDS)
                .map(|step| record(&format!("base-{step}"), BASE_PATH, step))
                .collect::<Result<Vec<_>, _>>()?,
            DIMENSION,
        );
        batch.seal = false;
        batch
    };
    build_resident_batch_v1(&adapter, &first)?;
    let after_first = inventory(&dataset_dir(&root, generation))?;
    let inodes_after_first: std::collections::BTreeSet<u64> =
        after_first.values().map(|facts| facts.inode).collect();

    let second = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:second".to_string(),
        batch_digest: "batch:second".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract_v1(DIMENSION),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: sealed_replace_batch_v1(
            repo_id(),
            revision_id(),
            generation,
            DELTA_PATH,
            vec![record("delta-only", DELTA_PATH, 9_000)?],
            DIMENSION,
        )
        .replace_scopes,
        tombstone_scopes: Vec::new(),
        seal: true,
    };
    build_resident_batch_v1(&adapter, &second)?;
    let after_second = inventory(&dataset_dir(&root, generation))?;

    let retained = after_second
        .values()
        .filter(|facts| inodes_after_first.contains(&facts.inode))
        .count();
    if retained == 0 {
        return Err("second batch re-materialized every object of its own generation".into());
    }
    for (name, facts) in &after_first {
        if name.ends_with(VERSION_HINT) {
            continue;
        }
        if let Some(now) = after_second.get(name)
            && now.digest != facts.digest
        {
            return Err(format!("object `{name}` changed content across batches").into());
        }
    }
    let second_batch = scoped_hit_ids(&adapter, generation, DELTA_PATH, &unit_vector(9_000), 5)?;
    if second_batch != ["delta-only"] {
        return Err(format!(
            "sealed generation does not serve the second batch's record: {second_batch:?}"
        )
        .into());
    }
    let first_batch = scoped_hit_ids(&adapter, generation, BASE_PATH, &unit_vector(7), 5)?;
    if first_batch.len() != 5 || !first_batch.iter().all(|id| id.starts_with("base-")) {
        return Err(
            format!("sealed generation lost the first batch's records: {first_batch:?}").into(),
        );
    }
    Ok(())
}
