//! Persisted semantic adapter behavior (LDB-01 layout/manifest + LDB-02
//! durable build/open). Exercises the real public port surface against a real
//! on-disk state root, so a green run is executable proof of durable open
//! without replay, not just source inspection.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, EmbeddingDistanceMetric, EmbeddingModelContract, EmbeddingNormalization,
    EmbeddingRecord, ExactRepoRelativePathV1, ManifestGeneration, OwnerDocKind,
    QueryConstraintSetV1, RepoId, RevisionId, SearchScopeKey, SemanticCorpusKindV1,
    SemanticIngestBatch, SemanticReplaceScope,
};
use quanta_index_core::{
    CoreError, GenerationQuarantineReasonV1, GenerationStorageKeyV1, L2_UNIT_NORM_TOLERANCE,
    RequestBudgetV1, SemanticIndexOpenPort,
};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, embedding_record_v1, inventory_persisted_generations,
    legacy_chunk_embedding_record_v1, model_contract_v1, sealed_replace_batch_v1, search_scope_v1,
    test_support, tombstone_scope_v1, validate_persisted_generation_v2,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn repo_id() -> RepoId {
    RepoId::new("repo-sem").expect("static fixture ID satisfies canonical policy")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-sem").expect("static fixture ID satisfies canonical policy")
}

fn model_contract(dimension: u32) -> EmbeddingModelContract {
    model_contract_v1(dimension)
}

fn scope(path: &str) -> SearchScopeKey {
    search_scope_v1(path)
}

fn embedding_record(id: &str, path: &str, vector: Vec<f32>) -> Result<EmbeddingRecord, String> {
    legacy_chunk_embedding_record_v1(id, path, vector)
}

fn sealed_batch(
    generation: ManifestGeneration,
    path: &str,
    embeddings: Vec<EmbeddingRecord>,
    contract_dimension: u32,
) -> SemanticIngestBatch {
    sealed_replace_batch_v1(
        repo_id(),
        revision_id(),
        generation,
        path,
        embeddings,
        contract_dimension,
    )
}

fn embedding_record_same_owner(
    id: &str,
    path: &str,
    owner_id: &str,
    vector: Vec<f32>,
) -> Result<EmbeddingRecord, String> {
    let mut record = legacy_chunk_embedding_record_v1(id, path, vector)?;
    record.owner_id = owner_id.to_string().into_boxed_str();
    record.record_id = format!("record-{owner_id}").into_boxed_str();
    Ok(record)
}

/// The sealed manifest's file commitment refuses a forged or missing sidecar
/// before the manifest is interpreted (QI-BB-017), so every forgery below is
/// answered by the same typed code naming the file.
fn expect_sidecar_corrupt(err: CoreError, expected_file: &str) -> TestResult {
    match err {
        CoreError::Typed { code, message }
            if code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt =>
        {
            if !message.contains(expected_file) {
                return Err(format!("sidecar refusal must name {expected_file}: {message}").into());
            }
            Ok(())
        }
        other @ (CoreError::Typed { .. }
        | CoreError::Storage(_)
        | CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)) => {
            Err(format!("expected GENERATION_SIDECAR_CORRUPT, got {other:?}").into())
        }
    }
}

fn generation_dir(root: &Path, generation: ManifestGeneration) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&repo_id(), &revision_id())
        .generation_dir(root, generation)
}

