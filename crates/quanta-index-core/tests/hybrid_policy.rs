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

// Independent, deliberately simple RRF oracle for the public typed-key fusion
// primitive. It uses linear lookup instead of the production map and
// de-duplicates each lane before assigning ranks, so this test does not
// reproduce the implementation's accumulator mechanics.
fn slow_rrf_oracle(lanes: &[&[String]]) -> Vec<String> {
    #[derive(Debug)]
    struct Entry {
        key: String,
        score: f64,
        in_first_lane: bool,
    }

    let mut entries = Vec::<Entry>::new();
    for (lane_index, lane) in lanes.iter().enumerate() {
        let mut seen = Vec::<String>::new();
        for key in *lane {
            if seen.iter().any(|seen_key| seen_key == key) {
                continue;
            }
            seen.push(key.clone());
            let rank = u32::try_from(seen.len()).map_or(f64::INFINITY, f64::from);
            let contribution = 1.0 / (60.0 + rank);
            if let Some(entry) = entries.iter_mut().find(|entry| entry.key == *key) {
                entry.score += contribution;
                entry.in_first_lane |= lane_index == 0;
            } else {
                entries.push(Entry {
                    key: key.clone(),
                    score: contribution,
                    in_first_lane: lane_index == 0,
                });
            }
        }
    }

    entries.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then(right.in_first_lane.cmp(&left.in_first_lane))
            .then_with(|| left.key.cmp(&right.key))
    });
    entries.into_iter().map(|entry| entry.key).collect()
}

fn strings(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
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

#[test]
fn rrf_key_fusion_matches_independent_oracle_across_rank_shapes() {
    let lexical = strings(&["lex-only", "shared", "lex-tail"]);
    let semantic_a = strings(&["semantic-a", "shared", "semantic-tail"]);
    let semantic_b = strings(&["semantic-b", "shared", "semantic-tail"]);
    let lanes = [&lexical[..], &semantic_a[..], &semantic_b[..]];

    let expected = slow_rrf_oracle(&lanes);
    let actual = HybridOrchestratorPolicy::fuse_rrf_key_lanes(&lanes, u32::MAX);

    assert_eq!(actual, expected);
}

#[test]
fn rrf_nonpreferred_lane_permutation_is_invariant() {
    let lexical = strings(&["lex-only", "shared", "lex-tail"]);
    let semantic_a = strings(&["semantic-a", "shared", "semantic-tail"]);
    let semantic_b = strings(&["semantic-b", "shared", "semantic-tail"]);
    let original = [&lexical[..], &semantic_a[..], &semantic_b[..]];
    let permuted = [&lexical[..], &semantic_b[..], &semantic_a[..]];

    let original_result = HybridOrchestratorPolicy::fuse_rrf_key_lanes(&original, u32::MAX);
    let permuted_result = HybridOrchestratorPolicy::fuse_rrf_key_lanes(&permuted, u32::MAX);

    assert_eq!(original_result, slow_rrf_oracle(&original));
    assert_eq!(permuted_result, slow_rrf_oracle(&permuted));
    assert_eq!(original_result, permuted_result);
}

#[test]
fn rrf_top_k_is_a_prefix_of_the_unbounded_order() {
    let lexical = strings(&["lex-only", "shared", "lex-tail"]);
    let semantic_a = strings(&["semantic-a", "shared", "semantic-tail"]);
    let semantic_b = strings(&["semantic-b", "shared", "semantic-tail"]);
    let lanes = [&lexical[..], &semantic_a[..], &semantic_b[..]];
    let full = HybridOrchestratorPolicy::fuse_rrf_key_lanes(&lanes, u32::MAX);

    for top_k in 0..=(full.len() + 2) {
        let bounded = HybridOrchestratorPolicy::fuse_rrf_key_lanes(
            &lanes,
            u32::try_from(top_k).expect("test top_k fits into u32"),
        );
        let expected = full
            .get(..top_k.min(full.len()))
            .expect("bounded test prefix is in range");
        assert_eq!(bounded, expected);
    }
}

#[test]
fn rrf_duplicate_identity_within_a_lane_is_idempotent() {
    // Duplicating A must neither add a second RRF contribution nor push B/C
    // down a rank. The duplicated input used to promote A above the two-lane
    // winner B, making this a semantic rather than cardinality-only check.
    let lexical = strings(&["B", "A", "A", "A"]);
    let semantic = strings(&["B", "C"]);
    let lanes = [&lexical[..], &semantic[..]];

    let expected = slow_rrf_oracle(&lanes);
    let actual = HybridOrchestratorPolicy::fuse_rrf_key_lanes(&lanes, u32::MAX);

    assert_eq!(expected, strings(&["B", "A", "C"]));
    assert_eq!(actual, expected);
}

#[test]
fn rrf_ties_prefer_first_lane_then_identity() {
    let lexical = strings(&["b"]);
    let semantic = strings(&["a"]);
    let lexical_tie = [&lexical[..], &semantic[..]];
    assert_eq!(
        HybridOrchestratorPolicy::fuse_rrf_key_lanes(&lexical_tie, u32::MAX),
        strings(&["b", "a"])
    );

    let empty = Vec::<String>::new();
    let semantic_b = strings(&["b"]);
    let semantic_a = strings(&["a"]);
    let no_first_lane = [&empty[..], &semantic_b[..], &semantic_a[..]];
    assert_eq!(
        HybridOrchestratorPolicy::fuse_rrf_key_lanes(&no_first_lane, u32::MAX),
        strings(&["a", "b"])
    );
}
