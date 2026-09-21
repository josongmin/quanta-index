//! SCV2 persisted semantic product scenarios.
//!
//! These tests exercise the public semantic adapter ports against a real
//! on-disk `LanceDB` generation. They intentionally combine multiple operations
//! per scenario so unit-level row-shape tests cannot substitute for owner-scope,
//! seal, restart, and identity behavior.

#![forbid(unsafe_code)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning scenarios assert with `assert!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]

use quanta_index_contract::{
    BatchIngestMode, ManifestGeneration, OwnerDocKind, RepoId, RevisionId, SemanticCorpusKindV1,
    SemanticIngestBatch, SemanticReplaceScope,
};
use quanta_index_core::{CoreError, RequestBudgetV1, SemanticIndexOpenPort};
use quanta_index_semantic::{
    SemanticAdapter, build_resident_batch_v1, embedding_record_v1, ingest_batch_v1,
    model_contract_v1, search_scope_v1, tombstone_scope_with_semantic_owner_v1,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The (1, 1, 0) direction at unit norm: the generations are `L2Unit`, so
/// their queries are held to the unit contract (QI-BB-031).
const DIAGONAL_XY: [f32; 3] = [
    std::f32::consts::FRAC_1_SQRT_2,
    std::f32::consts::FRAC_1_SQRT_2,
    0.0,
];

const DIMENSION: u32 = 3;
const MANIFEST_DIGEST: &str = "manifest:scv2-product-scenarios";
const CORPUS_POLICY_DIGEST: &str = "policy:semantic-source-v1";

fn repo_id() -> RepoId {
    RepoId::new("repo-scv2-scenarios").expect("static fixture ID satisfies canonical policy")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-scv2-scenarios").expect("static fixture ID satisfies canonical policy")
}

fn symbol_embedding(
    record: &str,
    owner: &str,
    vector: Vec<f32>,
) -> Result<quanta_index_contract::EmbeddingRecord, String> {
    embedding_record_v1(
        record,
        "src/session.rs",
        OwnerDocKind::Symbol,
        owner,
        SemanticCorpusKindV1::SymbolCard,
        vector,
    )
}

fn scenario_batch(
    generation: ManifestGeneration,
    base_generation: Option<ManifestGeneration>,
    batch_digest: &str,
    replace_scopes: Vec<SemanticReplaceScope>,
    tombstone_scopes: Vec<quanta_index_contract::SemanticTombstoneScope>,
    seal: bool,
) -> SemanticIngestBatch {
    let mut batch = ingest_batch_v1(
        repo_id(),
        revision_id(),
        generation,
        base_generation,
        MANIFEST_DIGEST.to_string(),
        batch_digest.to_string(),
        if base_generation.is_some() {
            BatchIngestMode::Delta
        } else {
            BatchIngestMode::ReplaceGeneration
        },
        model_contract_v1(DIMENSION),
        replace_scopes,
        tombstone_scopes,
        seal,
    );
    batch.required_corpora = vec![SemanticCorpusKindV1::SymbolCard];
    batch.corpus_policy_digest = Some(CORPUS_POLICY_DIGEST.to_string());
    batch
}

fn symbol_scope(embeddings: Vec<quanta_index_contract::EmbeddingRecord>) -> SemanticReplaceScope {
    SemanticReplaceScope {
        scope: search_scope_v1("src/session.rs"),
        scope_digest: "scope:src/session.rs:symbol-cards".to_string(),
        embeddings,
        cluster_memberships: Vec::new(),
    }
}

fn sorted_owner_record_pairs(
    hits: &[quanta_index_core::SemanticSearchHitV1],
) -> Vec<(String, String, Option<SemanticCorpusKindV1>)> {
    let mut pairs = hits
        .iter()
        .map(|hit| (hit.owner_id.clone(), hit.record_id.clone(), hit.corpus_kind))
        .collect::<Vec<_>>();
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    pairs
}

#[test]
fn scv2_s01_exact_owner_replace_does_not_erase_sibling_on_same_path() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(101);

    build_resident_batch_v1(
        &adapter,
        &scenario_batch(
            generation,
            None,
            "batch:s01:initial",
            vec![symbol_scope(vec![
                symbol_embedding(
                    "commit-v1",
                    "symbol:RuntimeSession::commit",
                    vec![1.0, 0.0, 0.0],
                )?,
                symbol_embedding(
                    "prepare-v1",
                    "symbol:RuntimeSession::prepare",
                    vec![0.0, 1.0, 0.0],
                )?,
            ])],
            Vec::new(),
            false,
        ),
    )?;

    build_resident_batch_v1(
        &adapter,
        &scenario_batch(
            generation,
            None,
            "batch:s01:replace-one-owner",
            vec![symbol_scope(vec![symbol_embedding(
                "commit-v2",
                "symbol:RuntimeSession::commit",
                vec![0.9, 0.1, 0.0],
            )?])],
            Vec::new(),
            false,
        ),
    )?;

    build_resident_batch_v1(
        &adapter,
        &scenario_batch(
            generation,
            None,
            "batch:s01:seal",
            Vec::new(),
            Vec::new(),
            true,
        ),
    )?;

    let searcher = adapter.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search_hits(&DIAGONAL_XY, 10, &RequestBudgetV1::unbounded())?;
    let pairs = sorted_owner_record_pairs(&hits);
    assert_eq!(
        pairs,
        vec![
            (
                "symbol:RuntimeSession::commit".to_string(),
                "record-commit-v2".to_string(),
                Some(SemanticCorpusKindV1::SymbolCard),
            ),
            (
                "symbol:RuntimeSession::prepare".to_string(),
                "record-prepare-v1".to_string(),
                Some(SemanticCorpusKindV1::SymbolCard),
            ),
        ],
        "owner-level replacement must not degrade back to path-level deletion",
    );
    Ok(())
}