/// Pure exhaustive cosine oracle for the constrained query contract.
///
/// This intentionally does not share the `LanceDB` predicate or ranking
/// implementation: it filters the fixture records in memory, computes cosine
/// similarity directly, and applies the stable identity tie-break.
fn exhaustive_constrained_cosine_oracle(
    query: &[f32],
    records: &[EmbeddingRecord],
    allowed_ids: &BTreeSet<String>,
    constraints: &QueryConstraintSetV1,
) -> Result<Vec<(String, f64)>, String> {
    let query_norm_squared = query
        .iter()
        .map(|value| {
            let value = f64::from(*value);
            value * value
        })
        .sum::<f64>();
    if query_norm_squared == 0.0 {
        return Err("oracle query vector must be non-zero".to_string());
    }

    let mut ranked = records
        .iter()
        .filter(|record| allowed_ids.contains(record.embedding_id.as_str()))
        .filter(|record| {
            constraints.language_any_of.is_empty()
                || constraints.language_any_of.contains(&record.language)
        })
        .map(|record| {
            if record.vector.len() != query.len() {
                return Err(format!(
                    "oracle dimension mismatch for {}: {} != {}",
                    record.embedding_id.as_str(),
                    record.vector.len(),
                    query.len()
                ));
            }
            let dot_product = query
                .iter()
                .zip(&record.vector)
                .map(|(left, right)| f64::from(*left) * f64::from(*right))
                .sum::<f64>();
            let record_norm_squared = record
                .vector
                .iter()
                .map(|value| {
                    let value = f64::from(*value);
                    value * value
                })
                .sum::<f64>();
            if record_norm_squared == 0.0 {
                return Err(format!(
                    "oracle stored vector must be non-zero for {}",
                    record.embedding_id.as_str()
                ));
            }
            Ok((
                record.embedding_id.as_str().to_string(),
                dot_product / (query_norm_squared * record_norm_squared).sqrt(),
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    ranked.sort_by(|(left_id, left_score), (right_id, right_score)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left_id.cmp(right_id))
    });
    Ok(ranked)
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts durable build/open roundtrip via assert macros"
)]
fn build_open_roundtrip_serves_from_durable_state() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
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

    build_resident_batch_v1(&adapter, &batch)?;

    // Durable layout exists: manifest + both markers + a non-empty lancedb
    // dataset dir (lancedb manages its own files under `dataset/`; we assert
    // presence + non-emptiness rather than naming specific files).
    let dir = generation_dir(&root, generation);
    assert!(dir.join("semantic-build-contract.cbor").exists());
    assert!(dir.join("semantic-manifest.cbor").exists());
    assert!(dir.join("MARKER_READY").exists());
    assert!(dir.join("MARKER_SEALED").exists());
    let dataset_dir = dir.join("dataset");
    assert!(dataset_dir.is_dir(), "lancedb dataset dir must exist");
    let dataset_entries = std::fs::read_dir(&dataset_dir)?.count();
    assert!(
        dataset_entries > 0,
        "lancedb dataset dir must contain at least one file/subdir after seal"
    );

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[1.0, 0.0, 0.0], 1, &RequestBudgetV1::unbounded())?;
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
    reason = "test asserts constraint pushdown across the persisted vector engine"
)]
fn language_constraint_is_applied_before_vector_limit_and_composes_with_scope_v1() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(8);
    let mut python = embedding_record_v1(
        "python-best",
        "src/shared.rs",
        OwnerDocKind::Chunk,
        "owner-python",
        SemanticCorpusKindV1::RawCodeFallback,
        vec![1.0, 0.0, 0.0],
    )?;
    python.language = LanguageCode::new("python").map_err(str::to_string)?;
    let rust = embedding_record_v1(
        "rust-target",
        "src/shared.rs",
        OwnerDocKind::Chunk,
        "owner-rust",
        SemanticCorpusKindV1::RawCodeFallback,
        vec![0.8, 0.2, 0.0],
    )?;
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(generation, "src/shared.rs", vec![python, rust], 3),
    )?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let rust_only =
        QueryConstraintSetV1::from_languages([LanguageCode::new("rust").map_err(str::to_string)?]);
    let hits = searcher.search_constrained(
        &[1.0, 0.0, 0.0],
        &rust_only,
        1,
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(
        hits.first().map(|hit| hit.candidate_id.as_str()),
        Some("rust-target"),
        "post-limit filtering would lose the lower-scoring allowed-language row"
    );

    let allowed_ids = BTreeSet::from(["rust-target".to_string(), "python-best".to_string()]);
    let scoped = searcher.search_scoped_constrained(
        &[1.0, 0.0, 0.0],
        &allowed_ids,
        &rust_only,
        1,
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(
        scoped.first().map(|hit| hit.candidate_id.as_str()),
        Some("rust-target")
    );
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts exact-path pushdown across the persisted vector engine"
)]
fn exact_path_constraint_is_applied_before_vector_limit_v1() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(82);
    build_resident_batch_v1(
        &adapter,
        &SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:82".to_string(),
            batch_digest: "batch:82".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(3),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![
                SemanticReplaceScope {
                    scope: scope("other/lib.rs"),
                    scope_digest: "scope:other-lib".to_string(),
                    embeddings: vec![embedding_record(
                        "wrong-best",
                        "other/lib.rs",
                        vec![1.0, 0.0, 0.0],
                    )?],
                    cluster_memberships: Vec::new(),
                },
                SemanticReplaceScope {
                    scope: scope("src/lib.rs"),
                    scope_digest: "scope:src-lib".to_string(),
                    embeddings: vec![embedding_record(
                        "requested",
                        "src/lib.rs",
                        vec![0.8, 0.2, 0.0],
                    )?],
                    cluster_memberships: Vec::new(),
                },
            ],
            tombstone_scopes: Vec::new(),
            seal: true,
        },
    )?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
        ExactRepoRelativePathV1::new("src/lib.rs").map_err(str::to_string)?,
    );
    let hits = searcher.search_constrained(
        &[1.0, 0.0, 0.0],
        &constraints,
        1,
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(
        hits.first().map(|hit| hit.candidate_id.as_str()),
        Some("requested"),
        "post-limit filtering would lose the lower-scoring exact-path row"
    );
    assert_eq!(hits.len(), 1);
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts a real LanceDB query against an independent exhaustive cosine oracle"
)]
fn constrained_vector_search_matches_exhaustive_oracle_and_top_k_prefix_v1() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(81);
    let query = vec![1.0, 0.0, 0.0];

    let python_nearest = {
        let mut record = embedding_record_v1(
            "python-nearest",
            "src/oracle.py",
            OwnerDocKind::Chunk,
            "owner-python",
            SemanticCorpusKindV1::RawCodeFallback,
            vec![1.0, 0.0, 0.0],
        )?;
        record.language = LanguageCode::new("python").map_err(str::to_string)?;
        record
    };
    let records = vec![
        python_nearest,
        embedding_record_v1(
            "rust-best",
            "src/oracle.rs",
            OwnerDocKind::Chunk,
            "owner-rust-best",
            SemanticCorpusKindV1::RawCodeFallback,
            vec![0.8, 0.6, 0.0],
        )?,
        embedding_record_v1(
            "rust-second",
            "src/oracle.rs",
            OwnerDocKind::Chunk,
            "owner-rust-second",
            SemanticCorpusKindV1::RawCodeFallback,
            vec![0.6, 0.8, 0.0],
        )?,
        embedding_record_v1(
            "rust-out-of-scope",
            "src/outside.rs",
            OwnerDocKind::Chunk,
            "owner-rust-outside",
            SemanticCorpusKindV1::RawCodeFallback,
            vec![0.99, 0.1, 0.0],
        )?,
        embedding_record_v1(
            "rust-irrelevant",
            "src/oracle.rs",
            OwnerDocKind::Chunk,
            "owner-rust-irrelevant",
            SemanticCorpusKindV1::RawCodeFallback,
            vec![0.0, 1.0, 0.0],
        )?,
    ];
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(generation, "src/oracle.rs", records.clone(), 3),
    )?;

    let constraints =
        QueryConstraintSetV1::from_languages([LanguageCode::new("rust").map_err(str::to_string)?]);
    let allowed_ids = BTreeSet::from([
        "python-nearest".to_string(),
        "rust-best".to_string(),
        "rust-second".to_string(),
        "rust-irrelevant".to_string(),
    ]);
    let expected =
        exhaustive_constrained_cosine_oracle(&query, &records, &allowed_ids, &constraints)
            .map_err(|err| format!("exhaustive cosine oracle: {err}"))?;
    assert_eq!(
        expected
            .iter()
            .map(|(id, _score)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["rust-best", "rust-second", "rust-irrelevant"],
        "the pure oracle must exclude both the closer python row and the closer out-of-scope row"
    );

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    for top_k in 1..=expected.len() + 1 {
        let top_k = u32::try_from(top_k).map_err(|err| format!("top_k conversion: {err}"))?;
        let actual = searcher.search_scoped_constrained(
            &query,
            &allowed_ids,
            &constraints,
            top_k,
            &RequestBudgetV1::unbounded(),
        )?;
        let expected_prefix = expected
            .get(..actual.len())
            .ok_or("oracle produced fewer rows than the adapter returned")?;
        assert_eq!(
            actual
                .iter()
                .map(|hit| hit.candidate_id.as_str())
                .collect::<Vec<_>>(),
            expected_prefix
                .iter()
                .map(|(id, _score)| id.as_str())
                .collect::<Vec<_>>(),
            "top_k={top_k} must return the exhaustive constrained prefix"
        );
        for (hit, (_id, expected_score)) in actual.iter().zip(expected_prefix) {
            assert!(
                (f64::from(hit.score) - expected_score).abs() < 0.000_1,
                "{} score {} differs from exhaustive cosine {expected_score}",
                hit.candidate_id,
                hit.score
            );
        }
    }
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
        let writer = SemanticAdapter::with_state_root(root.clone())?;
        build_resident_batch_v1(
            &writer,
            &sealed_batch(
                generation,
                "src/lib.rs",
                vec![embedding_record(
                    "emb-a",
                    "src/lib.rs",
                    vec![0.0, 1.0, 0.0],
                )?],
                3,
            ),
        )?;
    }

    // Brand new adapter instance: no in-memory carryover, no journal replay.
    let restarted = SemanticAdapter::with_state_root(root)?;
    let searcher = restarted.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[0.0, 1.0, 0.0], 3, &RequestBudgetV1::unbounded())?;
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
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(2);
    build_resident_batch_v1(&adapter, &{
        let mut batch = sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record_same_owner(
                "emb-1",
                "src/main.rs",
                "owner-main",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        );
        batch.seal = false;
        batch
    })?;
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record_same_owner(
                "emb-2",
                "src/main.rs",
                "owner-main",
                vec![0.0, 1.0, 0.0],
            )?],
            3,
        ),
    )?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[0.0, 1.0, 0.0], 5, &RequestBudgetV1::unbounded())?;
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
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let next = ManifestGeneration::new(2);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            base,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;
    // Delta to a NEW generation cloning the sealed base, then tombstone the path.
    build_resident_batch_v1(
        &adapter,
        &SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: next,
            base_generation: Some(base),
            manifest_digest: "manifest:2".to_string(),
            batch_digest: "batch:tombstone".to_string(),
            mode: BatchIngestMode::Delta,
            model_contract: model_contract(3),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: vec![tombstone_scope_v1("src/main.rs")],
            seal: true,
        },
    )?;

    let searcher = adapter.open(&repo_id(), &revision_id(), next)?;
    let hits = searcher.search(&[1.0, 0.0, 0.0], 5, &RequestBudgetV1::unbounded())?;
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
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let g1 = ManifestGeneration::new(1);
    let g2 = ManifestGeneration::new(2);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            g1,
            "a.rs",
            vec![embedding_record("emb-a", "a.rs", vec![1.0, 0.0, 0.0])?],
            3,
        ),
    )?;
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            g2,
            "b.rs",
            vec![embedding_record("emb-b", "b.rs", vec![0.0, 0.0, 1.0])?],
            3,
        ),
    )?;

    let s1 = adapter.open(&repo_id(), &revision_id(), g1)?;
    let h1 = s1.search(&[1.0, 0.0, 0.0], 5, &RequestBudgetV1::unbounded())?;
    let ids1: Vec<String> = h1.iter().map(|c| c.candidate_id.clone()).collect();
    assert_eq!(ids1, vec!["emb-a".to_string()]);

    let s2 = adapter.open(&repo_id(), &revision_id(), g2)?;
    let h2 = s2.search(&[0.0, 0.0, 1.0], 5, &RequestBudgetV1::unbounded())?;
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
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(5);
    build_resident_batch_v1(
        &adapter,
        &SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:empty".to_string(),
            batch_digest: "batch:empty".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(3),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        },
    )?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[1.0, 0.0, 0.0], 1, &RequestBudgetV1::unbounded())?;
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
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
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
    build_resident_batch_v1(&adapter, &batch)?;

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
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
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

    let Err(err) = build_resident_batch_v1(&adapter, &batch) else {
        return Err("contract dimension mismatch must fail closed".into());
    };
    // The vector contract is enforced by the shared validator (QI-BB-031),
    // so a dimension mismatch is the typed vector code naming the embedding.
    match err {
        CoreError::Typed { code, message } => {
            assert_eq!(
                code,
                quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::SemInvalidVector,
                )
            );
            assert!(
                message.contains("3 components, contract dimension is 2")
                    && message.contains("emb-1"),
                "{message}"
            );
        }
        other @ (CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => {
            return Err(format!("expected SEM_INVALID_VECTOR, got {other:?}").into());
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
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let Err(err) = searcher.search(&[1.0, 0.0], 1, &RequestBudgetV1::unbounded()) else {
        return Err("query dimension mismatch must fail closed".into());
    };
    match err {
        CoreError::Typed { code, .. } => {
            assert_eq!(
                code,
                quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::SemDimMismatch,
                )
            );
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

fn typed_code<T>(result: &Result<T, CoreError>) -> Option<String> {
    match result {
        Err(CoreError::Typed { code, .. }) => Some(code.to_string()),
        _ => None,
    }
}

/// Vectors an `L2Unit` contract of dimension 3 refuses, as a query and as
/// an ingested row alike.
fn off_contract_unit_vectors() -> [(&'static str, Vec<f32>); 5] {
    [
        ("half norm", vec![0.5, 0.0, 0.0]),
        ("double norm", vec![2.0, 0.0, 0.0]),
        ("zero", vec![0.0, 0.0, 0.0]),
        ("nan", vec![f32::NAN, 0.0, 0.0]),
        ("inf", vec![f32::INFINITY, 0.0, 0.0]),
    ]
}

/// Every query vector is held to the contract the generation's rows were
/// held to at ingest (QI-BB-031).
///
/// Under `L2Unit` a unit query serves; a vector of norm 0.5 or 2.0, a zero
/// vector and a non-finite one are refused `SEM_INVALID_VECTOR` by every
/// query entry — neighbour search, hits, candidate score — and the same
/// vector as an ingested row is refused under the same code, so the corpus
/// side and the query side agree. The wrong dimension stays
/// `SEM_DIM_MISMATCH`.
#[test]
fn query_vectors_are_held_to_the_generations_normalization() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![
                embedding_record("emb-1", "src/main.rs", vec![1.0, 0.0, 0.0])?,
                embedding_record("emb-2", "src/main.rs", vec![0.0, 1.0, 0.0])?,
            ],
            3,
        ),
    )?;
    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let unbounded = RequestBudgetV1::unbounded();
    let served = searcher.search(&[0.6, 0.8, 0.0], 2, &unbounded)?;
    if served.len() != 2 {
        return Err(format!("a unit query must serve both rows, served {}", served.len()).into());
    }
    let _score = searcher.score_candidate("emb-1", &[0.6, 0.8, 0.0], &unbounded)?;

    for (index, (label, vector)) in (2_u64..).zip(off_contract_unit_vectors()) {
        for (entry, code) in [
            (
                "search",
                typed_code(&searcher.search(&vector, 2, &unbounded)),
            ),
            (
                "hits",
                typed_code(&searcher.search_hits(&vector, 2, &unbounded)),
            ),
            (
                "score",
                typed_code(&searcher.score_candidate("emb-1", &vector, &unbounded)),
            ),
        ] {
            if code.as_deref() != Some("SEM_INVALID_VECTOR") {
                return Err(format!("a {label} query via {entry} answered {code:?}").into());
            }
        }
        let mut row = embedding_record("emb-off", "src/off.rs", vec![1.0, 0.0, 0.0])?;
        row.vector.clone_from(&vector);
        let ingested = build_resident_batch_v1(
            &adapter,
            &sealed_batch(ManifestGeneration::new(index), "src/off.rs", vec![row], 3),
        );
        if typed_code(&ingested).as_deref() != Some("SEM_INVALID_VECTOR") {
            return Err(
                format!("a {label} row must be refused like the query: {ingested:?}").into(),
            );
        }
    }
    match typed_code(&searcher.search(&[1.0, 0.0], 2, &unbounded)).as_deref() {
        Some("SEM_DIM_MISMATCH") => Ok(()),
        other => Err(format!("a two-component query answered {other:?}").into()),
    }
}

/// A raw (`None`) generation holds its query vectors to what its rows were
/// held to: finite, non-zero, the right dimension — a norm of 2.0 serves,
/// a zero or non-finite vector does not (QI-BB-031).
#[test]
fn a_raw_generation_serves_any_finite_non_zero_query() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    let mut row = embedding_record("emb-1", "src/main.rs", vec![1.0, 0.0, 0.0])?;
    row.vector = vec![2.0, 0.0, 0.0];
    let mut batch = sealed_batch(generation, "src/main.rs", vec![row], 3);
    batch.model_contract.normalization = EmbeddingNormalization::None;
    build_resident_batch_v1(&adapter, &batch)?;
    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let unbounded = RequestBudgetV1::unbounded();
    if searcher.search(&[2.0, 0.0, 0.0], 1, &unbounded)?.len() != 1 {
        return Err("a raw generation must serve a finite non-zero query".into());
    }
    for (label, vector) in [
        ("zero", vec![0.0, 0.0, 0.0]),
        ("nan", vec![f32::NAN, 0.0, 0.0]),
    ] {
        let code = typed_code(&searcher.search(&vector, 1, &unbounded));
        if code.as_deref() != Some("SEM_INVALID_VECTOR") {
            return Err(format!("a {label} query on a raw generation answered {code:?}").into());
        }
    }
    Ok(())
}

