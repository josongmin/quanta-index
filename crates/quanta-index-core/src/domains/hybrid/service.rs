use std::collections::BTreeMap;

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{LexicalCandidate, ManifestGeneration};

use crate::{domains::semantic::SemanticPolicy, error::CoreError};

/// Reciprocal Rank Fusion constant. Tunable but fixed here so all callers
/// produce identical fused rankings.
const RRF_K: f64 = 60.0;
const MIN_INTERNAL_FETCH_K: u32 = 100;

#[derive(Debug, Default, Clone, Copy)]
pub struct HybridOrchestratorPolicy;

impl HybridOrchestratorPolicy {
    pub fn validate_top_k(top_k: u32) -> Result<(), CoreError> {
        let max_top_k = SemanticPolicy::max_top_k();
        if top_k == 0 || top_k > max_top_k {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::HybTopKInvalid.as_code_str().to_string(),
                message: format!("hybrid: top_k must be within 1..={max_top_k}, got {top_k}"),
            });
        }
        Ok(())
    }

    #[must_use]
    pub const fn over_fetch_top_k(top_k: u32) -> u32 {
        let floored = if top_k < MIN_INTERNAL_FETCH_K {
            MIN_INTERNAL_FETCH_K
        } else {
            top_k
        };
        if floored > SemanticPolicy::max_top_k() {
            SemanticPolicy::max_top_k()
        } else {
            floored
        }
    }

    /// Require both tracks to have materialized the requested generation. If
    /// either is behind, query is `NOT_READY`.
    pub fn validate_joint_readiness(
        target: ManifestGeneration,
        lex_materialized: Option<ManifestGeneration>,
        sem_materialized: Option<ManifestGeneration>,
    ) -> Result<(), CoreError> {
        let lex_ok = lex_materialized.is_some_and(|g| g.get() >= target.get());
        let sem_ok = sem_materialized.is_some_and(|g| g.get() >= target.get());
        if !lex_ok || !sem_ok {
            return Err(CoreError::NotReady(format!(
                "hybrid: generation {} not jointly ready (lex_ok={lex_ok}, sem_ok={sem_ok})",
                target.get()
            )));
        }
        Ok(())
    }

    /// Fuse two ranked candidate lists by RRF.
    ///
    /// Tie-break order: higher fused score, then "present in lexical list",
    /// then `candidate_id` lexicographic.
    #[must_use]
    pub fn fuse_rrf(
        lexical: &[LexicalCandidate],
        semantic: &[LexicalCandidate],
        top_k: u32,
    ) -> Vec<LexicalCandidate> {
        let mut accs: BTreeMap<String, FuseAccumulator> = BTreeMap::new();
        accumulate(&mut accs, lexical, true);
        accumulate(&mut accs, semantic, false);

        let mut fused: Vec<FuseAccumulator> = accs.into_values().collect();
        fused.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.in_lex.cmp(&a.in_lex))
                .then_with(|| {
                    a.candidate
                        .candidate_id
                        .as_str()
                        .cmp(b.candidate.candidate_id.as_str())
                })
        });
        // Saturating cap: top_k larger than usize::MAX (only possible on
        // <64-bit targets) is treated as unlimited. `map_or` is the
        // clippy-preferred shape; `unwrap_or` is workspace-disallowed.
        let limit = usize::try_from(top_k).map_or(usize::MAX, |n| n);
        fused
            .into_iter()
            .take(limit)
            .map(|acc| acc.candidate)
            .collect()
    }
}

struct FuseAccumulator {
    score: f64,
    in_lex: bool,
    candidate: LexicalCandidate,
}

fn accumulate(
    accs: &mut BTreeMap<String, FuseAccumulator>,
    ranked: &[LexicalCandidate],
    is_lex: bool,
) {
    for (rank, c) in ranked.iter().enumerate() {
        let rank_plus_one = rank.saturating_add(1);
        // Saturating: rank index past u32::MAX collapses to the same RRF
        // tail score. `map_or` keeps the clippy + workspace lints happy.
        let rank_u32 = u32::try_from(rank_plus_one).map_or(u32::MAX, |n| n);
        let rank_f = f64::from(rank_u32);
        let key = c.candidate_id.clone();
        let entry = accs.entry(key).or_insert_with(|| FuseAccumulator {
            score: 0.0,
            in_lex: false,
            candidate: c.clone(),
        });
        entry.score += 1.0 / (RRF_K + rank_f);
        if is_lex {
            entry.in_lex = true;
        }
    }
}
