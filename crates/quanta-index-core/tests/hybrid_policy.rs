//! Unit tests for the hybrid orchestrator policy: joint-readiness gating and
//! RRF fusion edge cases. Targets pure functions on
//! [`HybridOrchestratorPolicy`]; no I/O, no async.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::{
    HighlightSpan, LexicalCandidate, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
};
use quanta_index_core::{CoreError, HybridOrchestratorPolicy};

type TestResult = Result<(), Box<dyn Error>>;

fn candidate(id: &str) -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: id.to_string(),
        repo_id: RepoId::new("r"),
        revision_id: RevisionId::new("v"),
        manifest_generation: ManifestGeneration::new(1),
        repo_relative_path: RepoRelativePath::new(""),
        start_line: 0,
        end_line: 0,
        score: 0.0,
        snippet: String::new(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

fn g(n: u64) -> ManifestGeneration {
    ManifestGeneration::new(n)
}

fn typed_code_or_debug(result: Result<(), CoreError>) -> String {
    match result {
        Err(CoreError::Typed { code, .. }) => code,
        other => format!("unexpected result: {other:?}"),
    }
}

#[test]
fn joint_readiness_both_ahead_is_ok() -> TestResult {
    HybridOrchestratorPolicy::validate_joint_readiness(g(5), Some(g(5)), Some(g(5)))?;
    HybridOrchestratorPolicy::validate_joint_readiness(g(5), Some(g(6)), Some(g(7)))?;
    Ok(())
}

#[test]
fn joint_readiness_lex_behind_is_not_ready() {
    let result = HybridOrchestratorPolicy::validate_joint_readiness(g(5), Some(g(4)), Some(g(5)));
    assert!(matches!(result, Err(CoreError::NotReady(_))));
}

#[test]
fn joint_readiness_sem_behind_is_not_ready() {
    let result = HybridOrchestratorPolicy::validate_joint_readiness(g(5), Some(g(5)), Some(g(4)));
    assert!(matches!(result, Err(CoreError::NotReady(_))));
}

#[test]
fn joint_readiness_both_unsealed_is_not_ready() {
    let result = HybridOrchestratorPolicy::validate_joint_readiness(g(1), None, None);
    assert!(matches!(result, Err(CoreError::NotReady(_))));
}

#[test]
fn hybrid_top_k_zero_is_invalid() {
    assert_eq!(
        typed_code_or_debug(HybridOrchestratorPolicy::validate_top_k(0)),
        "HYB_TOP_K_INVALID"
    );
}

#[test]
fn hybrid_top_k_above_ceiling_is_invalid() {
    assert_eq!(
        typed_code_or_debug(HybridOrchestratorPolicy::validate_top_k(
            quanta_index_core::SemanticPolicy::max_top_k() + 1,
        )),
        "HYB_TOP_K_INVALID"
    );
}

#[test]
fn hybrid_overfetch_applies_floor_and_ceiling() {
    assert_eq!(HybridOrchestratorPolicy::over_fetch_top_k(1), 100);
    assert_eq!(HybridOrchestratorPolicy::over_fetch_top_k(50), 100);
    assert_eq!(HybridOrchestratorPolicy::over_fetch_top_k(500), 500);
    assert_eq!(
        HybridOrchestratorPolicy::over_fetch_top_k(20_000),
        quanta_index_core::SemanticPolicy::max_top_k()
    );
}

#[test]
fn rrf_disjoint_sets_orders_by_combined_score() {
    let lex = [candidate("a"), candidate("b")];
    let sem = [candidate("c"), candidate("d")];
    let fused = HybridOrchestratorPolicy::fuse_rrf(&lex, &sem, 10);
    let ids: Vec<&str> = fused.iter().map(|c| c.candidate_id.as_str()).collect();
    // Rank 1 candidates "a" (lex) and "c" (sem) have equal RRF score. Tiebreak
    // prefers in_lex=true ("a"), then by candidate_id. So order:
    //   a (rank1 lex), c (rank1 sem), b (rank2 lex), d (rank2 sem)
    assert_eq!(ids, vec!["a", "c", "b", "d"]);
}

#[test]
fn rrf_lex_only_returns_lex_order() {
    let lex = [candidate("x"), candidate("y"), candidate("z")];
    let fused = HybridOrchestratorPolicy::fuse_rrf(&lex, &[], 10);
    let ids: Vec<&str> = fused.iter().map(|c| c.candidate_id.as_str()).collect();
    assert_eq!(ids, vec!["x", "y", "z"]);
}

#[test]
fn rrf_sem_only_returns_sem_order() {
    let sem = [candidate("x"), candidate("y"), candidate("z")];
    let fused = HybridOrchestratorPolicy::fuse_rrf(&[], &sem, 10);
    let ids: Vec<&str> = fused.iter().map(|c| c.candidate_id.as_str()).collect();
    assert_eq!(ids, vec!["x", "y", "z"]);
}

#[test]
fn rrf_top_k_zero_returns_empty() {
    let lex = [candidate("a"), candidate("b")];
    let fused = HybridOrchestratorPolicy::fuse_rrf(&lex, &[], 0);
    assert!(fused.is_empty());
}

#[test]
fn rrf_top_k_one_returns_singleton() {
    let lex = [candidate("a"), candidate("b"), candidate("c")];
    let fused = HybridOrchestratorPolicy::fuse_rrf(&lex, &[], 1);
    assert_eq!(fused.len(), 1);
    assert_eq!(fused.first().map(|c| c.candidate_id.as_str()), Some("a"));
}

#[test]
fn rrf_shared_id_aggregates_score() {
    // Same candidate "a" in both lists at rank 1: should be top by combined
    // score and "b" / "c" follow by their per-list rank-2 contributions.
    let lex = [candidate("a"), candidate("b")];
    let sem = [candidate("a"), candidate("c")];
    let fused = HybridOrchestratorPolicy::fuse_rrf(&lex, &sem, 10);
    let ids: Vec<&str> = fused.iter().map(|c| c.candidate_id.as_str()).collect();
    // "a" gets 2 * 1/(60+1) = 0.0328; "b" and "c" each get 1/(60+2) = 0.0161
    // Both b and c tie in score; tiebreak: b is in_lex, so b before c.
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[test]
fn rrf_shared_id_preserves_lexical_payload() {
    let mut lexical = candidate("shared");
    lexical.repo_relative_path = RepoRelativePath::new("src/lexical.rs");
    lexical.start_line = 10;
    lexical.end_line = 12;
    lexical.score = 0.875;
    lexical.snippet = "lexical match".to_string();
    lexical.snippet_hit_offset = Some(2);
    lexical.highlights = vec![HighlightSpan { start: 2, len: 7 }];

    let mut semantic = candidate("shared");
    semantic.repo_relative_path = RepoRelativePath::new("src/semantic.rs");
    semantic.start_line = 40;
    semantic.end_line = 45;
    semantic.score = 0.25;
    semantic.snippet = "semantic payload".to_string();
    semantic.snippet_hit_offset = None;
    semantic.highlights = Vec::new();

    let fused = HybridOrchestratorPolicy::fuse_rrf(&[lexical.clone()], &[semantic], 1);

    assert_eq!(fused, vec![lexical]);
}

#[test]
fn rrf_empty_inputs_returns_empty() {
    let fused = HybridOrchestratorPolicy::fuse_rrf(&[], &[], 10);
    assert!(fused.is_empty());
}