/// Every persisted row of an `L2Unit` generation is a unit vector, and the
/// manifest names the policy (QI-BB-031 완료 기준 #2).
///
/// The rows come from directions of every scale and are read back straight
/// from the sealed dataset, past the adapter; the policy is decoded from
/// the scope manifest's own bytes.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "test-only direct library read of the sealed dataset; the sync seam is the test's own runtime"
)]
fn every_persisted_row_of_an_l2_unit_generation_is_unit() -> TestResult {
    use arrow_array::{Array, FixedSizeListArray, Float32Array};
    use futures::TryStreamExt;
    use lancedb::query::ExecutableQuery;

    const ROWS: u32 = 40;
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    let embeddings = (0..ROWS)
        .map(|row| {
            let scale = f32::from(u16::try_from(row)?).mul_add(0.37, 0.01);
            let tilt = f32::from(u16::try_from(row % 7)?) - 3.0;
            embedding_record(&format!("emb-{row}"), "src/main.rs", vec![scale, tilt, 0.5])
                .map_err(|err| -> Box<dyn std::error::Error> { err.into() })
        })
        .collect::<Result<Vec<_>, _>>()?;
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(generation, "src/main.rs", embeddings, 3),
    )?;

    let dir = generation_dir(temp.path(), generation);
    let manifest: ciborium::value::Value =
        ciborium::from_reader(std::fs::read(dir.join("semantic-manifest.cbor"))?.as_slice())?;
    let normalization = manifest
        .as_map()
        .and_then(|entries| {
            entries
                .iter()
                .find(|(key, _value)| key.as_text() == Some("normalization"))
        })
        .and_then(|(_key, value)| value.as_text());
    if normalization != Some("l2_unit") {
        return Err(format!("the manifest must name l2_unit, names {normalization:?}").into());
    }

    let uri = dir.join("dataset").to_string_lossy().into_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let batches = runtime.block_on(async {
        let connection = lancedb::connect(&uri)
            .execute()
            .await
            .map_err(|err| format!("connect: {err}"))?;
        let table = connection
            .open_table("semantic")
            .execute()
            .await
            .map_err(|err| format!("open_table: {err}"))?;
        let stream = table
            .query()
            .execute()
            .await
            .map_err(|err| format!("query: {err}"))?;
        stream
            .try_collect::<Vec<_>>()
            .await
            .map_err(|err| format!("read rows: {err}"))
    })?;
    let mut seen = 0_u32;
    for batch in &batches {
        let vectors = batch
            .column_by_name("vector")
            .and_then(|column| column.as_any().downcast_ref::<FixedSizeListArray>())
            .ok_or("the dataset stores a fixed-size vector column")?;
        for row in vectors.iter() {
            let row = row.ok_or("a stored vector is null")?;
            let components = row
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or("a stored vector is f32")?;
            let norm = components
                .values()
                .iter()
                .map(|component| f64::from(*component) * f64::from(*component))
                .sum::<f64>()
                .sqrt();
            if (norm - 1.0).abs() > L2_UNIT_NORM_TOLERANCE {
                return Err(format!("a persisted row has norm {norm}").into());
            }
            seen = seen.saturating_add(1);
        }
    }
    if seen != ROWS {
        return Err(format!("read {seen} persisted rows, expected {ROWS}").into());
    }
    Ok(())
}

