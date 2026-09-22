use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    HybridSeedQueryRequest, ManifestGeneration, QueryConstraintSetV1, RepoId, RevisionId,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SemanticCorpusKindV1,
    SemanticSeedCorpusBudgetV1, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{RequestBudgetV1, SemanticSearchHitV1};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::selection::make_pin;
use crate::query_dispatcher::semantic_query::build_hybrid_seed_candidates;
use crate::query_dispatcher::tests::support::common::{
    TestResult, candidate, default_query_embedder, ready_ledger, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState,
};
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

#[test]
fn hybrid_seed_dispatch_includes_dense_only_entity_in_the_seed_set() -> TestResult {
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
        quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
            .map_err(str::to_string)?,
    );
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&lexical_state),
            results: vec![candidate("alpha", 1.0), candidate("beta", 0.9)],
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic_state),
        }),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let pin = make_pin(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(9),
    );
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "scope".to_string(),
                constraints: constraints.clone(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 3,
                cursor: None,
            },
            semantic_query_text: "scope alpha".to_string(),
            generation: Some(pin),
            generation_selector: None,
            dense_corpora: vec![
                SemanticSeedCorpusBudgetV1 {
                    corpus_kind: SemanticCorpusKindV1::SymbolCard,
                    top_k: 7,
                },
                SemanticSeedCorpusBudgetV1 {
                    corpus_kind: SemanticCorpusKindV1::RepositorySummary,
                    top_k: 11,
                },
                SemanticSeedCorpusBudgetV1 {
                    corpus_kind: SemanticCorpusKindV1::ClusterCard,
                    top_k: 13,
                },
            ],
            top_k: 3,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::HybridSeed(hybrid_seed) => {
            let response_json = serde_json::to_value(&hybrid_seed)?;
            if response_json
                .get("manifest_digest")
                .and_then(serde_json::Value::as_str)
                != Some("manifest-digest-9")
            {
                return Err(format!(
                    "hybrid seed response must carry the sealed semantic manifest digest, observed={response_json}"
                )
                .into());
            }
            let seed_candidates = hybrid_seed.seed_candidates;
            if !seed_candidates
                .iter()
                .any(|candidate| candidate.entity_id == "owner:SymbolCard")
            {
                return Err(format!(
                    "dense-only semantic entity must enter v2 seed set, observed={seed_candidates:?}"
                )
                .into());
            }
            let cluster_seed = seed_candidates
                .iter()
                .find(|candidate| {
                    candidate.corpus_kind == Some(SemanticCorpusKindV1::ClusterCard)
                })
                .ok_or_else(|| {
                    format!(
                        "requested ClusterCard lane must reach the hybrid seed response: {seed_candidates:?}"
                    )
                })?;
            if cluster_seed.authority_digest.as_deref() != Some("authority:ClusterCard") {
                return Err(format!(
                    "ClusterCard record authority must survive semantic search and seed assembly: {cluster_seed:?}"
                )
                .into());
            }
            if !seed_candidates.iter().all(|candidate| {
                candidate.degraded_reasons.iter().any(|reason| {
                    reason == "requested_semantic_corpus_unavailable:RepositorySummary"
                })
            }) {
                return Err(format!(
                    "missing requested corpus must remain explicit on every returned seed: {seed_candidates:?}"
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected HybridSeed response, got {other:?}").into());
        }
    }

    let (
        scoped_vectors,
        search_hit_vectors,
        search_vectors,
        corpus_searches,
        scoped_constraints,
        corpus_constraints,
    ) = {
        let guard = semantic_state
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        (
            guard.scoped_vectors.clone(),
            guard.search_hit_vectors.clone(),
            guard.search_vectors.clone(),
            guard.corpus_searches.clone(),
            guard.scoped_constraints.clone(),
            guard.corpus_constraints.clone(),
        )
    };
    let expected = default_query_embedder().embed_query(
        "scope alpha",
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    // QI-BB-019: the seed list is built from the dense lanes alone; no
    // second, lexical-scoped dense search runs behind it.
    if !scoped_vectors.is_empty() {
        return Err(format!(
            "hybrid seed must not run a lexical-scoped dense search: {scoped_vectors:?}"
        )
        .into());
    }
    // Exactly one dense search per requested corpus lane, all over the
    // one query vector.
    if search_hit_vectors.as_slice() != [expected.clone(), expected.clone(), expected] {
        return Err(format!("unexpected corpus hit vectors: {search_hit_vectors:?}").into());
    }
    if !search_vectors.is_empty() {
        return Err(format!("unexpected global lexical-shaped vectors: {search_vectors:?}").into());
    }
    if corpus_searches.as_slice()
        != [
            (SemanticCorpusKindV1::ClusterCard, 13),
            (SemanticCorpusKindV1::RepositorySummary, 11),
            (SemanticCorpusKindV1::SymbolCard, 7),
        ]
    {
        return Err(format!("unexpected corpus-prefiltered searches: {corpus_searches:?}").into());
    }
    if !scoped_constraints.is_empty()
        || corpus_constraints.as_slice()
            != [
                constraints.clone(),
                constraints.clone(),
                constraints.clone(),
            ]
    {
        return Err(format!(
            "hybrid-seed constraints drifted: scoped={scoped_constraints:?} corpus={corpus_constraints:?}"
        )
        .into());
    }
    let searched_constraints = {
        let guard = lexical_state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        guard.searched_constraints.clone()
    };
    if searched_constraints.as_slice() != [constraints] {
        return Err(format!(
            "hybrid-seed lexical leg lost exact-path constraints: {searched_constraints:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn hybrid_seed_keeps_cross_owner_ids_and_corpus_local_ranks_distinct() -> TestResult {
    fn hit(
        record_id: &str,
        owner_id: &str,
        corpus_kind: SemanticCorpusKindV1,
        score: f32,
    ) -> SemanticSearchHitV1 {
        SemanticSearchHitV1 {
            candidate: candidate(record_id, score),
            record_id: record_id.to_string(),
            owner_id: owner_id.to_string(),
            owner_kind: match corpus_kind {
                SemanticCorpusKindV1::SymbolCard => quanta_index_contract::OwnerDocKind::Symbol,
                SemanticCorpusKindV1::ModuleCard => quanta_index_contract::OwnerDocKind::Module,
                SemanticCorpusKindV1::ClusterCard
                | SemanticCorpusKindV1::RawCodeFallback
                | SemanticCorpusKindV1::DocumentLeaf
                | SemanticCorpusKindV1::DocumentSection
                | SemanticCorpusKindV1::DocumentSummary
                | SemanticCorpusKindV1::TestBehavior
                | SemanticCorpusKindV1::RepositorySummary => {
                    quanta_index_contract::OwnerDocKind::Chunk
                }
            },
            corpus_kind: Some(corpus_kind),
            authority_digest: format!("authority:{record_id}"),
        }
    }

    let semantic_lanes = vec![
        vec![
            hit(
                "module-shared",
                "shared",
                SemanticCorpusKindV1::ModuleCard,
                0.0001,
            ),
            hit(
                "module-only",
                "module-only",
                SemanticCorpusKindV1::ModuleCard,
                9_999.0,
            ),
        ],
        vec![
            hit(
                "symbol-shared",
                "shared",
                SemanticCorpusKindV1::SymbolCard,
                0.0002,
            ),
            hit(
                "symbol-only",
                "symbol-only",
                SemanticCorpusKindV1::SymbolCard,
                8_888.0,
            ),
        ],
    ];
    let seeds = build_hybrid_seed_candidates(&[], &semantic_lanes, &[], 3)?;
    let module_shared = seeds
        .iter()
        .find(|seed| {
            seed.entity_id == "shared"
                && seed.owner_kind == quanta_index_contract::OwnerDocKind::Module
        })
        .ok_or_else(|| "expected Module/shared seed".to_string())?;
    let symbol_shared = seeds
        .iter()
        .find(|seed| {
            seed.entity_id == "shared"
                && seed.owner_kind == quanta_index_contract::OwnerDocKind::Symbol
        })
        .ok_or_else(|| "expected Symbol/shared seed".to_string())?;

    for (seed, expected_corpus) in [
        (module_shared, SemanticCorpusKindV1::ModuleCard),
        (symbol_shared, SemanticCorpusKindV1::SymbolCard),
    ] {
        let contribution = seed
            .contributions
            .first()
            .ok_or_else(|| format!("cross-owner seed must retain one contribution: {seed:?}"))?;
        if seed.contributions.len() != 1
            || contribution.corpus_kind != Some(expected_corpus)
            || contribution.rank != 1
        {
            return Err(format!(
                "cross-owner seed must retain one rank-1 corpus-local contribution: {seed:?}"
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn hybrid_seed_preserves_authoritative_semantic_owner_kind() -> TestResult {
    let semantic_lanes = vec![vec![SemanticSearchHitV1 {
        candidate: candidate("test-behavior-record", 0.9),
        record_id: "test-behavior-record".to_string(),
        owner_id: "test:session_commit".to_string(),
        owner_kind: quanta_index_contract::OwnerDocKind::Test,
        corpus_kind: Some(SemanticCorpusKindV1::TestBehavior),
        authority_digest: "authority:test-behavior-record".to_string(),
    }]];

    let seeds = build_hybrid_seed_candidates(&[], &semantic_lanes, &[], 1)?;
    let seed = seeds
        .first()
        .ok_or_else(|| "expected one semantic seed".to_string())?;
    if seed.owner_kind != quanta_index_contract::OwnerDocKind::Test {
        return Err(format!(
            "semantic seed must preserve Test owner kind independently of corpus kind: {seed:?}"
        )
        .into());
    }
    Ok(())
}
