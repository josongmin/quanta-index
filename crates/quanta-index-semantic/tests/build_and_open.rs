//! Integration tests for `LanceSemanticAdapter` build + open flow.

use quanta_index_contract::{
    BundleArtifactRef, BundleEncoding, GenerationId, ManifestDigest, ManifestGeneration,
    PublishedGenerationSet, PublishedSearchBundleManifest, RepoId, RevisionId,
};
use quanta_index_core::CoreError;
use quanta_index_core::domains::materialization::outbound::{
    SearchPlaneSemanticIndexBuildPort, SearchPlaneVectorIndexStorePort, SemanticBuildInput,
};
use quanta_index_semantic::LanceSemanticAdapter;
use tempfile::TempDir;

const MAGIC: u32 = 0x5149_5345;

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error("core: {0}")]
    Core(#[from] CoreError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("assertion: {0}")]
    Assertion(String),
}

fn fail(msg: impl Into<String>) -> TestError {
    TestError::Assertion(msg.into())
}

fn make_manifest(gen_no: u64, has_embeddings: bool) -> PublishedSearchBundleManifest {
    let digest = ManifestDigest::new("sha256:placeholder");
    let lexical = BundleArtifactRef {
        relative_path: "lexical.arrow".to_owned(),
        encoding: BundleEncoding::ArrowIpc,
        byte_length: 0,
        content_digest: digest.clone(),
    };
    let symbols = BundleArtifactRef {
        relative_path: "symbols.arrow".to_owned(),
        encoding: BundleEncoding::ArrowIpc,
        byte_length: 0,
        content_digest: digest.clone(),
    };
    let embeddings = if has_embeddings {
        Some(BundleArtifactRef {
            relative_path: "embeddings.bin".to_owned(),
            encoding: BundleEncoding::RawF32,
            byte_length: 0,
            content_digest: digest,
        })
    } else {
        None
    };
    PublishedSearchBundleManifest {
        repo_id: RepoId::new("repo-x"),
        revision_id: RevisionId::new("rev-x"),
        manifest_generation: ManifestGeneration::new(gen_no),
        bundle_schema_version: 1,
        lexical_chunk_rows: lexical,
        symbol_rows: symbols,
        metadata_rows: None,
        graph_rows: None,
        embedding_input_views: None,
        embedding_records: embeddings,
        mutation_delta: None,
    }
}

fn make_generation_set(gen_no: u64) -> PublishedGenerationSet {
    PublishedGenerationSet {
        repo_id: RepoId::new("repo-x"),
        revision_id: RevisionId::new("rev-x"),
        manifest_generation: ManifestGeneration::new(gen_no),
        lexical_generation: GenerationId::new(1),
        symbol_generation: GenerationId::new(1),
        structural_generation: None,
        history_generation: None,
        semantic_generation: None,
        metadata_generation: None,
    }
}

fn encode_records(vector_dim: u32, records: &[(&str, Vec<f32>)]) -> Result<Vec<u8>, TestError> {
    let record_count = u32::try_from(records.len())
        .map_err(|_err| fail("test record_count does not fit in u32"))?;
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&MAGIC.to_le_bytes());
    buf.extend_from_slice(&record_count.to_le_bytes());
    buf.extend_from_slice(&vector_dim.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    for (entity_id, vector) in records {
        let id_bytes = entity_id.as_bytes();
        let id_len = u32::try_from(id_bytes.len())
            .map_err(|_err| fail("test id_len does not fit in u32"))?;
        buf.extend_from_slice(&id_len.to_le_bytes());
        buf.extend_from_slice(id_bytes);
        for lane in vector {
            buf.extend_from_slice(&lane.to_le_bytes());
        }
    }
    Ok(buf)
}

fn encode_with_header(record_count: u32, vector_dim: u32, body: &[u8]) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    buf.extend_from_slice(&MAGIC.to_le_bytes());
    buf.extend_from_slice(&record_count.to_le_bytes());
    buf.extend_from_slice(&vector_dim.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(body);
    buf
}