#[test]
fn corrupt_manifest_open_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(3);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let manifest_path = generation_dir(&root, generation).join("semantic-manifest.cbor");
    std::fs::write(&manifest_path, b"not-a-valid-cbor-manifest")?;

    // Fresh adapter to dodge the open cache.
    let reopened = SemanticAdapter::with_state_root(root)?;
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("corrupt manifest must fail closed".into());
    };
    expect_sidecar_corrupt(err, "semantic-manifest.cbor")
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts a missing lancedb dataset fails closed via assert macros"
)]
fn missing_lancedb_dataset_open_fails_closed() -> TestResult {
    // Build a sealed generation, then delete its entire lancedb dataset
    // directory. Open must fail closed (the seal marker still says the gen is
    // sealed, but the underlying lancedb table is gone — a corruption signal).
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(6);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let dataset_dir = generation_dir(&root, generation).join("dataset");
    std::fs::remove_dir_all(&dataset_dir)?;

    let reopened = SemanticAdapter::with_state_root(root)?;
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("missing lancedb dataset must fail closed".into());
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
    let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;

    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            ManifestGeneration::new(1),
            "a.rs",
            vec![embedding_record("emb-a", "a.rs", vec![1.0, 0.0, 0.0])?],
            3,
        ),
    )?;
    // Materialized-but-unsealed generation must NOT be reported.
    let mut unsealed = sealed_batch(
        ManifestGeneration::new(2),
        "b.rs",
        vec![embedding_record("emb-b", "b.rs", vec![0.0, 1.0, 0.0])?],
        3,
    );
    unsealed.seal = false;
    build_resident_batch_v1(&adapter, &unsealed)?;

    let inventory = inventory_persisted_generations(&semantic_root)?;
    assert!(
        inventory.quarantined.is_empty(),
        "{:?}",
        inventory.quarantined
    );
    let scanned = inventory.sealed;
    assert_eq!(scanned.len(), 1);
    let Some(record) = scanned.first() else {
        return Err("inventory must report the sealed generation".into());
    };
    assert_eq!(record.generation, ManifestGeneration::new(1));
    assert_eq!(record.manifest_digest, "manifest:1");
    assert_eq!(record.repo_id.as_str(), "repo-sem");
    Ok(())
}

/// A tampered manifest row count is a content defect (QI-BB-026).
///
/// The inventory still lists the generation (it reads identities, not
/// content), and the deep witness that boot no longer runs for every
/// generation refuses it with the row-count integrity error.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts corrupted sealed generations fail closed at the deep witness via assert macros"
)]
fn inventory_lists_a_content_corrupted_generation_and_the_deep_witness_refuses_it() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let semantic_root = root.join("indexes").join("semantic");
    let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
    let generation = ManifestGeneration::new(20);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "scan.rs",
            vec![embedding_record(
                "emb-scan",
                "scan.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let manifest_path = generation_dir(&semantic_root, generation).join("semantic-manifest.cbor");
    let manifest_bytes = std::fs::read(&manifest_path)?;
    let mut value: ciborium::value::Value = ciborium::from_reader(&manifest_bytes[..])
        .map_err(|err| format!("decode manifest cbor: {err}"))?;
    let mut bumped = false;
    if let ciborium::value::Value::Map(entries) = &mut value {
        for (key, val) in entries.iter_mut() {
            if key.as_text() == Some("row_count") {
                *val = ciborium::value::Value::Integer(ciborium::value::Integer::from(99_u64));
                bumped = true;
                break;
            }
        }
    }
    if !bumped {
        return Err("expected `row_count` field in manifest CBOR".into());
    }
    let mut tampered = Vec::new();
    ciborium::into_writer(&value, &mut tampered)
        .map_err(|err| format!("encode manifest cbor: {err}"))?;
    std::fs::write(&manifest_path, &tampered)?;

    let inventory = inventory_persisted_generations(&semantic_root)?;
    assert!(
        inventory.quarantined.is_empty(),
        "{:?}",
        inventory.quarantined
    );
    let Some(record) = inventory
        .sealed
        .iter()
        .find(|record| record.generation == generation)
    else {
        return Err("the inventory must still list a content-corrupted generation".into());
    };
    let Err(err) = validate_persisted_generation_v2(&semantic_root, record) else {
        return Err("the deep witness must refuse the corrupted generation".into());
    };
    expect_sidecar_corrupt(err, "semantic-manifest.cbor")
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts scoped search honors the allowlist via assert macros"
)]
fn search_scoped_restricts_to_allowlist() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);
    build_resident_batch_v1(
        &adapter,
        &SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:1".to_string(),
            batch_digest: "batch:1".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(3),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: scope("x.rs"),
                scope_digest: "scope:x".to_string(),
                embeddings: vec![
                    embedding_record("emb-1", "x.rs", vec![1.0, 0.0, 0.0])?,
                    embedding_record("emb-2", "x.rs", vec![0.9, 0.1, 0.0])?,
                ],
                cluster_memberships: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        },
    )?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let mut allow: BTreeSet<String> = BTreeSet::new();
    let _new = allow.insert("emb-2".to_string());
    // The global nearest to [1,0,0] is emb-1, but the allowlist excludes it.
    let hits =
        searcher.search_scoped(&[1.0, 0.0, 0.0], &allow, 5, &RequestBudgetV1::unbounded())?;
    let ids: Vec<String> = hits.iter().map(|c| c.candidate_id.clone()).collect();
    assert_eq!(ids, vec!["emb-2".to_string()]);
    Ok(())
}

