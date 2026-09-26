use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::{
    HybridCandidateV1, HybridLaneContributionV1, HybridLaneV1, INTERNAL_FETCH_CEILING,
    LexicalCandidate, ManifestGeneration,
};

use crate::{
    domains::semantic::SemanticPolicy,
    error::{CoreError, validate_query_top_k},
};

/// Reciprocal Rank Fusion constant. Tunable but fixed here so all callers
/// produce identical fused rankings.
const RRF_K: f64 = 60.0;
const MIN_INTERNAL_FETCH_K: u32 = 100;

/// Bounded startup-only fetch-floor experiment; production defaults to 100.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HybridFetchFloorPolicy {
    Floor25,
    Floor50,
    #[default]
    Floor100,
}

impl HybridFetchFloorPolicy {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "25" => Ok(Self::Floor25),
            "50" => Ok(Self::Floor50),
            "100" => Ok(Self::Floor100),
            _ => Err("hybrid fetch floor must be exactly 25, 50 or 100"),
        }
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        match self {
            Self::Floor25 => 25,
            Self::Floor50 => 50,
            Self::Floor100 => MIN_INTERNAL_FETCH_K,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Floor25 => "25",
            Self::Floor50 => "50",
            Self::Floor100 => "100",
        }
    }
}

/// Where one lane placed a fused identity: the lane's index in the fusion
/// input and the identity's 1-based rank in that lane (after the lane's
/// duplicates are dropped).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FusedLaneRankV1 {
    pub lane: usize,
    pub rank: u32,
}

/// A fused identity with the provenance that ranked it.
#[derive(Clone, Debug, PartialEq)]
pub struct FusedKeyV1<T> {
    pub key: T,
    /// The ranking key: the sum over `lanes` of `1 / (k + rank)` under
    /// [`HybridOrchestratorPolicy::rrf_k`], exactly as the fusion sorted it.
    pub fused_score: f64,
    /// Ascending lane index; each lane at most once.
    pub lanes: Vec<FusedLaneRankV1>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HybridOrchestratorPolicy;

impl HybridOrchestratorPolicy {
    /// Accept or refuse a caller's `top_k` under the shared contract.
    ///
    /// The hybrid route used to report its own `HYB_TOP_K_INVALID` for the
    /// same defect every other route reports differently; one policy, one code.
    pub fn validate_top_k(top_k: u32) -> Result<(), CoreError> {
        let _accepted = validate_query_top_k(top_k)?;
        Ok(())
    }

    #[must_use]
    pub const fn over_fetch_top_k(top_k: u32) -> u32 {
        Self::over_fetch_top_k_with_floor(top_k, HybridFetchFloorPolicy::Floor100)
    }

    #[must_use]
    pub const fn over_fetch_top_k_with_floor(top_k: u32, floor: HybridFetchFloorPolicy) -> u32 {
        let floored = if top_k < floor.get() {
            floor.get()
        } else {
            top_k
        };
        if floored > SemanticPolicy::max_top_k() {
            SemanticPolicy::max_top_k()
        } else {
            floored
        }
    }

    /// The most dense rows the admission loop examines for one query
    /// (QI-BB-018 보완 #3): the search plane's internal fetch ceiling.
    ///
    /// A dense lane under exact filters refetches at growing sizes until
    /// its admitted rows are as deep as an unfiltered lane; this bounds the
    /// work so a filter that admits almost nothing cannot make one query
    /// read the whole generation. The lane is `capped` past it, and the
    /// route's trace says so.
    #[must_use]
    pub const fn dense_admission_examine_ceiling() -> u32 {
        INTERNAL_FETCH_CEILING
    }