fn make_records(count: usize, vector_dim: usize) -> Result<Vec<(String, Vec<f32>)>, TestError> {
    let mut out: Vec<(String, Vec<f32>)> = Vec::with_capacity(count);
    for idx in 0..count {
        let id = format!("entity-{idx:04}");
        let mut vec: Vec<f32> = Vec::with_capacity(vector_dim);
        let idx_u32 = u32::try_from(idx).map_err(|_err| fail("idx does not fit in u32"))?;
        let dim_u32 = u32::try_from(vector_dim).map_err(|_err| fail("dim does not fit in u32"))?;
        for lane in 0..vector_dim {
            let lane_u32 = u32::try_from(lane).map_err(|_err| fail("lane does not fit in u32"))?;
            let combined = idx_u32.wrapping_mul(dim_u32).wrapping_add(lane_u32);
            let bits = combined.wrapping_add(1);
            let scaled = f32::from_bits(bits);
            let lane_value = if scaled.is_finite() { scaled } else { 0.0_f32 };
            vec.push(lane_value);
        }
        out.push((id, vec));
    }
    Ok(out)
}

fn records_as_refs(records: &[(String, Vec<f32>)]) -> Vec<(&str, Vec<f32>)> {
    records
        .iter()
        .map(|(id, vec)| (id.as_str(), vec.clone()))
        .collect()
}

fn tempdir() -> Result<TempDir, TestError> {
    TempDir::new().map_err(TestError::from)
}

#[test]
fn happy_path_build_then_open() -> Result<(), TestError> {
    let tmp = tempdir()?;
    let adapter = LanceSemanticAdapter::with_state_root(tmp.path());
    let manifest = make_manifest(7, true);
    let generation = make_generation_set(7);

    let records = make_records(10, 768)?;
    let encoded = encode_records(768, &records_as_refs(&records))?;

    adapter.build_semantic_index(
        &manifest,
        SemanticBuildInput {
            embedding_records: Some(&encoded),
        },
    )?;

    if !adapter.dataset_present(&generation) {
        return Err(fail("dataset_present must be true after build"));
    }
    adapter.open_vector_store(&generation)?;

    {
        let dataset_opt = adapter.open_dataset_for_query(&generation)?;
        if dataset_opt.is_none() {
            return Err(fail(
                "open_dataset_for_query returned None after successful build",
            ));
        }
    }

    let count = adapter
        .dataset_row_count(&generation)?
        .ok_or_else(|| fail("dataset_row_count returned None after successful build"))?;
    if count != 10 {
        return Err(fail(format!("expected 10 rows, got {count}")));
    }
    Ok(())
}

#[test]
fn open_dataset_for_query_returns_none_before_build() -> Result<(), TestError> {
    let tmp = tempdir()?;
    let adapter = LanceSemanticAdapter::with_state_root(tmp.path());
    let generation = make_generation_set(11);
    if adapter.open_dataset_for_query(&generation)?.is_some() {
        return Err(fail("dataset must be absent before any build"));
    }
    Ok(())
}

#[test]
fn open_dataset_for_query_returns_none_when_no_embeddings_built() -> Result<(), TestError> {
    let tmp = tempdir()?;
    let adapter = LanceSemanticAdapter::with_state_root(tmp.path());
    let manifest = make_manifest(3, false);
    let generation = make_generation_set(3);

    adapter.build_semantic_index(
        &manifest,
        SemanticBuildInput {
            embedding_records: None,
        },
    )?;

    if adapter.dataset_present(&generation) {
        return Err(fail(
            "dataset_present must be false for lexical-only generation",
        ));
    }
    adapter.open_vector_store(&generation)?;
    if adapter.open_dataset_for_query(&generation)?.is_some() {
        return Err(fail("lexical-only generation must yield None"));
    }
    Ok(())
}

#[test]
fn build_with_bad_magic_is_invalid_contract() -> Result<(), TestError> {
    let tmp = tempdir()?;
    let adapter = LanceSemanticAdapter::with_state_root(tmp.path());
    let manifest = make_manifest(2, true);

    let mut bytes = encode_records(4, &[("a", vec![0.0, 0.0, 0.0, 0.0])])?;
    let head_slice = bytes
        .get_mut(0..4)
        .ok_or_else(|| fail("encoded buffer shorter than 4 bytes"))?;
    head_slice.copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);

    let outcome = adapter.build_semantic_index(
        &manifest,
        SemanticBuildInput {
            embedding_records: Some(&bytes),
        },
    );
    match outcome {
        Err(CoreError::InvalidContract(msg)) if msg.contains("magic") => Ok(()),
        Err(other) => Err(fail(format!(
            "expected InvalidContract(magic), got {other:?}"
        ))),
        Ok(()) => Err(fail("expected InvalidContract, got Ok")),
    }
}