#[test]
fn manifest_row_count_mismatch_fails_closed() -> TestResult {
    // Build a sealed generation, then re-encode the manifest with a wrong
    // `row_count` so it stays **valid CBOR + valid scope** but disagrees with
    // the live lancedb table's actual row count. This specifically exercises
    // the cross-check at search.rs `live_row_count_u64 != manifest.row_count`,
    // not the generic decode/scope-validation paths covered by other tests.
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(8);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let manifest_path = generation_dir(&root, generation).join("semantic-manifest.cbor");
    let manifest_bytes = std::fs::read(&manifest_path)?;
    let mut value: ciborium::value::Value = ciborium::from_reader(&manifest_bytes[..])
        .map_err(|err| format!("decode manifest cbor: {err}"))?;
    let mut bumped = false;
    if let ciborium::value::Value::Map(entries) = &mut value {
        for (key, val) in entries.iter_mut() {
            if key.as_text() == Some("row_count") {
                *val = ciborium::value::Value::Integer(ciborium::value::Integer::from(99_u64));
                bumped = true;
                break;
            }
        }
    }
    if !bumped {
        return Err("expected `row_count` field in manifest CBOR".into());
    }
    let mut tampered = Vec::new();
    ciborium::into_writer(&value, &mut tampered)
        .map_err(|err| format!("encode manifest cbor: {err}"))?;
    std::fs::write(&manifest_path, &tampered)?;

    let reopened = SemanticAdapter::with_state_root(root)?;
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("manifest row_count mismatch must fail closed".into());
    };
    expect_sidecar_corrupt(err, "semantic-manifest.cbor")
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts a delta with an absent base fails closed via assert macros"
)]
fn delta_with_missing_base_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let batch = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: ManifestGeneration::new(10),
        base_generation: Some(ManifestGeneration::new(99)),
        manifest_digest: "manifest:10".to_string(),
        batch_digest: "batch:10".to_string(),
        mode: BatchIngestMode::Delta,
        model_contract: model_contract(3),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope("x.rs"),
            scope_digest: "scope:x".to_string(),
            embeddings: vec![embedding_record("emb-1", "x.rs", vec![1.0, 0.0, 0.0])?],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: true,
    };

    let Err(err) = build_resident_batch_v1(&adapter, &batch) else {
        return Err("delta with absent base must fail closed".into());
    };
    match err {
        CoreError::NotReady(message) => {
            assert!(message.contains("delta base generation 99 is absent"));
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
    reason = "test asserts replace-generation batches reject base_generation via assert macros"
)]
fn replace_generation_with_base_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let mut batch = sealed_batch(
        ManifestGeneration::new(14),
        "x.rs",
        vec![embedding_record("emb-1", "x.rs", vec![1.0, 0.0, 0.0])?],
        3,
    );
    batch.base_generation = Some(ManifestGeneration::new(1));

    let Err(err) = build_resident_batch_v1(&adapter, &batch) else {
        return Err("replace-generation batch with base_generation must fail closed".into());
    };
    match err {
        CoreError::InvalidContract(message) => {
            assert!(
                message.contains("ReplaceGeneration") && message.contains("base_generation"),
                "expected replace/base contract error, got: {message}"
            );
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
    reason = "test asserts delta batches require a base_generation via assert macros"
)]
fn delta_without_base_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let mut batch = sealed_batch(
        ManifestGeneration::new(15),
        "x.rs",
        vec![embedding_record("emb-1", "x.rs", vec![1.0, 0.0, 0.0])?],
        3,
    );
    batch.mode = BatchIngestMode::Delta;
    batch.base_generation = None;

    let Err(err) = build_resident_batch_v1(&adapter, &batch) else {
        return Err("delta batch without base_generation must fail closed".into());
    };
    match err {
        CoreError::InvalidContract(message) => {
            assert!(
                message.contains("Delta") && message.contains("base_generation"),
                "expected delta/base contract error, got: {message}"
            );
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
    reason = "test asserts reused unsealed generations reject a different delta base via assert macros"
)]
fn reused_unsealed_generation_with_different_base_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base_a = ManifestGeneration::new(1);
    let base_b = ManifestGeneration::new(2);
    let target = ManifestGeneration::new(16);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            base_a,
            "base-a.rs",
            vec![embedding_record("emb-a", "base-a.rs", vec![1.0, 0.0, 0.0])?],
            3,
        ),
    )?;
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            base_b,
            "base-b.rs",
            vec![embedding_record("emb-b", "base-b.rs", vec![0.0, 1.0, 0.0])?],
            3,
        ),
    )?;

    let first = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: target,
        base_generation: Some(base_a),
        manifest_digest: "manifest:16".to_string(),
        batch_digest: "batch:16:first".to_string(),
        mode: BatchIngestMode::Delta,
        model_contract: model_contract(3),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope("delta.rs"),
            scope_digest: "scope:delta:first".to_string(),
            embeddings: vec![embedding_record(
                "emb-first",
                "delta.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: false,
    };
    build_resident_batch_v1(&adapter, &first)?;

    let second = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: target,
        base_generation: Some(base_b),
        manifest_digest: "manifest:16".to_string(),
        batch_digest: "batch:16:second".to_string(),
        mode: BatchIngestMode::Delta,
        model_contract: model_contract(3),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope("delta.rs"),
            scope_digest: "scope:delta:second".to_string(),
            embeddings: vec![embedding_record(
                "emb-second",
                "delta.rs",
                vec![0.0, 1.0, 0.0],
            )?],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: false,
    };

    let Err(err) = build_resident_batch_v1(&adapter, &second) else {
        return Err("reused unsealed generation with a different base must fail closed".into());
    };
    match err {
        CoreError::InvalidContract(message) => {
            assert!(
                message.contains("base_generation") && message.contains("does not match"),
                "expected delta base provenance error, got: {message}"
            );
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
    reason = "test asserts an unsupported distance metric is rejected via assert macros"
)]
fn build_rejects_unsupported_distance_metric() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let mut batch = sealed_batch(
        ManifestGeneration::new(1),
        "x.rs",
        vec![embedding_record("emb-1", "x.rs", vec![1.0, 0.0, 0.0])?],
        3,
    );
    batch.model_contract.distance_metric = EmbeddingDistanceMetric::Euclidean;

    let Err(err) = build_resident_batch_v1(&adapter, &batch) else {
        return Err("unsupported distance metric must be rejected".into());
    };
    match err {
        CoreError::InvalidContract(message) => {
            assert!(message.contains("unsupported distance metric"));
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
    reason = "test asserts evicted generations reload correctly via assert macros"
)]
fn open_cache_survives_eviction_beyond_capacity() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    // Build more sealed generations than the open-cache capacity (8) and open
    // each so the early ones are evicted.
    for generation in 1..=12_u64 {
        build_resident_batch_v1(
            &adapter,
            &sealed_batch(
                ManifestGeneration::new(generation),
                "x.rs",
                vec![embedding_record(
                    &format!("emb-{generation}"),
                    "x.rs",
                    vec![1.0, 0.0, 0.0],
                )?],
                3,
            ),
        )?;
        let warm = adapter.open(
            &repo_id(),
            &revision_id(),
            ManifestGeneration::new(generation),
        )?;
        let _hits = warm.search(&[1.0, 0.0, 0.0], 1, &RequestBudgetV1::unbounded())?;
    }

    // The earliest generation was evicted; it must still reload from durable
    // state and serve identical results.
    let evicted = adapter.open(&repo_id(), &revision_id(), ManifestGeneration::new(1))?;
    let hits = evicted.search(&[1.0, 0.0, 0.0], 1, &RequestBudgetV1::unbounded())?;
    let Some(hit) = hits.first() else {
        return Err("evicted generation must reload and serve".into());
    };
    assert_eq!(hit.candidate_id.as_str(), "emb-1");
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts concurrent opens are consistent via assert macros"
)]
fn concurrent_open_of_same_generation_is_consistent() -> TestResult {
    use std::sync::Arc;
    use std::thread;

    let temp = tempfile::tempdir()?;
    let adapter = Arc::new(SemanticAdapter::with_state_root(temp.path().to_path_buf())?);
    let generation = ManifestGeneration::new(1);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "x.rs",
            vec![embedding_record("emb-1", "x.rs", vec![1.0, 0.0, 0.0])?],
            3,
        ),
    )?;

    let mut handles = Vec::new();
    for _worker in 0..8 {
        let adapter = Arc::clone(&adapter);
        handles.push(thread::spawn(move || -> Result<String, String> {
            let searcher = adapter
                .open(&repo_id(), &revision_id(), generation)
                .map_err(|err| format!("open: {err}"))?;
            let hits = searcher
                .search(&[1.0, 0.0, 0.0], 1, &RequestBudgetV1::unbounded())
                .map_err(|err| format!("search: {err}"))?;
            let hit = hits.first().ok_or_else(|| "no hit".to_string())?;
            Ok(hit.candidate_id.clone())
        }));
    }

    for handle in handles {
        let outcome = match handle.join() {
            Ok(inner) => inner,
            Err(_panic) => return Err("open thread panicked".into()),
        };
        let id = outcome?;
        assert_eq!(id, "emb-1");
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts a delta against an unsealed base fails closed via assert macros"
)]
fn delta_with_unsealed_base_fails_closed() -> TestResult {
    // R1 fix #2: prepare_generation_dir must refuse to clone a base whose
    // SEALED marker is absent (in-progress base is a moving target; copy is
    // only safe against a frozen dataset).
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(1);
    let mut base_batch = sealed_batch(
        base,
        "a.rs",
        vec![embedding_record("emb-1", "a.rs", vec![1.0, 0.0, 0.0])?],
        3,
    );
    base_batch.seal = false;
    build_resident_batch_v1(&adapter, &base_batch)?;

    let next = ManifestGeneration::new(2);
    let delta = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: next,
        base_generation: Some(base),
        manifest_digest: "manifest:delta".to_string(),
        batch_digest: "batch:delta".to_string(),
        mode: BatchIngestMode::Delta,
        model_contract: model_contract(3),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SemanticReplaceScope {
            scope: scope("b.rs"),
            scope_digest: "scope:b".to_string(),
            embeddings: vec![embedding_record("emb-2", "b.rs", vec![0.0, 1.0, 0.0])?],
            cluster_memberships: Vec::new(),
        }],
        tombstone_scopes: Vec::new(),
        seal: true,
    };
    let Err(err) = build_resident_batch_v1(&adapter, &delta) else {
        return Err("delta against unsealed base must fail closed".into());
    };
    match err {
        CoreError::NotReady(message) => {
            assert!(
                message.contains("not sealed"),
                "expected `not sealed` in error, got: {message}"
            );
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
    reason = "test asserts validate-before-delete preserves prior unsealed rows via assert macros"
)]
fn validate_before_delete_preserves_prior_unsealed_rows() -> TestResult {
    // R1 fix #1: validation is now hoisted ABOVE the delete loop, so a later
    // scope's contract failure may NOT destructively mutate earlier scopes'
    // data on the unsealed dataset. Construct a multi-scope batch with one
    // valid scope + one dim-mismatched scope; verify the prior row for the
    // first scope's path survives the rejection.
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(1);

    // Batch 1: prime the unsealed gen with emb-1 at a.rs.
    let mut batch1 = sealed_batch(
        generation,
        "a.rs",
        vec![embedding_record("emb-1", "a.rs", vec![1.0, 0.0, 0.0])?],
        3,
    );
    batch1.seal = false;
    build_resident_batch_v1(&adapter, &batch1)?;

    // Batch 2: try a multi-scope batch where the second scope's embedding
    // dimension does NOT match the contract. The first scope (valid) replaces
    // a.rs — that delete must NOT happen because validation rejects the batch
    // up front.
    let mut batch2 = sealed_batch(
        generation,
        "a.rs",
        vec![embedding_record(
            "emb-replace",
            "a.rs",
            vec![0.0, 1.0, 0.0],
        )?],
        3,
    );
    batch2.seal = false;
    batch2.replace_scopes.push(SemanticReplaceScope {
        scope: scope("b.rs"),
        scope_digest: "scope:b".to_string(),
        embeddings: vec![{
            let mut record = embedding_record("emb-bad-dim", "b.rs", vec![1.0, 0.0, 0.0])?;
            record.embedding_input_digest = "in:bad".to_string().into_boxed_str();
            record.vector_digest = "vec:bad".to_string().into_boxed_str();
            record.snippet = "x".to_string().into_boxed_str();
            record.start_byte = 0;
            record.end_byte = 1;
            record.start_line = 1;
            record.end_line = 1;
            record.vector = vec![1.0, 0.0];
            record
        }],
        cluster_memberships: Vec::new(),
    });

    let Err(err) = build_resident_batch_v1(&adapter, &batch2) else {
        return Err("invalid-dim scope must reject batch".into());
    };
    assert!(
        matches!(err, CoreError::Typed { ref code, .. }
        if *code == quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
            quanta_index_contract::lex::LexicalErrorCode::SemInvalidVector,
        )),
        "{err:?}"
    );

    // Batch 3: empty seal — opens the unsealed gen as-is. emb-1 MUST still be
    // present (no destructive delete from the rejected batch).
    let batch3 = SemanticIngestBatch {
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation,
        base_generation: None,
        manifest_digest: "manifest:1".to_string(),
        batch_digest: "batch:1:seal".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        model_contract: model_contract(3),
        required_corpora: Vec::new(),
        corpus_policy_digest: None,
        clear_surfaces: Vec::new(),
        replace_scopes: Vec::new(),
        tombstone_scopes: Vec::new(),
        seal: true,
    };
    build_resident_batch_v1(&adapter, &batch3)?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[1.0, 0.0, 0.0], 5, &RequestBudgetV1::unbounded())?;
    let ids: Vec<String> = hits.iter().map(|c| c.candidate_id.clone()).collect();
    assert!(
        ids.contains(&"emb-1".to_string()),
        "validate-before-delete must preserve emb-1; got {ids:?}"
    );
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts append-failure staging preserves prior unsealed rows via assert macros"
)]
fn append_failure_preserves_prior_unsealed_rows() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(15);

    let mut first = sealed_batch(
        generation,
        "src/main.rs",
        vec![embedding_record(
            "emb-1",
            "src/main.rs",
            vec![1.0, 0.0, 0.0],
        )?],
        3,
    );
    first.seal = false;
    build_resident_batch_v1(&adapter, &first)?;

    let mut second = sealed_batch(
        generation,
        "src/main.rs",
        vec![embedding_record(
            "emb-2",
            "src/main.rs",
            vec![0.0, 1.0, 0.0],
        )?],
        3,
    );
    second.seal = false;
    test_support::set_append_fail_path(Some("src/main.rs"));
    let Err(err) = build_resident_batch_v1(&adapter, &second) else {
        return Err("injected append failure must surface".into());
    };
    test_support::set_append_fail_path(None);
    assert!(
        matches!(err, CoreError::Storage(ref message) if message.contains("injected append failure")),
        "expected injected append failure storage error, got: {err:?}"
    );

    build_resident_batch_v1(
        &adapter,
        &SemanticIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation,
            base_generation: None,
            manifest_digest: "manifest:15".to_string(),
            batch_digest: "batch:15:seal".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: model_contract(3),
            required_corpora: Vec::new(),
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        },
    )?;

    let searcher =
        SemanticAdapter::with_state_root(root)?.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search(&[1.0, 0.0, 0.0], 5, &RequestBudgetV1::unbounded())?;
    let ids: Vec<String> = hits
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    assert!(
        ids.contains(&"emb-1".to_string()),
        "append failure must preserve prior rows; got {ids:?}"
    );
    assert!(
        !ids.contains(&"emb-2".to_string()),
        "failed append must not leak replacement rows; got {ids:?}"
    );
    Ok(())
}