    /// The fetch size after `fetched` rows admitted too few (QI-BB-018
    /// 보완 #3): double, capped at
    /// [`Self::dense_admission_examine_ceiling`]; `None` once the ceiling
    /// itself was fetched, which ends the loop `capped`.
    #[must_use]
    pub const fn next_dense_admission_fetch(fetched: u32) -> Option<u32> {
        let ceiling = Self::dense_admission_examine_ceiling();
        if fetched >= ceiling {
            return None;
        }
        let doubled = fetched.saturating_mul(2);
        Some(if doubled > ceiling { ceiling } else { doubled })
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

    /// The RRF constant every fusion and every explain of one uses.
    #[must_use]
    pub const fn rrf_k() -> f64 {
        RRF_K
    }

    /// The RRF score of an identity placed at `ranks` (1-based, one per
    /// lane that saw it): the sum of `1 / (k + rank)`, accumulated in the
    /// given order from zero — the arithmetic [`Self::fuse_rrf_key_lanes_with_provenance`]
    /// sorts by, so a recompute over a fused row's lane ranks in lane order
    /// reproduces its `fused_score` exactly.
    #[must_use]
    pub fn rrf_score(ranks: impl IntoIterator<Item = u32>) -> f64 {
        ranks
            .into_iter()
            .fold(0.0_f64, |score, rank| score + rrf_term(rank))
    }

    /// Fuse the two hybrid lanes by RRF into result rows that carry their
    /// provenance (QI-BB-022).
    ///
    /// Lane 0 is the lexical lane and lane 1 the dense lane, both in their
    /// engines' ranked order. Each row's `candidate` is the preferred lane's
    /// row for the identity (lexical when that lane saw it, else dense), and
    /// its contributions are the lanes that saw it, in lane order, with the
    /// rank and the raw score each lane emitted. Ranking and tie-break are
    /// those of [`Self::fuse_rrf`], which is the same fusion without the
    /// provenance.
    ///
    /// A lane row whose score is not finite is a typed error: the contract
    /// refuses to carry it, so this refuses to fuse it.
    pub fn fuse_rrf_candidates(
        lexical: &[LexicalCandidate],
        semantic: &[LexicalCandidate],
        top_k: u32,
    ) -> Result<Vec<HybridCandidateV1>, CoreError> {
        let lexical_ids = lexical
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>();
        let semantic_ids = semantic
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<Vec<_>>();
        // The first row per identity is the one whose rank the fusion
        // counted, since a lane's repeated identities are dropped there.
        let lanes = [
            LaneRows::new(HybridLaneV1::Lexical, lexical),
            LaneRows::new(HybridLaneV1::Dense, semantic),
        ];
        Self::fuse_rrf_key_lanes_with_provenance(&[&lexical_ids, &semantic_ids], top_k)
            .into_iter()
            .map(|fused| hybrid_candidate_from_fused(&fused, &lanes))
            .collect()
    }

    /// Fuse two ranked candidate lists by RRF.
    ///
    /// Tie-break order: higher fused score, then "present in lexical list",
    /// then `candidate_id` lexicographic. Each identity contributes at most
    /// once per lane. When both lanes contain the same identity, the lexical
    /// candidate owns the returned payload. This is
    /// [`Self::fuse_rrf_candidates`] without the provenance and without the
    /// finite-score gate: the rows it returns are the lane rows as given.
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
    /// String-only callers use [`Self::fuse_rrf_id_lanes`]; callers whose
    /// identity includes a kind or another typed discriminator must use this
    /// method so unrelated values with identical display text cannot
    /// collapse into one RRF candidate. This is
    /// [`Self::fuse_rrf_key_lanes_with_provenance`] keeping only the keys.
    #[must_use]
    pub fn fuse_rrf_key_lanes<T>(lanes: &[&[T]], top_k: u32) -> Vec<T>
    where
        T: Clone + Ord,
    {
        Self::fuse_rrf_key_lanes_with_provenance(lanes, top_k)
            .into_iter()
            .map(|fused| fused.key)
            .collect()
    }

    /// Fuse any number of independently ranked, typed stable-identity lanes,
    /// keeping each winner's provenance.
    ///
    /// This is the one RRF algorithm every other fusion entry point wraps.
    /// Each lane is an ordering of identities: a repeated identity is
    /// dropped before ranks are assigned, so it can neither amplify itself
    /// nor displace the identities after it. Every identity scores the sum
    /// of `1 / (k + rank)` over the lanes that saw it; the order is that
    /// score descending, then identities the first lane saw before those it
    /// did not, then the identity's own order; the first `top_k` win.
    #[must_use]
    pub fn fuse_rrf_key_lanes_with_provenance<T>(lanes: &[&[T]], top_k: u32) -> Vec<FusedKeyV1<T>>
    where
        T: Clone + Ord,
    {
        let mut accs: BTreeMap<T, KeyFuseAccumulator> = BTreeMap::new();
        for (lane_index, lane) in lanes.iter().enumerate() {
            accumulate_keys(&mut accs, lane, lane_index);
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
            .map(|(key, acc)| FusedKeyV1 {
                key,
                fused_score: acc.score,
                lanes: acc.lanes,
            })
            .collect()
    }
}

/// One RRF term: `1 / (k + rank)`.
fn rrf_term(rank: u32) -> f64 {
    1.0 / (RRF_K + f64::from(rank))
}

struct KeyFuseAccumulator {
    score: f64,
    in_lex: bool,
    lanes: Vec<FusedLaneRankV1>,
}

fn accumulate_keys<T>(accs: &mut BTreeMap<T, KeyFuseAccumulator>, ranked: &[T], lane_index: usize)
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
        let rank = u32::try_from(rank_plus_one).map_or(u32::MAX, |n| n);
        let entry = accs
            .entry(key.clone())
            .or_insert_with(|| KeyFuseAccumulator {
                score: 0.0,
                in_lex: false,
                lanes: Vec::new(),
            });
        entry.score += rrf_term(rank);
        if lane_index == 0 {
            entry.in_lex = true;
        }
        entry.lanes.push(FusedLaneRankV1 {
            lane: lane_index,
            rank,
        });
    }
}

/// One hybrid lane's rows, addressable by identity.
struct LaneRows<'a> {
    lane: HybridLaneV1,
    by_id: BTreeMap<&'a str, &'a LexicalCandidate>,
}

