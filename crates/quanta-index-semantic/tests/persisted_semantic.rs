//! Persisted semantic adapter behavior (LDB-01 layout/manifest + LDB-02
//! durable build/open). Exercises the real public port surface against a real
//! on-disk state root, so a green run is executable proof of durable open
//! without replay, not just source inspection.

use std::path::{Path, PathBuf};

use quanta_index_contract::{
    BatchIngestMode, EmbeddingDistanceMetric, EmbeddingId, EmbeddingModelContract,
    EmbeddingNormalization, EmbeddingRecord, ManifestGeneration, OwnerDocKind, RepoId,
    RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface, SemanticIngestBatch,
    SemanticReplaceScope, SemanticTombstoneScope, lex::LanguageCode,
};
use quanta_index_core::{CoreError, SemanticBatchBuildPort, SemanticIndexOpenPort};
use quanta_index_semantic::{SemanticAdapter, scan_persisted_generations};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn repo_id() -> RepoId {
    RepoId::new("repo-sem")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-sem")
}

fn model_contract(dimension: u32) -> EmbeddingModelContract {
    EmbeddingModelContract {
        model_id: "text-embed".to_string().into_boxed_str(),
        model_version: Some("1".to_string().into_boxed_str()),
        dimension,
        normalization: EmbeddingNormalization::L2Unit,
        distance_metric: EmbeddingDistanceMetric::Cosine,
        policy_digest: "policy:feed".to_string().into_boxed_str(),
        view_policy_digest: Some("view:feed".to_string().into_boxed_str()),
    }
}

fn scope(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn embedding_record(id: &str, path: &str, vector: Vec<f32>) -> Result<EmbeddingRecord, String> {
    let language = LanguageCode::new("rust")
        .map_err(|err| format!("fixture language `rust` must stay valid: {err}"))?;
    Ok(EmbeddingRecord {
        embedding_id: EmbeddingId::new(id),
        owner_kind: OwnerDocKind::Chunk,
        owner_id: format!("owner-{id}").into_boxed_str(),
        source_doc_id: format!("doc-{id}").into_boxed_str(),
        repo_relative_path: RepoRelativePath::new(path),
        language,
        symbol_kind: None,
        start_byte: 0,
        end_byte: 12,
        start_line: 1,
        end_line: 3,
        snippet: format!("fn {id}() {{}}").into_boxed_str(),
        embedding_input_digest: format!("input:{id}").into_boxed_str(),
        vector_digest: format!("vector:{id}").into_boxed_str(),
        view_kind: "raw_chunk".to_string().into_boxed_str(),
        vector,
    })
}

fn sealed_batch(
    generation: ManifestGeneration,
    path: &str,
    embeddings: Vec<EmbeddingRecord>,
    contract_dimension: u32,
) -> SemanticIngestBatch {
    SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: format!("manifest:{}", generation.get()),
        batch_digest: format!("batch:{}:{path}", generation.get()),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(contract_dimension),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope(path),
            scope_digest: format!("scope:{path}"),
            embeddings,
        }],
        tombstone_scopes: Vec::new(),
        seal: true,
    }
}