/// Re-encode the on-disk manifest after mutating one named text field.
///
/// Used by the forged-manifest negative tests below. The mutation stays valid
/// CBOR so we exercise the actual `validate_scope` branch, not the generic
/// decode-failure branch already covered by `corrupt_manifest_open_fails_closed`.
fn forge_manifest_text_field(
    manifest_path: &Path,
    field: &str,
    new_value: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(manifest_path)?;
    let mut value: ciborium::value::Value =
        ciborium::from_reader(&bytes[..]).map_err(|err| format!("decode manifest cbor: {err}"))?;
    let mut bumped = false;
    if let ciborium::value::Value::Map(entries) = &mut value {
        for (key, val) in entries.iter_mut() {
            if key.as_text() == Some(field) {
                *val = ciborium::value::Value::Text(new_value.to_string());
                bumped = true;
                break;
            }
        }
    }
    if !bumped {
        return Err(format!("expected `{field}` field in manifest CBOR").into());
    }
    let mut tampered = Vec::new();
    ciborium::into_writer(&value, &mut tampered)
        .map_err(|err| format!("encode manifest cbor: {err}"))?;
    std::fs::write(manifest_path, &tampered)?;
    Ok(())
}

fn forge_manifest_u64_field(
    manifest_path: &Path,
    field: &str,
    new_value: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = std::fs::read(manifest_path)?;
    let mut value: ciborium::value::Value =
        ciborium::from_reader(&bytes[..]).map_err(|err| format!("decode manifest cbor: {err}"))?;
    let mut bumped = false;
    if let ciborium::value::Value::Map(entries) = &mut value {
        for (key, val) in entries.iter_mut() {
            if key.as_text() == Some(field) {
                *val = ciborium::value::Value::Integer(ciborium::value::Integer::from(new_value));
                bumped = true;
                break;
            }
        }
    }
    if !bumped {
        return Err(format!("expected `{field}` field in manifest CBOR").into());
    }
    let mut tampered = Vec::new();
    ciborium::into_writer(&value, &mut tampered)
        .map_err(|err| format!("encode manifest cbor: {err}"))?;
    std::fs::write(manifest_path, &tampered)?;
    Ok(())
}