#[test]
fn build_with_dim_mismatch_is_invalid_contract() -> Result<(), TestError> {
    let tmp = tempdir()?;
    let adapter = LanceSemanticAdapter::with_state_root(tmp.path());
    let manifest = make_manifest(4, true);

    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&1u32.to_le_bytes());
    body.push(b'a');
    body.extend_from_slice(&0.0f32.to_le_bytes());
    body.extend_from_slice(&0.0f32.to_le_bytes());
    body.extend_from_slice(&0.0f32.to_le_bytes());
    let bytes = encode_with_header(1, 4, &body);

    match adapter.build_semantic_index(
        &manifest,
        SemanticBuildInput {
            embedding_records: Some(&bytes),
        },
    ) {
        Err(CoreError::InvalidContract(_)) => Ok(()),
        Err(other) => Err(fail(format!("expected InvalidContract, got {other:?}"))),
        Ok(()) => Err(fail("expected InvalidContract, got Ok")),
    }
}

#[test]
fn duplicate_build_is_idempotent() -> Result<(), TestError> {
    let tmp = tempdir()?;
    let adapter = LanceSemanticAdapter::with_state_root(tmp.path());
    let manifest = make_manifest(5, true);
    let generation = make_generation_set(5);
    let records = make_records(3, 4)?;
    let encoded = encode_records(4, &records_as_refs(&records))?;

    adapter.build_semantic_index(
        &manifest,
        SemanticBuildInput {
            embedding_records: Some(&encoded),
        },
    )?;
    if !adapter.dataset_present(&generation) {
        return Err(fail("dataset must be present after first build"));
    }

    adapter.build_semantic_index(
        &manifest,
        SemanticBuildInput {
            embedding_records: Some(&encoded),
        },
    )?;
    if !adapter.dataset_present(&generation) {
        return Err(fail("dataset must remain present after idempotent rebuild"));
    }
    Ok(())
}

#[test]
fn truncated_buffer_is_invalid_contract() -> Result<(), TestError> {
    let tmp = tempdir()?;
    let adapter = LanceSemanticAdapter::with_state_root(tmp.path());
    let manifest = make_manifest(6, true);

    let records = make_records(1, 4)?;
    let mut body: Vec<u8> = Vec::new();
    for (id, vec) in records_as_refs(&records) {
        let id_len = u32::try_from(id.len()).map_err(|_err| fail("id_len fits in u32"))?;
        body.extend_from_slice(&id_len.to_le_bytes());
        body.extend_from_slice(id.as_bytes());
        for lane in &vec {
            body.extend_from_slice(&lane.to_le_bytes());
        }
    }
    let bytes = encode_with_header(10, 4, &body);
    match adapter.build_semantic_index(
        &manifest,
        SemanticBuildInput {
            embedding_records: Some(&bytes),
        },
    ) {
        Err(CoreError::InvalidContract(_)) => Ok(()),
        Err(other) => Err(fail(format!("expected InvalidContract, got {other:?}"))),
        Ok(()) => Err(fail("expected InvalidContract, got Ok")),
    }
}

#[test]
fn empty_entity_id_is_invalid_contract() -> Result<(), TestError> {
    let tmp = tempdir()?;
    let adapter = LanceSemanticAdapter::with_state_root(tmp.path());
    let manifest = make_manifest(8, true);

    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..4 {
        body.extend_from_slice(&0.0f32.to_le_bytes());
    }
    let bytes = encode_with_header(1, 4, &body);

    match adapter.build_semantic_index(
        &manifest,
        SemanticBuildInput {
            embedding_records: Some(&bytes),
        },
    ) {
        Err(CoreError::InvalidContract(msg)) if msg.contains("entity_id_len must be > 0") => Ok(()),
        Err(other) => Err(fail(format!(
            "expected InvalidContract(entity_id_len), got {other:?}"
        ))),
        Ok(()) => Err(fail("expected InvalidContract, got Ok")),
    }
}

#[test]
fn open_vector_store_errors_when_marker_missing() -> Result<(), TestError> {
    let tmp = tempdir()?;
    let adapter = LanceSemanticAdapter::with_state_root(tmp.path());
    let generation = make_generation_set(9);

    let target = tmp
        .path()
        .join("semantic")
        .join(generation.manifest_generation.get().to_string());
    std::fs::create_dir_all(&target)?;

    match adapter.open_vector_store(&generation) {
        Err(CoreError::NotReady(_)) => Ok(()),
        Err(other) => Err(fail(format!("expected NotReady, got {other:?}"))),
        Ok(()) => Err(fail("expected NotReady, got Ok")),
    }
}