fn generation_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    root.join(repo_id().as_str())
        .join(revision_id().as_str())
        .join(format!("g{}", generation.get()))
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts durable build/open roundtrip via assert macros"
)]
fn build_open_roundtrip_serves_from_durable_state() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(7);
    let batch = sealed_batch(
        generation,
        "src/main.rs",
        vec![embedding_record(
            "emb-1",
            "src/main.rs",
            vec![1.0, 0.0, 0.0],
        )?],
        3,
    );

    adapter.build_batch(&batch)?;

    // Durable layout exists: manifest + both markers + dataset rows + graph.
    let dir = generation_dir(&root, generation);
    assert!(dir.join("semantic-manifest.cbor").exists());
    assert!(dir.join("MARKER_READY").exists());
    assert!(dir.join("MARKER_SEALED").exists());
    assert!(dir.join("dataset").join("rows.cbor").exists());
    assert!(dir.join("dataset").join("graph.cbor").exists());

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[1.0, 0.0, 0.0], 1)?;
    let Some(hit) = hits.first() else {
        return Err("durable semantic search must return one hit".into());
    };
    assert_eq!(hits.len(), 1);
    assert_eq!(hit.candidate_id.as_str(), "emb-1");
    assert_eq!(hit.repo_relative_path.as_str(), "src/main.rs");
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts a fresh adapter opens prior persisted state via assert macros"
)]
fn restart_opens_prior_generation_without_replay() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let generation = ManifestGeneration::new(4);
    {
        let writer = SemanticAdapter::with_state_root(root.clone());
        writer.build_batch(&sealed_batch(
            generation,
            "src/lib.rs",
            vec![embedding_record(
                "emb-a",
                "src/lib.rs",
                vec![0.0, 1.0, 0.0],
            )?],
            3,
        ))?;
    }

    // Brand new adapter instance: no in-memory carryover, no journal replay.
    let restarted = SemanticAdapter::with_state_root(root);
    let searcher = restarted.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[0.0, 1.0, 0.0], 3)?;
    let Some(hit) = hits.first() else {
        return Err("restarted adapter must serve the persisted generation".into());
    };
    assert_eq!(hit.candidate_id.as_str(), "emb-a");
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts replace overwrites same-path entries via assert macros"
)]
fn replace_scope_overwrites_same_path_entries() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(2);
    adapter.build_batch(&{
        let mut batch = sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        );
        batch.seal = false;
        batch
    })?;
    adapter.build_batch(&sealed_batch(
        generation,
        "src/main.rs",
        vec![embedding_record(
            "emb-2",
            "src/main.rs",
            vec![0.0, 1.0, 0.0],
        )?],
        3,
    ))?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[0.0, 1.0, 0.0], 5)?;
    let Some(hit) = hits.first() else {
        return Err("replacement search must return one hit".into());
    };
    assert_eq!(hits.len(), 1);
    assert_eq!(hit.candidate_id.as_str(), "emb-2");
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts tombstone removes prior embeddings via assert macros"
)]
fn tombstone_scope_removes_existing_entries() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf());
    let base = ManifestGeneration::new(1);
    let next = ManifestGeneration::new(2);
    adapter.build_batch(&sealed_batch(
        base,
        "src/main.rs",
        vec![embedding_record(
            "emb-1",
            "src/main.rs",
            vec![1.0, 0.0, 0.0],
        )?],
        3,
    ))?;
    // Delta to a NEW generation cloning the sealed base, then tombstone the path.
    adapter.build_batch(&SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: next,
        base_generation: Some(base),
        manifest_digest: "manifest:2".to_string(),
        batch_digest: "batch:tombstone".to_string(),
        mode: BatchIngestMode::Delta,
        model_contract: model_contract(3),
        replace_scopes: Vec::new(),
        tombstone_scopes: vec![SemanticTombstoneScope {
            scope: scope("src/main.rs"),
        }],
        seal: true,
    })?;

    let searcher = adapter.open(&repo_id(), &revision_id(), next)?;
    let hits = searcher.search(&[1.0, 0.0, 0.0], 5)?;
    assert!(hits.is_empty());
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts generation pin isolation via assert macros"
)]
fn generation_pin_isolates_results() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf());
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);
    adapter.build_batch(&sealed_batch(
        g1,
        "a.rs",
        vec![embedding_record("emb-a", "a.rs", vec![1.0, 0.0, 0.0])?],
        3,
    ))?;
    adapter.build_batch(&sealed_batch(
        g2,
        "b.rs",
        vec![embedding_record("emb-b", "b.rs", vec![0.0, 0.0, 1.0])?],
        3,
    ))?;

    let s1 = adapter.open(&repo_id(), &revision_id(), g1)?;
    let h1 = s1.search(&[1.0, 0.0, 0.0], 5)?;
    let ids1: Vec<String> = h1.iter().map(|c| c.candidate_id.clone()).collect();
    assert_eq!(ids1, vec!["emb-a".to_string()]);

    let s2 = adapter.open(&repo_id(), &revision_id(), g2)?;
    let h2 = s2.search(&[0.0, 0.0, 1.0], 5)?;
    let ids2: Vec<String> = h2.iter().map(|c| c.candidate_id.clone()).collect();
    assert_eq!(ids2, vec!["emb-b".to_string()]);
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts sealed-empty generation serves empty hits via assert macros"
)]
fn sealed_empty_generation_serves_empty_hits() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(5);
    adapter.build_batch(&SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:empty".to_string(),
        batch_digest: "batch:empty".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(3),
        replace_scopes: Vec::new(),
        tombstone_scopes: Vec::new(),
        seal: true,
    })?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[1.0, 0.0, 0.0], 1)?;
    assert!(hits.is_empty());
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts unsealed generation fails closed via assert macros"
)]
fn unsealed_generation_open_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(9);
    let mut batch = sealed_batch(
        generation,
        "src/main.rs",
        vec![embedding_record(
            "emb-1",
            "src/main.rs",
            vec![1.0, 0.0, 0.0],
        )?],
        3,
    );
    batch.seal = false;
    adapter.build_batch(&batch)?;

    let Err(err) = adapter.open(&repo_id(), &revision_id(), generation) else {
        return Err("unsealed generation must not open".into());
    };
    match err {
        CoreError::NotReady(message) => {
            assert!(message.contains("not sealed"));
        }
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => {
            return Err(format!("expected NotReady, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts contract dimension mismatch fails closed via assert macros"
)]
fn build_rejects_contract_dimension_mismatch() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf());
    let batch = sealed_batch(
        ManifestGeneration::new(1),
        "src/main.rs",
        vec![embedding_record(
            "emb-1",
            "src/main.rs",
            vec![1.0, 0.0, 0.0],
        )?],
        2,
    );

    let Err(err) = adapter.build_batch(&batch) else {
        return Err("contract dimension mismatch must fail closed".into());
    };
    match err {
        CoreError::InvalidContract(message) => {
            assert!(message.contains("embedding emb-1 dim 3 != contract dim 2"));
        }
        other @ (CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => {
            return Err(format!("expected InvalidContract, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts query dimension mismatch is typed via assert macros"
)]
fn query_dimension_mismatch_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf());
    let generation = ManifestGeneration::new(1);
    adapter.build_batch(&sealed_batch(
        generation,
        "src/main.rs",
        vec![embedding_record(
            "emb-1",
            "src/main.rs",
            vec![1.0, 0.0, 0.0],
        )?],
        3,
    ))?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let Err(err) = searcher.search(&[1.0, 0.0], 1) else {
        return Err("query dimension mismatch must fail closed".into());
    };
    match err {
        CoreError::Typed { code, .. } => {
            assert_eq!(code, "SEM_DIM_MISMATCH");
        }
        other @ (CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => {
            return Err(format!("expected typed SemDimMismatch, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts corrupted manifest fails closed via assert macros"
)]
fn corrupt_manifest_open_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(3);
    adapter.build_batch(&sealed_batch(
        generation,
        "src/main.rs",
        vec![embedding_record(
            "emb-1",
            "src/main.rs",
            vec![1.0, 0.0, 0.0],
        )?],
        3,
    ))?;

    let manifest_path = generation_dir(&root, generation).join("semantic-manifest.cbor");
    std::fs::write(&manifest_path, b"not-a-valid-cbor-manifest")?;

    // Fresh adapter to dodge the open cache.
    let reopened = SemanticAdapter::with_state_root(root);
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("corrupt manifest must fail closed".into());
    };
    assert!(matches!(err, CoreError::Storage(_)));
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts tampered dataset fails closed via assert macros"
)]
fn tampered_dataset_open_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone());
    let generation = ManifestGeneration::new(6);
    adapter.build_batch(&sealed_batch(
        generation,
        "src/main.rs",
        vec![embedding_record(
            "emb-1",
            "src/main.rs",
            vec![1.0, 0.0, 0.0],
        )?],
        3,
    ))?;

    let rows_path = generation_dir(&root, generation)
        .join("dataset")
        .join("rows.cbor");
    let mut bytes = std::fs::read(&rows_path)?;
    if let Some(last) = bytes.last_mut() {
        *last ^= 0xFF;
    }
    std::fs::write(&rows_path, &bytes)?;

    let reopened = SemanticAdapter::with_state_root(root);
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("tampered dataset must fail closed".into());
    };
    assert!(matches!(err, CoreError::Storage(_)));
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts generation scan reports sealed generations via assert macros"
)]
fn scan_reports_sealed_generations_only() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let semantic_root = root.join("indexes").join("semantic");
    let adapter = SemanticAdapter::with_state_root(semantic_root.clone());

    adapter.build_batch(&sealed_batch(
        ManifestGeneration::new(1),
        "a.rs",
        vec![embedding_record("emb-a", "a.rs", vec![1.0, 0.0, 0.0])?],
        3,
    ))?;
    // Materialized-but-unsealed generation must NOT be reported.
    let mut unsealed = sealed_batch(
        ManifestGeneration::new(2),
        "b.rs",
        vec![embedding_record("emb-b", "b.rs", vec![0.0, 1.0, 0.0])?],
        3,
    );
    unsealed.seal = false;
    adapter.build_batch(&unsealed)?;

    let scanned = scan_persisted_generations(&semantic_root)?;
    assert_eq!(scanned.len(), 1);
    let Some(record) = scanned.first() else {
        return Err("scan must report the sealed generation".into());
    };
    assert_eq!(record.generation, ManifestGeneration::new(1));
    assert_eq!(record.manifest_digest, "manifest:1");
    assert_eq!(record.repo_id.as_str(), "repo-sem");
    Ok(())
}