/// Fields the format-2 manifest never carried, in any later format's order.
const POST_V2_MANIFEST_FIELDS: &[&str] = &[
    "semantic_row_root_digest",
    "present_corpora",
    "required_corpora",
    "card_schema_versions",
    "render_policy_digests",
    "corpus_policy_digest",
    "cluster_membership_root_digest",
    "cluster_membership_cluster_count",
    "cluster_membership_member_row_count",
    "vector_index",
];

/// Rewrite a current manifest as an honest format-2 manifest.
///
/// The version field says `2` and every later field is gone, so the adapter
/// decodes it through its legacy shape rather than through a current shape
/// with a forged version.
fn forge_legacy_v2_manifest(manifest_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    forge_manifest_u64_field(manifest_path, "format_version", 2)?;
    let bytes = std::fs::read(manifest_path)?;
    let mut value: ciborium::value::Value =
        ciborium::from_reader(&bytes[..]).map_err(|err| format!("decode manifest cbor: {err}"))?;
    let ciborium::value::Value::Map(entries) = &mut value else {
        return Err("manifest CBOR is not a map".into());
    };
    let before = entries.len();
    entries.retain(|(key, _)| {
        !key.as_text()
            .is_some_and(|name| POST_V2_MANIFEST_FIELDS.contains(&name))
    });
    if before.saturating_sub(entries.len()) != POST_V2_MANIFEST_FIELDS.len() {
        return Err(format!(
            "expected every post-v2 field in the current manifest, removed {}",
            before.saturating_sub(entries.len())
        )
        .into());
    }
    let mut legacy = Vec::new();
    ciborium::into_writer(&value, &mut legacy)
        .map_err(|err| format!("encode manifest cbor: {err}"))?;
    std::fs::write(manifest_path, &legacy)?;
    Ok(())
}

