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
        let lexical_ids = lexical
            .iter()
            .map(|candidate| candidate.candidate_id.clone())
            .collect::<Vec<_>>();
        let semantic_ids = semantic
            .iter()
            .map(|candidate| candidate.candidate_id.clone())
            .collect::<Vec<_>>();
        let mut candidates = lexical
            .iter()
            .chain(semantic)
            .map(|candidate| (candidate.candidate_id.clone(), candidate.clone()))
            .collect::<BTreeMap<_, _>>();
        Self::fuse_rrf_ids(&lexical_ids, &semantic_ids, top_k)
            .into_iter()
            .filter_map(|candidate_id| candidates.remove(candidate_id.as_str()))
            .collect()
    }

    /// Fuse ranked stable identities without manufacturing a result DTO.
    ///
    /// This is the authority for entity-level fusion where callers own a DTO
    /// other than `LexicalCandidate`. Tie-break order matches [`Self::fuse_rrf`]:
    /// higher fused score, presence in the first lane, then identity.
    #[must_use]
    pub fn fuse_rrf_ids(lexical: &[String], semantic: &[String], top_k: u32) -> Vec<String> {
        Self::fuse_rrf_id_lanes(&[lexical, semantic], top_k)
    }

    /// Fuse any number of independently ranked stable-identity lanes.
    ///
    /// The first lane owns the deterministic preferred-lane tie break. This
    /// keeps BM25 behavior stable while allowing each semantic corpus to retain
    /// an independent rank domain.
    #[must_use]
    pub fn fuse_rrf_id_lanes(lanes: &[&[String]], top_k: u32) -> Vec<String> {
        let mut accs: BTreeMap<String, IdFuseAccumulator> = BTreeMap::new();
        for (lane_index, lane) in lanes.iter().enumerate() {
            accumulate_ids(&mut accs, lane, lane_index == 0);
        }

        let mut fused: Vec<IdFuseAccumulator> = accs.into_values().collect();
        fused.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(b.in_lex.cmp(&a.in_lex))
                .then_with(|| a.candidate_id.cmp(&b.candidate_id))
        });
        // Saturating cap: top_k larger than usize::MAX (only possible on
        // <64-bit targets) is treated as unlimited. `map_or` is the
        // clippy-preferred shape; `unwrap_or` is workspace-disallowed.
        let limit = usize::try_from(top_k).map_or(usize::MAX, |n| n);
        fused
            .into_iter()
            .take(limit)
            .map(|acc| acc.candidate_id)
            .collect()
    }
}

struct IdFuseAccumulator {
    score: f64,
    in_lex: bool,
    candidate_id: String,
}

fn accumulate_ids(accs: &mut BTreeMap<String, IdFuseAccumulator>, ranked: &[String], is_lex: bool) {
    for (rank, candidate_id) in ranked.iter().enumerate() {
        let rank_plus_one = rank.saturating_add(1);
        // Saturating: rank index past u32::MAX collapses to the same RRF
        // tail score. `map_or` keeps the clippy + workspace lints happy.
        let rank_u32 = u32::try_from(rank_plus_one).map_or(u32::MAX, |n| n);
        let rank_f = f64::from(rank_u32);
        let entry = accs
            .entry(candidate_id.clone())
            .or_insert_with(|| IdFuseAccumulator {
                score: 0.0,
                in_lex: false,
                candidate_id: candidate_id.clone(),
            });
        entry.score += 1.0 / (RRF_K + rank_f);
        if is_lex {
            entry.in_lex = true;
        }
    }
}

#[cfg(test)]
mod tests {
    //! Mutation-kill coverage for `HybridOrchestratorPolicy`.
    //!
    //! Equivalent mutations in `over_fetch_top_k` are documented but not
    //! assertable here because the boundary values collapse to the same
    //! observable result. `.cargo/mutants.toml` excludes only those two exact
    //! survivor names so any future change to this function reopens review.

    use super::*;
    use crate::domains::semantic::SemanticPolicy;
    use quanta_index_contract::{
        LexicalCandidate, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    };

    #[test]
    fn validate_top_k_boundary_kills_gt_to_ge_mutation() {
        let max = SemanticPolicy::max_top_k();
        assert!(HybridOrchestratorPolicy::validate_top_k(max).is_ok());
        assert!(HybridOrchestratorPolicy::validate_top_k(max + 1).is_err());
    }

    fn cand(id: &str) -> LexicalCandidate {
        LexicalCandidate {
            candidate_id: id.to_string(),
            repo_id: RepoId::new("repo-hybrid"),
            revision_id: RevisionId::new("rev-hybrid"),
            manifest_generation: ManifestGeneration::new(1),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            start_line: 0,
            end_line: 0,
            score: 0.0,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }

    #[test]
    fn fuse_rrf_kills_add_to_mul_mutation_in_accumulate() {
        // A appears at rank 5 in both lists; B appears only at rank 1 in lex.
        // Under `+` (original): A=2/65≈0.0308 > B=1/61≈0.0164 → top-1 = A.
        // Under `*` (mutation): A=2/300≈0.0067 < B=1/60≈0.0167 → top-1 = B.
        let lexical = vec![cand("B"), cand("X1"), cand("X2"), cand("X3"), cand("A")];
        let semantic = vec![cand("Y1"), cand("Y2"), cand("Y3"), cand("Y4"), cand("A")];
        let fused = HybridOrchestratorPolicy::fuse_rrf(&lexical, &semantic, 1);
        assert_eq!(fused.len(), 1);
        let top = fused
            .first()
            .map(|candidate| candidate.candidate_id.as_str());
        assert_eq!(top, Some("A"));
    }
}
