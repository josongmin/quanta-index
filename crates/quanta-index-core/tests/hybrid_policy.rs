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

#[expect(
    clippy::expect_used,
    reason = "static fixture IDs provably satisfy the canonical ID policy"
)]
fn candidate(id: &str) -> LexicalCandidate {
    LexicalCandidate {
        candidate_id: id.to_string(),
        repo_id: RepoId::new("r").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("v").expect("static fixture ID satisfies canonical policy"),
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
        Err(CoreError::Typed { code, .. }) => code.to_string(),
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

// `top_k` is one contract for every route (QI-BB-025): the hybrid route reports
// the shared `QUERY_TOP_K_OUT_OF_RANGE`, not a route-private code, so an SDK
// caller sees one refusal regardless of which route it asked.
#[test]
fn hybrid_top_k_zero_is_invalid() {
    assert_eq!(
        typed_code_or_debug(HybridOrchestratorPolicy::validate_top_k(0)),
        quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE
    );
}

#[test]
fn hybrid_top_k_above_ceiling_is_invalid() {
    assert_eq!(
        typed_code_or_debug(HybridOrchestratorPolicy::validate_top_k(
            quanta_index_core::SemanticPolicy::max_top_k() + 1,
        )),
        quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE
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

// QI-BB-022: the provenance-carrying fusion reports each winner's lane
// positions as given and a fused score equal to an independently recomputed
// RRF sum, in exactly the order the key-only fusion returns.
#[test]
fn rrf_provenance_reports_input_positions_and_the_recomputed_score() -> TestResult {
    use quanta_index_core::FusedLaneRankV1;
    let lexical = strings(&["lex-only", "shared", "lex-tail"]);
    let semantic_a = strings(&["semantic-a", "shared", "semantic-tail"]);
    let semantic_b = strings(&["semantic-b", "shared", "semantic-tail", "lex-tail"]);
    let lanes = [&lexical[..], &semantic_a[..], &semantic_b[..]];

    let fused = HybridOrchestratorPolicy::fuse_rrf_key_lanes_with_provenance(&lanes, u32::MAX);
    let keys = fused
        .iter()
        .map(|entry| entry.key.clone())
        .collect::<Vec<_>>();
    if keys != HybridOrchestratorPolicy::fuse_rrf_key_lanes(&lanes, u32::MAX) {
        return Err(format!("provenance must not move the ranking: {keys:?}").into());
    }
    for entry in &fused {
        // Every lane that lists the key is reported once, in lane order, at
        // the key's 1-based position in that lane; no other lane is.
        let mut expected_lanes = Vec::new();
        for (lane, rows) in lanes.iter().enumerate() {
            if let Some(index) = rows.iter().position(|row| *row == entry.key) {
                let rank = u32::try_from(index.saturating_add(1))?;
                expected_lanes.push(FusedLaneRankV1 { lane, rank });
            }
        }
        if entry.lanes != expected_lanes {
            return Err(format!(
                "{} placed at {:?}, expected {expected_lanes:?}",
                entry.key, entry.lanes
            )
            .into());
        }
        let recomputed: f64 = entry
            .lanes
            .iter()
            .map(|placement| 1.0 / (60.0 + f64::from(placement.rank)))
            .sum();
        if entry.fused_score.to_bits() != recomputed.to_bits() {
            return Err(format!(
                "{} scored {} but its ranks sum to {recomputed}",
                entry.key, entry.fused_score
            )
            .into());
        }
        if HybridOrchestratorPolicy::rrf_score(entry.lanes.iter().map(|placement| placement.rank))
            .to_bits()
            != entry.fused_score.to_bits()
        {
            return Err(format!("rrf_score must reproduce {}'s fused score", entry.key).into());
        }
    }
    let shared = fused
        .iter()
        .find(|entry| entry.key == "shared")
        .ok_or("shared fused")?;
    if shared.lanes.len() != 3 || shared.fused_score.to_bits() != (3.0_f64 / 62.0).to_bits() {
        return Err(format!("shared is rank 2 in all three lanes: {shared:?}").into());
    }
    Ok(())
}

// The provenance is over the de-duplicated lane, like the score.
#[test]
fn rrf_provenance_ranks_the_deduplicated_lane() -> TestResult {
    let lexical = strings(&["B", "A", "A", "A", "C"]);
    let lanes = [&lexical[..]];
    let fused = HybridOrchestratorPolicy::fuse_rrf_key_lanes_with_provenance(&lanes, u32::MAX);
    let placements = fused
        .iter()
        .map(|entry| {
            (
                entry.key.as_str(),
                entry
                    .lanes
                    .iter()
                    .map(|placement| placement.rank)
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    if placements != [("B", vec![1]), ("A", vec![2]), ("C", vec![3])] {
        return Err(format!("duplicates must not consume ranks: {placements:?}").into());
    }
    Ok(())
}

// The candidate-level fusion is the lane-DTO fusion plus provenance: the
// same rows in the same order, each with the lanes that saw it.
#[test]
fn rrf_candidates_carry_the_lane_rows_and_provenance_in_the_lane_dto_order() -> TestResult {
    use quanta_index_contract::{HybridLaneContributionV1, HybridLaneV1};
    let mut lex_shared = candidate("shared");
    lex_shared.score = 2.5;
    let mut lex_only = candidate("lex-only");
    lex_only.score = 1.5;
    let mut sem_shared = candidate("shared");
    sem_shared.score = 0.75;
    sem_shared.snippet = "dense payload".to_string();
    let mut sem_only = candidate("sem-only");
    sem_only.score = -0.25;
    let lexical = [lex_shared, lex_only];
    let semantic = [sem_only, sem_shared];

    let rows = HybridOrchestratorPolicy::fuse_rrf_candidates(&lexical, &semantic, 10)?;
    let lane_rows = rows
        .iter()
        .map(|row| row.candidate.clone())
        .collect::<Vec<_>>();
    if lane_rows != HybridOrchestratorPolicy::fuse_rrf(&lexical, &semantic, 10) {
        return Err(format!("candidate fusion must carry fuse_rrf's rows: {rows:?}").into());
    }
    let contributions = rows
        .iter()
        .map(|row| {
            (
                row.candidate.candidate_id.as_str(),
                row.contributions.clone(),
            )
        })
        .collect::<Vec<_>>();
    let expected = [
        (
            "shared",
            vec![
                HybridLaneContributionV1 {
                    lane: HybridLaneV1::Lexical,
                    rank: 1,
                    raw_score: 2.5,
                },
                HybridLaneContributionV1 {
                    lane: HybridLaneV1::Dense,
                    rank: 2,
                    raw_score: 0.75,
                },
            ],
        ),
        // The dense lane's rank-1 row outscores the lexical lane's rank-2 row.
        (
            "sem-only",
            vec![HybridLaneContributionV1 {
                lane: HybridLaneV1::Dense,
                rank: 1,
                raw_score: -0.25,
            }],
        ),
        (
            "lex-only",
            vec![HybridLaneContributionV1 {
                lane: HybridLaneV1::Lexical,
                rank: 2,
                raw_score: 1.5,
            }],
        ),
    ];
    if contributions != expected {
        return Err(format!("provenance: {contributions:?}").into());
    }
    // The carried row is the preferred lane's, so its score is that lane's
    // raw score and the contract's row invariant holds for every row.
    for row in &rows {
        row.validate_v1()?;
    }
    if rows.first().map(|row| row.candidate.snippet.as_str()) != Some("") {
        return Err("the lexical lane's row is carried when it saw the identity".into());
    }
    Ok(())
}

// A lane row the contract cannot carry is a typed refusal, not a row.
#[test]
fn rrf_candidates_refuse_a_non_finite_lane_score() {
    let mut poisoned = candidate("poisoned");
    poisoned.score = f32::NAN;
    let result = HybridOrchestratorPolicy::fuse_rrf_candidates(&[poisoned], &[], 10);
    assert!(
        matches!(result, Err(CoreError::Storage(ref message)) if message.contains("non-finite")),
        "{result:?}"
    );
}