#[test]
fn open_with_forged_manifest_scope_fails_closed() -> TestResult {
    // R4 MAJOR coverage: explicit negative for `SemanticManifest::validate_scope`'s
    // repo-mismatch branch. A built-and-sealed generation has its manifest
    // re-encoded with a different `repo_id` (still valid CBOR + matching format
    // version). Open against the original requested scope must fail closed with
    // a typed scope-mismatch storage error, NOT silently serve the table.
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(11);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let manifest_path = generation_dir(&root, generation).join("semantic-manifest.cbor");
    forge_manifest_text_field(&manifest_path, "repo_id", "repo-DIFFERENT")?;

    // Fresh adapter to bypass the open cache.
    let reopened = SemanticAdapter::with_state_root(root)?;
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("manifest scope mismatch must fail closed".into());
    };
    expect_sidecar_corrupt(err, "semantic-manifest.cbor")
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test asserts cross-batch dim mismatch fails closed via assert macros"
)]
fn cross_batch_dimension_mismatch_on_unsealed_generation_fails_closed() -> TestResult {
    // R4 MAJOR coverage: explicit negative for `verify_table_dimension`. An
    // unsealed generation's FixedSizeList vector dim is fixed by the FIRST
    // batch's contract dim; a follow-up batch on the same gen with a different
    // contract dim must surface InvalidContract from the adapter's dim check,
    // not lancedb's late opaque arrow schema-mismatch error.
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(12);

    // Batch 1: contract dim=3 -> creates the lancedb table at dim=3.
    let mut batch1 = sealed_batch(
        generation,
        "a.rs",
        vec![embedding_record("emb-1", "a.rs", vec![1.0, 0.0, 0.0])?],
        3,
    );
    batch1.seal = false;
    build_resident_batch_v1(&adapter, &batch1)?;

    // Batch 2: same unsealed gen, contract dim=4, with a dim-4 vector so the
    // batch's own per-embedding validation passes. `ensure_table` opens the
    // existing dim-3 table and `verify_table_dimension(_, 4)` must fail closed.
    let mut batch2 = sealed_batch(
        generation,
        "b.rs",
        vec![embedding_record("emb-2", "b.rs", vec![1.0, 0.0, 0.0, 0.0])?],
        4,
    );
    batch2.seal = false;

    let Err(err) = build_resident_batch_v1(&adapter, &batch2) else {
        return Err("cross-batch dim mismatch must fail closed".into());
    };
    match err {
        CoreError::InvalidContract(message) => {
            assert!(
                message.contains("dimension 3") && message.contains("batch dimension 4"),
                "expected cross-batch dim mismatch error, got: {message}"
            );
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
fn open_with_forged_non_cosine_distance_metric_fails_closed() -> TestResult {
    // R4 MAJOR coverage: explicit negative for `SemanticManifest::validate_scope`'s
    // distance-metric guard. `build_batch` rejects non-cosine contracts up front
    // (covered by `build_rejects_unsupported_distance_metric`), so the only way
    // a non-cosine manifest can reach `open_generation` is corruption / drift.
    // We forge that state here and assert open fails closed with the dedicated
    // distance-metric storage error rather than silently serving cosine-quantized
    // data through a foreign metric contract.
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(13);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let manifest_path = generation_dir(&root, generation).join("semantic-manifest.cbor");
    forge_manifest_text_field(&manifest_path, "distance_metric", "euclidean")?;

    let reopened = SemanticAdapter::with_state_root(root)?;
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("forged non-cosine distance_metric must fail closed".into());
    };
    expect_sidecar_corrupt(err, "semantic-manifest.cbor")
}

#[test]
fn open_with_missing_generation_contract_on_v3_manifest_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(16);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let contract_path = generation_dir(&root, generation).join("semantic-build-contract.cbor");
    std::fs::remove_file(&contract_path)?;

    let reopened = SemanticAdapter::with_state_root(root)?;
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("v3 manifest without generation contract must fail closed".into());
    };
    expect_sidecar_corrupt(err, "semantic-build-contract.cbor")
}

/// A generation sealed at format 2 is refused typed at open.
///
/// Format 2 has no build contract and no sealed manifest; the refusal
/// carries the instruction to rebuild and the generation is never decoded
/// through an older shape and served on what it happens to carry
/// (breaking-first; QI-BB-027).
#[test]
fn a_format_2_generation_is_refused_typed_at_open() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(21);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-legacy",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let generation_dir = generation_dir(&root, generation);
    forge_legacy_v2_manifest(&generation_dir.join("semantic-manifest.cbor"))?;
    std::fs::remove_file(generation_dir.join("semantic-build-contract.cbor"))?;
    // A generation sealed at format 2 predates the sealed manifest.
    std::fs::remove_file(generation_dir.join("semantic-sealed-manifest.cbor"))?;

    let reopened = SemanticAdapter::with_state_root(root)?;
    match reopened.open(&repo_id(), &revision_id(), generation) {
        Err(CoreError::Typed { code, message })
            if code == quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestMissing
                && message.contains("rebuilt") => {}
        Ok(_) => return Err("a format-2 generation must not be served".into()),
        Err(other) => {
            return Err(format!(
                "a format-2 generation must be refused typed with the rebuild instruction, got {other:?}"
            )
            .into());
        }
    }
    Ok(())
}

/// The inventory sets a format-2 generation aside under the format reason
/// instead of seeding it, so nothing can pin, activate or build on it.
#[test]
fn scan_quarantines_a_format_2_generation_as_format_unsupported() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let semantic_root = root.join("indexes").join("semantic");
    let adapter = SemanticAdapter::with_state_root(semantic_root.clone())?;
    let generation = ManifestGeneration::new(22);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "scan-legacy.rs",
            vec![embedding_record(
                "emb-scan-legacy",
                "scan-legacy.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let generation_dir = generation_dir(&semantic_root, generation);
    forge_legacy_v2_manifest(&generation_dir.join("semantic-manifest.cbor"))?;
    std::fs::remove_file(generation_dir.join("semantic-build-contract.cbor"))?;

    let inventory = inventory_persisted_generations(&semantic_root)?;
    if inventory
        .sealed
        .iter()
        .any(|record| record.generation == generation)
    {
        return Err("a format-2 generation must not be seeded".into());
    }
    let Some(entry) = inventory
        .quarantined
        .iter()
        .find(|entry| entry.path == generation_dir)
    else {
        return Err(format!(
            "a format-2 generation must be quarantined: {:?}",
            inventory.quarantined
        )
        .into());
    };
    if entry.reason != GenerationQuarantineReasonV1::FormatUnsupported
        || !entry.detail.contains("format version 2")
        || !entry.detail.contains("rebuild")
    {
        return Err(format!("the quarantine names the format and the remedy: {entry:?}").into());
    }
    Ok(())
}

#[test]
fn open_with_forged_model_id_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(17);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let manifest_path = generation_dir(&root, generation).join("semantic-manifest.cbor");
    forge_manifest_text_field(&manifest_path, "model_id", "DIFFERENT-MODEL")?;

    let reopened = SemanticAdapter::with_state_root(root)?;
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("forged model_id must fail closed".into());
    };
    expect_sidecar_corrupt(err, "semantic-manifest.cbor")
}

#[test]
fn open_with_forged_model_version_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(18);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let manifest_path = generation_dir(&root, generation).join("semantic-manifest.cbor");
    forge_manifest_text_field(&manifest_path, "model_version", "2")?;

    let reopened = SemanticAdapter::with_state_root(root)?;
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("forged model_version must fail closed".into());
    };
    expect_sidecar_corrupt(err, "semantic-manifest.cbor")
}

#[test]
fn open_with_forged_normalization_fails_closed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let adapter = SemanticAdapter::with_state_root(root.clone())?;
    let generation = ManifestGeneration::new(19);
    build_resident_batch_v1(
        &adapter,
        &sealed_batch(
            generation,
            "src/main.rs",
            vec![embedding_record(
                "emb-1",
                "src/main.rs",
                vec![1.0, 0.0, 0.0],
            )?],
            3,
        ),
    )?;

    let manifest_path = generation_dir(&root, generation).join("semantic-manifest.cbor");
    forge_manifest_text_field(&manifest_path, "normalization", "none")?;

    let reopened = SemanticAdapter::with_state_root(root)?;
    let Err(err) = reopened.open(&repo_id(), &revision_id(), generation) else {
        return Err("forged normalization must fail closed".into());
    };
    expect_sidecar_corrupt(err, "semantic-manifest.cbor")
}
