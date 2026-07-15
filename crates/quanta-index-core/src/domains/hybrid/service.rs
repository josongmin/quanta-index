use std::collections::{BTreeMap, BTreeSet};

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
    /// then `candidate_id` lexicographic. Each identity contributes at most
    /// once per lane. When both lanes contain the same identity, the lexical
    /// candidate owns the returned payload.
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
        let mut candidates = semantic
            .iter()
            .chain(lexical)
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
    /// an independent rank domain. Repeated identities within one lane are
    /// ignored before rank assignment.
    #[must_use]
    pub fn fuse_rrf_id_lanes(lanes: &[&[String]], top_k: u32) -> Vec<String> {
        Self::fuse_rrf_key_lanes(lanes, top_k)
    }

    /// Fuse any number of independently ranked, typed stable-identity lanes.
    ///
    /// This is the canonical entity-fusion primitive. String-only callers use
    /// [`Self::fuse_rrf_id_lanes`]; callers whose identity includes a kind or
    /// another typed discriminator must use this method so unrelated values
    /// with identical display text cannot collapse into one RRF candidate.
    #[must_use]
    pub fn fuse_rrf_key_lanes<T>(lanes: &[&[T]], top_k: u32) -> Vec<T>
    where
        T: Clone + Ord,
    {
        let mut accs: BTreeMap<T, KeyFuseAccumulator> = BTreeMap::new();
        for (lane_index, lane) in lanes.iter().enumerate() {
            accumulate_keys(&mut accs, lane, lane_index == 0);
        }

        let mut fused: Vec<(T, KeyFuseAccumulator)> = accs.into_iter().collect();
        fused.sort_by(|(left_key, left), (right_key, right)| {
            right
                .score
                .total_cmp(&left.score)
                .then(right.in_lex.cmp(&left.in_lex))
                .then_with(|| left_key.cmp(right_key))
        });
        // Saturating cap: top_k larger than usize::MAX (only possible on
        // <64-bit targets) is treated as unlimited. `map_or` is the
        // clippy-preferred shape; `unwrap_or` is workspace-disallowed.
        let limit = usize::try_from(top_k).map_or(usize::MAX, |n| n);
        fused
            .into_iter()
            .take(limit)
            .map(|(key, _acc)| key)
            .collect()
    }
}

struct KeyFuseAccumulator {
    score: f64,
    in_lex: bool,
}

fn accumulate_keys<T>(accs: &mut BTreeMap<T, KeyFuseAccumulator>, ranked: &[T], is_lex: bool)
where
    T: Clone + Ord,
{
    // A ranked lane is an ordering of identities, not a multiset. Ignore a
    // duplicate before assigning rank so malformed adapter output cannot
    // amplify a candidate or displace later unique identities.
    let mut seen = BTreeSet::<&T>::new();
    for key in ranked {
        if !seen.insert(key) {
            continue;
        }
        let rank_plus_one = seen.len();
        // Saturating: rank index past u32::MAX collapses to the same RRF
        // tail score. `map_or` keeps the clippy + workspace lints happy.
        let rank_u32 = u32::try_from(rank_plus_one).map_or(u32::MAX, |n| n);
        let rank_f = f64::from(rank_u32);
        let entry = accs
            .entry(key.clone())
            .or_insert_with(|| KeyFuseAccumulator {
                score: 0.0,
                in_lex: false,
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

    #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
    struct TypedKey {
        kind: &'static str,
        id: &'static str,
    }

    #[test]
    fn typed_fusion_keeps_distinct_kinds_with_the_same_display_id() {
        let chunk = TypedKey {
            kind: "chunk",
            id: "same-text",
        };
        let symbol = TypedKey {
            kind: "symbol",
            id: "same-text",
        };
        let fused = HybridOrchestratorPolicy::fuse_rrf_key_lanes(
            &[std::slice::from_ref(&chunk), std::slice::from_ref(&symbol)],
            2,
        );
        assert_eq!(fused, vec![chunk, symbol]);
    }
}