#[test]
fn scv2_s02_owner_tombstone_removes_only_target_owner() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let base = ManifestGeneration::new(102);
    let next = ManifestGeneration::new(103);

    build_resident_batch_v1(
        &adapter,
        &scenario_batch(
            base,
            None,
            "batch:s02:base",
            vec![symbol_scope(vec![
                symbol_embedding(
                    "commit",
                    "symbol:RuntimeSession::commit",
                    vec![1.0, 0.0, 0.0],
                )?,
                symbol_embedding(
                    "prepare",
                    "symbol:RuntimeSession::prepare",
                    vec![0.0, 1.0, 0.0],
                )?,
            ])],
            Vec::new(),
            true,
        ),
    )?;

    build_resident_batch_v1(
        &adapter,
        &scenario_batch(
            next,
            Some(base),
            "batch:s02:tombstone-commit",
            Vec::new(),
            vec![tombstone_scope_with_semantic_owner_v1(
                "src/session.rs",
                SemanticCorpusKindV1::SymbolCard,
                OwnerDocKind::Symbol,
                "symbol:RuntimeSession::commit",
            )],
            true,
        ),
    )?;

    let searcher = adapter.open(&repo_id(), &revision_id(), next)?;
    let hits = searcher.search_hits(&DIAGONAL_XY, 10, &RequestBudgetV1::unbounded())?;
    let pairs = sorted_owner_record_pairs(&hits);
    assert_eq!(
        pairs,
        vec![(
            "symbol:RuntimeSession::prepare".to_string(),
            "record-prepare".to_string(),
            Some(SemanticCorpusKindV1::SymbolCard),
        )],
        "a semantic owner tombstone must preserve sibling owners sharing the file",
    );
    Ok(())
}

#[test]
fn scv2_s03_restart_preserves_owner_and_corpus_identity() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let generation = ManifestGeneration::new(104);
    {
        let writer = SemanticAdapter::with_state_root(root.clone())?;
        build_resident_batch_v1(
            &writer,
            &scenario_batch(
                generation,
                None,
                "batch:s03:sealed",
                vec![symbol_scope(vec![symbol_embedding(
                    "commit",
                    "symbol:RuntimeSession::commit",
                    vec![1.0, 0.0, 0.0],
                )?])],
                Vec::new(),
                true,
            ),
        )?;
    }

    let restarted = SemanticAdapter::with_state_root(root)?;
    let searcher = restarted.open(&repo_id(), &revision_id(), generation)?;
    let hits = searcher.search_hits(&[1.0, 0.0, 0.0], 3, &RequestBudgetV1::unbounded())?;
    let Some(hit) = hits.first() else {
        return Err("restarted adapter must return the persisted symbol card".into());
    };
    assert_eq!(hit.record_id, "record-commit");
    assert_eq!(hit.owner_id, "symbol:RuntimeSession::commit");
    assert_eq!(hit.owner_kind, OwnerDocKind::Symbol);
    assert_eq!(hit.corpus_kind, Some(SemanticCorpusKindV1::SymbolCard));
    Ok(())
}

#[test]
fn scv2_s04_seal_rejects_missing_required_corpus() -> TestResult {
    let temp = tempfile::tempdir()?;
    let adapter = SemanticAdapter::with_state_root(temp.path().to_path_buf())?;
    let generation = ManifestGeneration::new(105);
    let mut batch = scenario_batch(
        generation,
        None,
        "batch:s04:missing-module-card",
        vec![symbol_scope(vec![symbol_embedding(
            "commit",
            "symbol:RuntimeSession::commit",
            vec![1.0, 0.0, 0.0],
        )?])],
        Vec::new(),
        true,
    );
    batch
        .required_corpora
        .push(SemanticCorpusKindV1::ModuleCard);

    let error = build_resident_batch_v1(&adapter, &batch)
        .expect_err("generation missing ModuleCard must not seal");
    match error {
        CoreError::Storage(message) => assert!(
            message.contains("required corpus `ModuleCard` missing"),
            "failure must name the missing required corpus: {message}",
        ),
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)) => {
            return Err(format!("expected storage seal error, got {other:?}").into());
        }
    }
    Ok(())
}