impl<'a> LaneRows<'a> {
    fn new(lane: HybridLaneV1, rows: &'a [LexicalCandidate]) -> Self {
        let mut by_id = BTreeMap::new();
        for row in rows {
            // First occurrence wins: it is the one the fusion ranked.
            let _first_occurrence = by_id.entry(row.candidate_id.as_str()).or_insert(row);
        }
        Self { lane, by_id }
    }
}

/// Assemble one fused identity's result row from the lanes that saw it.
fn hybrid_candidate_from_fused(
    fused: &FusedKeyV1<&str>,
    lanes: &[LaneRows<'_>; 2],
) -> Result<HybridCandidateV1, CoreError> {
    let mut contributions = Vec::with_capacity(fused.lanes.len());
    let mut preferred: Option<&LexicalCandidate> = None;
    for placement in &fused.lanes {
        let Some(lane_rows) = lanes.get(placement.lane) else {
            return Err(CoreError::Storage(format!(
                "hybrid: fusion placed {} in lane {}, which is not a hybrid lane",
                fused.key, placement.lane
            )));
        };
        let Some(row) = lane_rows.by_id.get(fused.key) else {
            return Err(CoreError::Storage(format!(
                "hybrid: fusion ranked {} in the {} lane but that lane has no such row",
                fused.key,
                lane_rows.lane.as_code_str()
            )));
        };
        if !row.score.is_finite() {
            return Err(CoreError::Storage(format!(
                "hybrid: the {} lane emitted a non-finite score for {}",
                lane_rows.lane.as_code_str(),
                fused.key
            )));
        }
        contributions.push(HybridLaneContributionV1 {
            lane: lane_rows.lane,
            rank: placement.rank,
            raw_score: row.score,
        });
        if preferred.is_none() {
            preferred = Some(row);
        }
    }
    let Some(preferred) = preferred else {
        return Err(CoreError::Storage(format!(
            "hybrid: fusion returned {} without a lane that saw it",
            fused.key
        )));
    };
    Ok(HybridCandidateV1 {
        candidate: preferred.clone(),
        fused_score: fused.fused_score,
        contributions,
    })
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
    fn hybrid_fetch_floor_policy_is_closed_and_defaults_to_production() {
        assert_eq!(
            HybridFetchFloorPolicy::default(),
            HybridFetchFloorPolicy::Floor100
        );
        for (text, value) in [("25", 25), ("50", 50), ("100", 100)] {
            let policy = HybridFetchFloorPolicy::parse(text).expect("bounded selector");
            assert_eq!(policy.as_str(), text);
            assert_eq!(policy.get(), value);
        }
        for invalid in [
            "", "0", "10", "24", "26", "51", "101", "025", "25 ", "+25", "default",
        ] {
            assert!(HybridFetchFloorPolicy::parse(invalid).is_err());
        }
    }

    #[test]
    fn hybrid_fetch_floor_preserves_default_and_ceiling() {
        for top_k in [0, 1, 10, 25, 50, 100, SemanticPolicy::max_top_k(), u32::MAX] {
            assert_eq!(
                HybridOrchestratorPolicy::over_fetch_top_k(top_k),
                top_k.max(100).min(SemanticPolicy::max_top_k()),
            );
            for floor in [
                HybridFetchFloorPolicy::Floor25,
                HybridFetchFloorPolicy::Floor50,
                HybridFetchFloorPolicy::Floor100,
            ] {
                assert_eq!(
                    HybridOrchestratorPolicy::over_fetch_top_k_with_floor(top_k, floor),
                    top_k.max(floor.get()).min(SemanticPolicy::max_top_k()),
                );
            }
        }
        assert_eq!(
            HybridOrchestratorPolicy::next_dense_admission_fetch(25),
            Some(50)
        );
        assert_eq!(
            HybridOrchestratorPolicy::next_dense_admission_fetch(50),
            Some(100)
        );
        assert_eq!(
            HybridOrchestratorPolicy::next_dense_admission_fetch(INTERNAL_FETCH_CEILING),
            None
        );
    }

    #[test]
    fn validate_top_k_boundary_kills_gt_to_ge_mutation() {
        let max = SemanticPolicy::max_top_k();
        assert!(HybridOrchestratorPolicy::validate_top_k(max).is_ok());
        assert!(HybridOrchestratorPolicy::validate_top_k(max + 1).is_err());
    }

    fn cand(id: &str) -> LexicalCandidate {
        LexicalCandidate {
            candidate_id: id.to_string(),
            repo_id: RepoId::new("repo-hybrid")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-hybrid")
                .expect("static fixture ID satisfies canonical policy"),
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
    fn dense_admission_refill_doubles_to_the_ceiling_then_stops() {
        let ceiling = HybridOrchestratorPolicy::dense_admission_examine_ceiling();
        assert_eq!(ceiling, quanta_index_contract::INTERNAL_FETCH_CEILING);
        assert_eq!(
            HybridOrchestratorPolicy::next_dense_admission_fetch(100),
            Some(200)
        );
        assert_eq!(
            HybridOrchestratorPolicy::next_dense_admission_fetch(6_400),
            Some(ceiling),
            "the last refill is cut to the ceiling"
        );
        assert_eq!(
            HybridOrchestratorPolicy::next_dense_admission_fetch(ceiling),
            None,
            "a fetch of the ceiling ends the loop"
        );
        assert_eq!(
            HybridOrchestratorPolicy::next_dense_admission_fetch(u32::MAX),
            None
        );
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
