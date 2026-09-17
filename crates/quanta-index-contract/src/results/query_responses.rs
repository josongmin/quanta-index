use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    AuxEpochV1, CommitCandidate, DiffCandidate, GenerationPin, HistoryCursor, LexicalCandidate,
    ManifestGeneration, OwnerDocKind, QueryResultWindowV1, RepoId, RepoRelativePath, RevisionId,
    RuntimeMetadataCursorV1, SemanticCorpusKindV1, StructuralCandidate, StructuralCursorV1,
    lex::{SymbolKindCode, SymbolKindFamily},
};

use super::{CandidatePresenceV1, SearchExplanation};

#[derive(Clone, Debug, PartialEq)]
pub struct TextQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub window: QueryResultWindowV1,
    pub file_owner_rows: Option<Vec<FileOwnerProjectionRow>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileOwnerProjectionRow {
    pub candidate_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub repo_relative_path: RepoRelativePath,
    pub owners: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolCandidate {
    pub candidate_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub repo_relative_path: RepoRelativePath,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f32,
    pub snippet: String,
    pub symbol_kind: SymbolKindCode,
    pub symbol_kind_family: Option<SymbolKindFamily>,
}

const SYMBOL_CANDIDATE_FIELDS: &[&str] = &[
    "candidate_id",
    "repo_id",
    "revision_id",
    "manifest_generation",
    "repo_relative_path",
    "start_line",
    "end_line",
    "score",
    "snippet",
    "symbol_kind",
    "symbol_kind_family",
];

macro_rules! impl_symbol_candidate_serde {
    ($fields:ident, $visitor:ident) => {
        impl Serialize for SymbolCandidate {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct("SymbolCandidate", 11)?;
                state.serialize_field("candidate_id", &self.candidate_id)?;
                state.serialize_field("repo_id", &self.repo_id)?;
                state.serialize_field("revision_id", &self.revision_id)?;
                state.serialize_field("manifest_generation", &self.manifest_generation)?;
                state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
                state.serialize_field("start_line", &self.start_line)?;
                state.serialize_field("end_line", &self.end_line)?;
                state.serialize_field("score", &self.score)?;
                state.serialize_field("snippet", &self.snippet)?;
                state.serialize_field("symbol_kind", &self.symbol_kind)?;
                state.serialize_field("symbol_kind_family", &self.symbol_kind_family)?;
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = SymbolCandidate;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a SymbolCandidate map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut candidate_id: Option<String> = None;
                let mut repo_id: Option<RepoId> = None;
                let mut revision_id: Option<RevisionId> = None;
                let mut manifest_generation: Option<ManifestGeneration> = None;
                let mut repo_relative_path: Option<RepoRelativePath> = None;
                let mut start_line: Option<u32> = None;
                let mut end_line: Option<u32> = None;
                let mut score: Option<f32> = None;
                let mut snippet: Option<String> = None;
                let mut symbol_kind: Option<SymbolKindCode> = None;
                let mut symbol_kind_family: Option<Option<SymbolKindFamily>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "candidate_id" => {
                            if candidate_id.is_some() {
                                return Err(de::Error::duplicate_field("candidate_id"));
                            }
                            candidate_id = Some(map.next_value()?);
                        }
                        "repo_id" => {
                            if repo_id.is_some() {
                                return Err(de::Error::duplicate_field("repo_id"));
                            }
                            repo_id = Some(map.next_value()?);
                        }
                        "revision_id" => {
                            if revision_id.is_some() {
                                return Err(de::Error::duplicate_field("revision_id"));
                            }
                            revision_id = Some(map.next_value()?);
                        }
                        "manifest_generation" => {
                            if manifest_generation.is_some() {
                                return Err(de::Error::duplicate_field("manifest_generation"));
                            }
                            manifest_generation = Some(map.next_value()?);
                        }
                        "repo_relative_path" => {
                            if repo_relative_path.is_some() {
                                return Err(de::Error::duplicate_field("repo_relative_path"));
                            }
                            repo_relative_path = Some(map.next_value()?);
                        }
                        "start_line" => {
                            if start_line.is_some() {
                                return Err(de::Error::duplicate_field("start_line"));
                            }
                            start_line = Some(map.next_value()?);
                        }
                        "end_line" => {
                            if end_line.is_some() {
                                return Err(de::Error::duplicate_field("end_line"));
                            }
                            end_line = Some(map.next_value()?);
                        }
                        "score" => {
                            if score.is_some() {
                                return Err(de::Error::duplicate_field("score"));
                            }
                            score = Some(map.next_value()?);
                        }
                        "snippet" => {
                            if snippet.is_some() {
                                return Err(de::Error::duplicate_field("snippet"));
                            }
                            snippet = Some(map.next_value()?);
                        }
                        "symbol_kind" => {
                            if symbol_kind.is_some() {
                                return Err(de::Error::duplicate_field("symbol_kind"));
                            }
                            symbol_kind = Some(map.next_value()?);
                        }
                        "symbol_kind_family" => {
                            if symbol_kind_family.is_some() {
                                return Err(de::Error::duplicate_field("symbol_kind_family"));
                            }
                            symbol_kind_family = Some(map.next_value()?);
                        }
                        other => return Err(de::Error::unknown_field(other, $fields)),
                    }
                }
                Ok(SymbolCandidate {
                    candidate_id: candidate_id
                        .ok_or_else(|| de::Error::missing_field("candidate_id"))?,
                    repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
                    revision_id: revision_id
                        .ok_or_else(|| de::Error::missing_field("revision_id"))?,
                    manifest_generation: manifest_generation
                        .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
                    repo_relative_path: repo_relative_path
                        .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
                    start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
                    end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
                    score: score.ok_or_else(|| de::Error::missing_field("score"))?,
                    snippet: snippet.ok_or_else(|| de::Error::missing_field("snippet"))?,
                    symbol_kind: symbol_kind
                        .ok_or_else(|| de::Error::missing_field("symbol_kind"))?,
                    symbol_kind_family: symbol_kind_family
                        .ok_or_else(|| de::Error::missing_field("symbol_kind_family"))?,
                })
            }
        }

        impl<'de> Deserialize<'de> for SymbolCandidate {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct("SymbolCandidate", $fields, $visitor)
            }
        }
    };
}

impl_symbol_candidate_serde!(SYMBOL_CANDIDATE_FIELDS, SymbolCandidateVisitor);

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<SymbolCandidate>,
    pub window: QueryResultWindowV1,
}

const SYMBOL_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "window"];

const TEXT_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "window", "file_owner_rows"];
const FILE_OWNER_PROJECTION_ROW_FIELDS: &[&str] = &[
    "candidate_id",
    "repo_id",
    "revision_id",
    "manifest_generation",
    "repo_relative_path",
    "owners",
];

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub window: QueryResultWindowV1,
    pub explanation: SearchExplanation,
}

const SEMANTIC_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "window", "explanation"];

/// The hybrid response: the RRF fusion of the two independent lanes, one
/// [`HybridCandidateV1`] per fused identity (QI-BB-018, QI-BB-022).
///
/// `results` is in ranking order — `fused_score` descending, then a row the
/// lexical lane saw before one it did not, then `candidate_id` ascending —
/// and no identity appears twice. Both are checked on encode and decode.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<HybridCandidateV1>,
    pub window: QueryResultWindowV1,
    pub explanation: SearchExplanation,
}

const HYBRID_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "window", "explanation"];

/// One of the two lanes the hybrid route fuses (QI-BB-018).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HybridLaneV1 {
    /// The lowered text query over the lexical index.
    Lexical,
    /// The embedded semantic query over the generation's vectors.
    Dense,
}

impl HybridLaneV1 {
    /// The lane's lower-case code, as traces and renderers name it.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Dense => "dense",
        }
    }
}

const HYBRID_LANE_V1_VARIANTS: &[&str] = &["Lexical", "Dense"];

/// Where one lane placed a hybrid candidate and what it scored it there.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridLaneContributionV1 {
    pub lane: HybridLaneV1,
    /// 1-based rank in the lane's own ranked list.
    pub rank: u32,
    /// The score the lane's engine emitted for the candidate: BM25 for the
    /// lexical lane, cosine similarity for the dense lane. Finite.
    pub raw_score: f32,
}

const HYBRID_LANE_CONTRIBUTION_V1_FIELDS: &[&str] = &["lane", "rank", "raw_score"];

/// One hybrid result row: a lane's row for the identity, the RRF score
/// that ranked it, and the per-lane provenance the score was fused from
/// (QI-BB-022).
///
/// `fused_score` is the ranking key: the sum over `contributions` of
/// `1 / (k + rank)` under the plane's RRF constant, exactly as the plane
/// sorted it — which is why it is `f64` on the wire (narrowing would create
/// ties the sort never had). `candidate` is the row the *preferred* lane
/// emitted — the lexical lane's row when it saw the identity, else the
/// dense lane's — so `candidate.score` is that lane's raw score, not the
/// fused one; it equals the first contribution's `raw_score`.
///
/// Invariants, checked on encode and decode: `fused_score` finite and
/// positive; one or two contributions in lane order (lexical before dense),
/// each lane at most once, each rank at least 1, each `raw_score` finite;
/// `candidate.score` equal to the first contribution's `raw_score`.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridCandidateV1 {
    pub candidate: LexicalCandidate,
    pub fused_score: f64,
    pub contributions: Vec<HybridLaneContributionV1>,
}

const HYBRID_CANDIDATE_V1_FIELDS: &[&str] = &["candidate", "fused_score", "contributions"];

/// Why a [`HybridCandidateV1`], or a list of them, is not a hybrid result.
#[derive(Clone, Debug, PartialEq)]
pub enum HybridCandidatePolicyErrorV1 {
    FusedScoreNotPositiveFinite { fused_score: f64 },
    ContributionCountOutOfRange { count: usize },
    ContributionsNotInLaneOrder,
    RankIsZero { lane: HybridLaneV1 },
    RawScoreNotFinite { lane: HybridLaneV1, raw_score: f32 },
    CandidateScoreIsNotPreferredLaneScore { score: f32, raw_score: f32 },
    ResultsNotInRankingOrder { position: usize },
    DuplicateCandidateId { candidate_id: String },
}

impl fmt::Display for HybridCandidatePolicyErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FusedScoreNotPositiveFinite { fused_score } => write!(
                formatter,
                "hybrid candidate fused_score must be finite and positive; got {fused_score}"
            ),
            Self::ContributionCountOutOfRange { count } => write!(
                formatter,
                "hybrid candidate must carry one or two lane contributions; got {count}"
            ),
            Self::ContributionsNotInLaneOrder => formatter.write_str(
                "hybrid candidate contributions must be in lane order (lexical, then dense) with each lane at most once",
            ),
            Self::RankIsZero { lane } => write!(
                formatter,
                "hybrid candidate {} contribution rank must be at least 1",
                lane.as_code_str()
            ),
            Self::RawScoreNotFinite { lane, raw_score } => write!(
                formatter,
                "hybrid candidate {} contribution raw_score must be finite; got {raw_score}",
                lane.as_code_str()
            ),
            Self::CandidateScoreIsNotPreferredLaneScore { score, raw_score } => write!(
                formatter,
                "hybrid candidate score {score} is not its preferred lane's raw_score {raw_score}"
            ),
            Self::ResultsNotInRankingOrder { position } => write!(
                formatter,
                "hybrid results are not in ranking order (fused_score desc, lexical-seen first, candidate_id asc) at position {position}"
            ),
            Self::DuplicateCandidateId { candidate_id } => write!(
                formatter,
                "hybrid results carry candidate {candidate_id} more than once"
            ),
        }
    }
}

impl std::error::Error for HybridCandidatePolicyErrorV1 {}

impl HybridCandidateV1 {
    /// Check the row invariants documented on the type.
    pub fn validate_v1(&self) -> Result<(), HybridCandidatePolicyErrorV1> {
        if !(self.fused_score.is_finite() && self.fused_score > 0.0) {
            return Err(HybridCandidatePolicyErrorV1::FusedScoreNotPositiveFinite {
                fused_score: self.fused_score,
            });
        }
        if !(1..=2).contains(&self.contributions.len()) {
            return Err(HybridCandidatePolicyErrorV1::ContributionCountOutOfRange {
                count: self.contributions.len(),
            });
        }
        for contribution in &self.contributions {
            if contribution.rank == 0 {
                return Err(HybridCandidatePolicyErrorV1::RankIsZero {
                    lane: contribution.lane,
                });
            }
            if !contribution.raw_score.is_finite() {
                return Err(HybridCandidatePolicyErrorV1::RawScoreNotFinite {
                    lane: contribution.lane,
                    raw_score: contribution.raw_score,
                });
            }
        }
        for pair in self.contributions.windows(2) {
            let in_lane_order = matches!(
                pair,
                [
                    HybridLaneContributionV1 {
                        lane: HybridLaneV1::Lexical,
                        ..
                    },
                    HybridLaneContributionV1 {
                        lane: HybridLaneV1::Dense,
                        ..
                    }
                ]
            );
            if !in_lane_order {
                return Err(HybridCandidatePolicyErrorV1::ContributionsNotInLaneOrder);
            }
        }
        let Some(preferred) = self.contributions.first() else {
            return Err(HybridCandidatePolicyErrorV1::ContributionCountOutOfRange { count: 0 });
        };
        // The row is a copy of the preferred lane's row, so its score is
        // bit-identical to that lane's raw score; both are finite here.
        if self.candidate.score.to_bits() != preferred.raw_score.to_bits() {
            return Err(
                HybridCandidatePolicyErrorV1::CandidateScoreIsNotPreferredLaneScore {
                    score: self.candidate.score,
                    raw_score: preferred.raw_score,
                },
            );
        }
        Ok(())
    }

    /// Whether the lexical lane saw this identity — the second ranking key
    /// after `fused_score`.
    #[must_use]
    pub fn seen_by_lexical_lane(&self) -> bool {
        self.contributions
            .iter()
            .any(|contribution| contribution.lane == HybridLaneV1::Lexical)
    }

    /// The contribution of `lane`, when that lane saw the identity.
    #[must_use]
    pub fn contribution(&self, lane: HybridLaneV1) -> Option<&HybridLaneContributionV1> {
        self.contributions
            .iter()
            .find(|contribution| contribution.lane == lane)
    }
}

/// Check that `results` is a hybrid ranking: every row valid, ordered by
/// `fused_score` descending, then lexical-seen rows first, then
/// `candidate_id` ascending, with no identity repeated.
pub fn validate_hybrid_results_v1(
    results: &[HybridCandidateV1],
) -> Result<(), HybridCandidatePolicyErrorV1> {
    let mut seen = std::collections::BTreeSet::<&str>::new();
    for row in results {
        row.validate_v1()?;
        if !seen.insert(row.candidate.candidate_id.as_str()) {
            return Err(HybridCandidatePolicyErrorV1::DuplicateCandidateId {
                candidate_id: row.candidate.candidate_id.clone(),
            });
        }
    }
    for (position, pair) in results.windows(2).enumerate() {
        let [left, right] = pair else {
            return Err(HybridCandidatePolicyErrorV1::ResultsNotInRankingOrder { position });
        };
        let ordering = right
            .fused_score
            .total_cmp(&left.fused_score)
            .then(
                right
                    .seen_by_lexical_lane()
                    .cmp(&left.seen_by_lexical_lane()),
            )
            .then_with(|| {
                left.candidate
                    .candidate_id
                    .as_str()
                    .cmp(right.candidate.candidate_id.as_str())
            });
        if ordering != core::cmp::Ordering::Less {
            return Err(HybridCandidatePolicyErrorV1::ResultsNotInRankingOrder {
                position: position.saturating_add(1),
            });
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedLane {
    Exact,
    Bm25,
    Dense,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeedContribution {
    pub lane: SeedLane,
    pub rank: u32,
    pub raw_score: Option<f32>,
    pub corpus_kind: Option<SemanticCorpusKindV1>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeedCandidate {
    pub record_id: String,
    pub entity_id: String,
    pub owner_kind: OwnerDocKind,
    pub corpus_kind: Option<SemanticCorpusKindV1>,
    /// Authority digest of the semantic record that produced this seed.
    /// This is distinct from the generation manifest digest.
    pub authority_digest: Option<String>,
    pub repo_relative_path: RepoRelativePath,
    pub snippet: String,
    pub seed_rank: u32,
    pub contributions: Vec<SeedContribution>,
    pub degraded_reasons: Vec<String>,
}

/// Canonical identity for ranking and deduplicating [`SeedCandidate`] values.
///
/// `entity_id` is opaque outside its owner domain. Including `owner_kind` in
/// the ordered key prevents unrelated records with equal display text from
/// collapsing during RRF fusion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedFusionIdentity {
    owner_kind: OwnerDocKind,
    entity_id: String,
    corpus_kind: Option<SemanticCorpusKindV1>,
}

impl SeedFusionIdentity {
    #[must_use]
    pub const fn new(owner_kind: OwnerDocKind, entity_id: String) -> Self {
        Self {
            owner_kind,
            entity_id,
            corpus_kind: None,
        }
    }

    #[must_use]
    pub const fn new_with_corpus(
        owner_kind: OwnerDocKind,
        entity_id: String,
        corpus_kind: Option<SemanticCorpusKindV1>,
    ) -> Self {
        Self {
            owner_kind,
            entity_id,
            corpus_kind,
        }
    }

    #[must_use]
    pub const fn owner_kind(&self) -> OwnerDocKind {
        self.owner_kind
    }

    #[must_use]
    pub fn entity_id(&self) -> &str {
        &self.entity_id
    }

    #[must_use]
    pub const fn corpus_kind(&self) -> Option<SemanticCorpusKindV1> {
        self.corpus_kind
    }

    /// Compare borrowed identity parts without allocating temporary keys.
    ///
    /// The order is exactly the derived [`Ord`] order of this type:
    /// `owner_kind` first, followed by the opaque `entity_id` and corpus kind.
    #[must_use]
    pub fn cmp_parts(
        left_owner_kind: OwnerDocKind,
        left_entity_id: &str,
        right_owner_kind: OwnerDocKind,
        right_entity_id: &str,
    ) -> core::cmp::Ordering {
        Self::cmp_parts_with_corpus_v2(
            left_owner_kind,
            left_entity_id,
            None,
            right_owner_kind,
            right_entity_id,
            None,
        )
    }

    #[must_use]
    pub fn cmp_parts_with_corpus_v2(
        left_owner_kind: OwnerDocKind,
        left_entity_id: &str,
        left_corpus_kind: Option<SemanticCorpusKindV1>,
        right_owner_kind: OwnerDocKind,
        right_entity_id: &str,
        right_corpus_kind: Option<SemanticCorpusKindV1>,
    ) -> core::cmp::Ordering {
        left_owner_kind
            .cmp(&right_owner_kind)
            .then_with(|| left_entity_id.cmp(right_entity_id))
            .then_with(|| {
                corpus_kind_code(left_corpus_kind).cmp(corpus_kind_code(right_corpus_kind))
            })
    }
}

impl Ord for SeedFusionIdentity {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        Self::cmp_parts_with_corpus_v2(
            self.owner_kind,
            self.entity_id.as_str(),
            self.corpus_kind,
            other.owner_kind,
            other.entity_id.as_str(),
            other.corpus_kind,
        )
    }
}

impl PartialOrd for SeedFusionIdentity {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn corpus_kind_code(corpus_kind: Option<SemanticCorpusKindV1>) -> &'static str {
    corpus_kind.map_or("", SemanticCorpusKindV1::as_code_str)
}

impl From<&SeedCandidate> for SeedFusionIdentity {
    fn from(candidate: &SeedCandidate) -> Self {
        Self::new_with_corpus(
            candidate.owner_kind,
            candidate.entity_id.clone(),
            candidate.corpus_kind,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
/// The hybrid seed response: one canonical seed list, fused from the
/// lexical lane and one dense lane per requested corpus (QI-BB-019).
///
/// `window.returned` counts `seed_candidates`; there is no second list.
pub struct HybridSeedQueryResponse {
    pub generation: GenerationPin,
    pub manifest_digest: String,
    pub seed_candidates: Vec<SeedCandidate>,
    pub window: QueryResultWindowV1,
    pub explanation: SearchExplanation,
}

const HYBRID_SEED_QUERY_RESPONSE_FIELDS: &[&str] = &[
    "generation",
    "manifest_digest",
    "seed_candidates",
    "window",
    "explanation",
];
const SEED_CONTRIBUTION_V2_FIELDS: &[&str] = &["lane", "rank", "raw_score", "corpus_kind"];
const SEED_CANDIDATE_V2_FIELDS: &[&str] = &[
    "record_id",
    "entity_id",
    "owner_kind",
    "corpus_kind",
    "authority_digest",
    "repo_relative_path",
    "snippet",
    "seed_rank",
    "contributions",
    "degraded_reasons",
];

#[derive(Clone, Debug, PartialEq)]
/// One page of history results (QI-BB-023).
///
/// Exactly one of `commits` / `diffs` is populated, by the query's
/// `type:`. Both are in recency order — the total order documented on
/// [`HistoryCursor`] — and `window` counts that page: `returned` is the
/// rows on it, `candidate_count` the exact number of matches after the
/// request's cursor, `has_more` whether a next page exists, in which case
/// `next_cursor` positions it. `examined` is how many records the plane
/// evaluated to answer. `read_epoch` is the history authority epoch the
/// page was cut from (QI-BB-020 W2): the current one for a fresh walk,
/// the cursor's for a continuation; `next_cursor` carries it forward.
pub struct SearchPlaneHistoryQueryResponse {
    pub generation: GenerationPin,
    pub commits: Vec<CommitCandidate>,
    pub diffs: Vec<DiffCandidate>,
    pub window: QueryResultWindowV1,
    pub read_epoch: AuxEpochV1,
    pub examined: u64,
    pub next_cursor: Option<HistoryCursor>,
}

const SEARCH_PLANE_HISTORY_QUERY_RESPONSE_FIELDS: &[&str] = &[
    "generation",
    "commits",
    "diffs",
    "window",
    "read_epoch",
    "examined",
    "next_cursor",
];

impl Serialize for SearchPlaneHistoryQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.next_cursor.is_some() { 7 } else { 6 };
        let mut state =
            serializer.serialize_struct("SearchPlaneHistoryQueryResponse", field_count)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("commits", &self.commits)?;
        state.serialize_field("diffs", &self.diffs)?;
        state.serialize_field("window", &self.window)?;
        state.serialize_field("read_epoch", &self.read_epoch)?;
        state.serialize_field("examined", &self.examined)?;
        if let Some(next_cursor) = &self.next_cursor {
            state.serialize_field("next_cursor", next_cursor)?;
        }
        state.end()
    }
}

struct SearchPlaneHistoryQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneHistoryQueryResponseVisitor {
    type Value = SearchPlaneHistoryQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneHistoryQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut commits: Option<Vec<CommitCandidate>> = None;
        let mut diffs: Option<Vec<DiffCandidate>> = None;
        let mut window: Option<QueryResultWindowV1> = None;
        let mut read_epoch: Option<AuxEpochV1> = None;
        let mut examined: Option<u64> = None;
        let mut next_cursor: Option<HistoryCursor> = None;
        let mut next_cursor_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "commits" => {
                    if commits.is_some() {
                        return Err(de::Error::duplicate_field("commits"));
                    }
                    commits = Some(map.next_value()?);
                }
                "diffs" => {
                    if diffs.is_some() {
                        return Err(de::Error::duplicate_field("diffs"));
                    }
                    diffs = Some(map.next_value()?);
                }
                "window" => {
                    if window.is_some() {
                        return Err(de::Error::duplicate_field("window"));
                    }
                    window = Some(map.next_value()?);
                }
                "read_epoch" => {
                    if read_epoch.is_some() {
                        return Err(de::Error::duplicate_field("read_epoch"));
                    }
                    read_epoch = Some(map.next_value()?);
                }
                "examined" => {
                    if examined.is_some() {
                        return Err(de::Error::duplicate_field("examined"));
                    }
                    examined = Some(map.next_value()?);
                }
                "next_cursor" => {
                    if next_cursor_seen {
                        return Err(de::Error::duplicate_field("next_cursor"));
                    }
                    next_cursor_seen = true;
                    next_cursor = map.next_value()?;
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_HISTORY_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let commits = commits.ok_or_else(|| de::Error::missing_field("commits"))?;
        let diffs = diffs.ok_or_else(|| de::Error::missing_field("diffs"))?;
        let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
        // A page carries one kind of row; the window counts exactly those.
        if !commits.is_empty() && !diffs.is_empty() {
            return Err(de::Error::custom(
                "history page carries both commits and diffs",
            ));
        }
        let returned = usize::try_from(window.returned()).map_err(|error| {
            de::Error::custom(format!(
                "query result window returned count cannot fit usize: {error}"
            ))
        })?;
        if returned != commits.len().saturating_add(diffs.len()) {
            return Err(de::Error::custom(
                "query result window returned count does not match history rows",
            ));
        }
        if window.has_more() != next_cursor.is_some() {
            return Err(de::Error::custom(
                "history page has_more and next_cursor disagree",
            ));
        }
        let read_epoch = read_epoch.ok_or_else(|| de::Error::missing_field("read_epoch"))?;
        // The continuation is cut from the epoch this page read; a cursor
        // naming another epoch could not have been issued by this page.
        if next_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.aux_epoch != read_epoch)
        {
            return Err(de::Error::custom(
                "history page next_cursor names an epoch other than read_epoch",
            ));
        }
        Ok(SearchPlaneHistoryQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            commits,
            diffs,
            window,
            read_epoch,
            examined: examined.ok_or_else(|| de::Error::missing_field("examined"))?,
            next_cursor,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneHistoryQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneHistoryQueryResponse",
            SEARCH_PLANE_HISTORY_QUERY_RESPONSE_FIELDS,
            SearchPlaneHistoryQueryResponseVisitor,
        )
    }
}

/// Manual serde for a `{ generation, results, window }` response.
macro_rules! impl_generation_results_response_serde {
    ($ty:ident, $fields:ident, $visitor:ident, $result_ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct(stringify!($ty), $fields.len())?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field("results", &self.results)?;
                state.serialize_field("window", &self.window)?;
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!("a ", stringify!($ty), " map"))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut generation: Option<GenerationPin> = None;
                let mut results: Option<Vec<$result_ty>> = None;
                let mut window: Option<QueryResultWindowV1> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "results" => {
                            if results.is_some() {
                                return Err(de::Error::duplicate_field("results"));
                            }
                            results = Some(map.next_value()?);
                        }
                        "window" => {
                            if window.is_some() {
                                return Err(de::Error::duplicate_field("window"));
                            }
                            window = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
                let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
                let returned = usize::try_from(window.returned()).map_err(|error| {
                    de::Error::custom(format!(
                        "query result window returned count cannot fit usize: {error}"
                    ))
                })?;
                if returned != results.len() {
                    return Err(de::Error::custom(
                        "query result window returned count does not match results length",
                    ));
                }
                Ok($ty {
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    results,
                    window,
                })
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
}

/// Manual serde for one keyset page (QI-BB-025 W4): `{ generation,
/// results, window, <epochs...>, examined, next_cursor? }`.
///
/// The rows are in the route's total order — `candidate_id` ascending —
/// and the decoder holds the page to it fail-closed: `window.returned`
/// counts the rows, the rows strictly ascend by candidate id, `has_more`
/// and `next_cursor` agree, `next_cursor` names the last row, and every
/// epoch the cursor carries equals the epoch the page reports for it
/// (each `(response_epoch, cursor_epoch)` pair in `epochs`).
macro_rules! impl_keyset_page_response_serde {
    (
        $ty:ident,
        $fields:ident,
        $visitor:ident,
        $result_ty:ty,
        $cursor_ty:ty,
        epochs = [$(($response_epoch:ident, $cursor_epoch:ident)),+ $(,)?]
    ) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let field_count = if self.next_cursor.is_some() {
                    $fields.len()
                } else {
                    $fields.len().saturating_sub(1)
                };
                let mut state = serializer.serialize_struct(stringify!($ty), field_count)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field("results", &self.results)?;
                state.serialize_field("window", &self.window)?;
                $(state.serialize_field(stringify!($response_epoch), &self.$response_epoch)?;)+
                state.serialize_field("examined", &self.examined)?;
                if let Some(next_cursor) = &self.next_cursor {
                    state.serialize_field("next_cursor", next_cursor)?;
                }
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!("a ", stringify!($ty), " map"))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut generation: Option<GenerationPin> = None;
                let mut results: Option<Vec<$result_ty>> = None;
                let mut window: Option<QueryResultWindowV1> = None;
                $(let mut $response_epoch: Option<AuxEpochV1> = None;)+
                let mut examined: Option<u64> = None;
                let mut next_cursor: Option<$cursor_ty> = None;
                let mut next_cursor_seen = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "results" => {
                            if results.is_some() {
                                return Err(de::Error::duplicate_field("results"));
                            }
                            results = Some(map.next_value()?);
                        }
                        "window" => {
                            if window.is_some() {
                                return Err(de::Error::duplicate_field("window"));
                            }
                            window = Some(map.next_value()?);
                        }
                        $(
                            stringify!($response_epoch) => {
                                if $response_epoch.is_some() {
                                    return Err(de::Error::duplicate_field(
                                        stringify!($response_epoch),
                                    ));
                                }
                                $response_epoch = Some(map.next_value()?);
                            }
                        )+
                        "examined" => {
                            if examined.is_some() {
                                return Err(de::Error::duplicate_field("examined"));
                            }
                            examined = Some(map.next_value()?);
                        }
                        "next_cursor" => {
                            if next_cursor_seen {
                                return Err(de::Error::duplicate_field("next_cursor"));
                            }
                            next_cursor_seen = true;
                            next_cursor = map.next_value()?;
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
                let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
                let returned = usize::try_from(window.returned()).map_err(|error| {
                    de::Error::custom(format!(
                        "query result window returned count cannot fit usize: {error}"
                    ))
                })?;
                if returned != results.len() {
                    return Err(de::Error::custom(
                        "query result window returned count does not match results length",
                    ));
                }
                // The page is in the total order the cursor positions.
                if results
                    .iter()
                    .zip(results.iter().skip(1))
                    .any(|(row, next)| row.candidate_id.as_str() >= next.candidate_id.as_str())
                {
                    return Err(de::Error::custom(
                        "keyset page rows are not strictly ascending by candidate id",
                    ));
                }
                if window.has_more() != next_cursor.is_some() {
                    return Err(de::Error::custom(
                        "keyset page has_more and next_cursor disagree",
                    ));
                }
                if let Some(cursor) = &next_cursor {
                    // The cursor is the last row's key, in the epochs this
                    // page read; anything else could not have been issued
                    // by this page.
                    if results.last().map(|row| row.candidate_id.as_str())
                        != Some(cursor.candidate_id.as_str())
                    {
                        return Err(de::Error::custom(
                            "keyset page next_cursor does not name the last row",
                        ));
                    }
                    $(
                        if Some(cursor.$cursor_epoch) != $response_epoch {
                            return Err(de::Error::custom(concat!(
                                "keyset page next_cursor names an epoch other than ",
                                stringify!($response_epoch),
                            )));
                        }
                    )+
                }
                Ok($ty {
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    results,
                    window,
                    $(
                        $response_epoch: $response_epoch.ok_or_else(|| {
                            de::Error::missing_field(stringify!($response_epoch))
                        })?,
                    )+
                    examined: examined.ok_or_else(|| de::Error::missing_field("examined"))?,
                    next_cursor,
                })
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
}

/// Serde for a `{ generation, results, window, explanation }` response.
///
/// `validate_results` is the cross-row invariant the list must satisfy,
/// checked on encode and decode; the four-argument form has none, so any
/// list of well-formed rows is accepted.
macro_rules! impl_generation_results_explanation_response_serde {
    ($ty:ident, $fields:ident, $visitor:ident, $result_ty:ty) => {
        impl_generation_results_explanation_response_serde!(
            $ty,
            $fields,
            $visitor,
            $result_ty,
            validate_results = |_results: &[$result_ty]| Ok::<(), core::convert::Infallible>(())
        );
    };
    (
        $ty:ident,
        $fields:ident,
        $visitor:ident,
        $result_ty:ty,
        validate_results = $validate_results:expr
    ) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                ($validate_results)(&self.results).map_err(serde::ser::Error::custom)?;
                let mut state = serializer.serialize_struct(stringify!($ty), 4)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field("results", &self.results)?;
                state.serialize_field("window", &self.window)?;
                state.serialize_field("explanation", &self.explanation)?;
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!("a ", stringify!($ty), " map"))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut generation: Option<GenerationPin> = None;
                let mut results: Option<Vec<$result_ty>> = None;
                let mut window: Option<QueryResultWindowV1> = None;
                let mut explanation: Option<SearchExplanation> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        "results" => {
                            if results.is_some() {
                                return Err(de::Error::duplicate_field("results"));
                            }
                            results = Some(map.next_value()?);
                        }
                        "window" => {
                            if window.is_some() {
                                return Err(de::Error::duplicate_field("window"));
                            }
                            window = Some(map.next_value()?);
                        }
                        "explanation" => {
                            if explanation.is_some() {
                                return Err(de::Error::duplicate_field("explanation"));
                            }
                            explanation = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
                let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
                let returned = usize::try_from(window.returned()).map_err(|error| {
                    de::Error::custom(format!(
                        "query result window returned count cannot fit usize: {error}"
                    ))
                })?;
                if returned != results.len() {
                    return Err(de::Error::custom(
                        "query result window returned count does not match results length",
                    ));
                }
                ($validate_results)(&results).map_err(de::Error::custom)?;
                Ok($ty {
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    results,
                    window,
                    explanation: explanation
                        .ok_or_else(|| de::Error::missing_field("explanation"))?,
                })
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
}

impl Serialize for FileOwnerProjectionRow {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileOwnerProjectionRow", 6)?;
        state.serialize_field("candidate_id", &self.candidate_id)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("owners", &self.owners)?;
        state.end()
    }
}

struct FileOwnerProjectionRowVisitor;

impl<'de> Visitor<'de> for FileOwnerProjectionRowVisitor {
    type Value = FileOwnerProjectionRow;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileOwnerProjectionRow map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut candidate_id: Option<String> = None;
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut owners: Option<Vec<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "candidate_id" => {
                    if candidate_id.is_some() {
                        return Err(de::Error::duplicate_field("candidate_id"));
                    }
                    candidate_id = Some(map.next_value()?);
                }
                "repo_id" => {
                    if repo_id.is_some() {
                        return Err(de::Error::duplicate_field("repo_id"));
                    }
                    repo_id = Some(map.next_value()?);
                }
                "revision_id" => {
                    if revision_id.is_some() {
                        return Err(de::Error::duplicate_field("revision_id"));
                    }
                    revision_id = Some(map.next_value()?);
                }
                "manifest_generation" => {
                    if manifest_generation.is_some() {
                        return Err(de::Error::duplicate_field("manifest_generation"));
                    }
                    manifest_generation = Some(map.next_value()?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                "owners" => {
                    if owners.is_some() {
                        return Err(de::Error::duplicate_field("owners"));
                    }
                    owners = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_OWNER_PROJECTION_ROW_FIELDS,
                    ));
                }
            }
        }
        Ok(FileOwnerProjectionRow {
            candidate_id: candidate_id.ok_or_else(|| de::Error::missing_field("candidate_id"))?,
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            owners: owners.ok_or_else(|| de::Error::missing_field("owners"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileOwnerProjectionRow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileOwnerProjectionRow",
            FILE_OWNER_PROJECTION_ROW_FIELDS,
            FileOwnerProjectionRowVisitor,
        )
    }
}

impl Serialize for TextQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 3usize;
        if self.file_owner_rows.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("TextQueryResponse", field_count)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("window", &self.window)?;
        if let Some(file_owner_rows) = &self.file_owner_rows {
            state.serialize_field("file_owner_rows", file_owner_rows)?;
        }
        state.end()
    }
}

struct TextQueryResponseVisitor;

impl<'de> Visitor<'de> for TextQueryResponseVisitor {
    type Value = TextQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a TextQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut results: Option<Vec<LexicalCandidate>> = None;
        let mut window: Option<QueryResultWindowV1> = None;
        let mut file_owner_rows: Option<Vec<FileOwnerProjectionRow>> = None;
        let mut file_owner_rows_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "results" => {
                    if results.is_some() {
                        return Err(de::Error::duplicate_field("results"));
                    }
                    results = Some(map.next_value()?);
                }
                "window" => {
                    if window.is_some() {
                        return Err(de::Error::duplicate_field("window"));
                    }
                    window = Some(map.next_value()?);
                }
                "file_owner_rows" => {
                    if file_owner_rows_seen {
                        return Err(de::Error::duplicate_field("file_owner_rows"));
                    }
                    file_owner_rows_seen = true;
                    file_owner_rows = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, TEXT_QUERY_RESPONSE_FIELDS));
                }
            }
        }
        let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
        let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
        let returned = usize::try_from(window.returned()).map_err(|error| {
            de::Error::custom(format!(
                "query result window returned count cannot fit usize: {error}"
            ))
        })?;
        if returned != results.len() {
            return Err(de::Error::custom(
                "query result window returned count does not match results length",
            ));
        }
        Ok(TextQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            results,
            window,
            file_owner_rows,
        })
    }
}

impl<'de> Deserialize<'de> for TextQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "TextQueryResponse",
            TEXT_QUERY_RESPONSE_FIELDS,
            TextQueryResponseVisitor,
        )
    }
}

impl_generation_results_response_serde!(
    SymbolQueryResponse,
    SYMBOL_QUERY_RESPONSE_FIELDS,
    SymbolQueryResponseVisitor,
    SymbolCandidate
);

impl_generation_results_explanation_response_serde!(
    SemanticQueryResponse,
    SEMANTIC_QUERY_RESPONSE_FIELDS,
    SemanticQueryResponseVisitor,
    LexicalCandidate
);

impl_generation_results_explanation_response_serde!(
    HybridQueryResponse,
    HYBRID_QUERY_RESPONSE_FIELDS,
    HybridQueryResponseVisitor,
    HybridCandidateV1,
    validate_results = validate_hybrid_results_v1
);

impl Serialize for HybridLaneV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Lexical => "Lexical",
            Self::Dense => "Dense",
        })
    }
}

impl<'de> Deserialize<'de> for HybridLaneV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HybridLaneVisitor;

        impl Visitor<'_> for HybridLaneVisitor {
            type Value = HybridLaneV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("string enum HybridLaneV1")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "Lexical" => Ok(HybridLaneV1::Lexical),
                    "Dense" => Ok(HybridLaneV1::Dense),
                    other => Err(de::Error::unknown_variant(other, HYBRID_LANE_V1_VARIANTS)),
                }
            }
        }

        deserializer.deserialize_str(HybridLaneVisitor)
    }
}

impl Serialize for HybridLaneContributionV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HybridLaneContributionV1", 3)?;
        state.serialize_field("lane", &self.lane)?;
        state.serialize_field("rank", &self.rank)?;
        state.serialize_field("raw_score", &self.raw_score)?;
        state.end()
    }
}

struct HybridLaneContributionV1Visitor;

impl<'de> Visitor<'de> for HybridLaneContributionV1Visitor {
    type Value = HybridLaneContributionV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HybridLaneContributionV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut lane: Option<HybridLaneV1> = None;
        let mut rank: Option<u32> = None;
        let mut raw_score: Option<f32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "lane" => {
                    if lane.is_some() {
                        return Err(de::Error::duplicate_field("lane"));
                    }
                    lane = Some(map.next_value()?);
                }
                "rank" => {
                    if rank.is_some() {
                        return Err(de::Error::duplicate_field("rank"));
                    }
                    rank = Some(map.next_value()?);
                }
                "raw_score" => {
                    if raw_score.is_some() {
                        return Err(de::Error::duplicate_field("raw_score"));
                    }
                    raw_score = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        HYBRID_LANE_CONTRIBUTION_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(HybridLaneContributionV1 {
            lane: lane.ok_or_else(|| de::Error::missing_field("lane"))?,
            rank: rank.ok_or_else(|| de::Error::missing_field("rank"))?,
            raw_score: raw_score.ok_or_else(|| de::Error::missing_field("raw_score"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HybridLaneContributionV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HybridLaneContributionV1",
            HYBRID_LANE_CONTRIBUTION_V1_FIELDS,
            HybridLaneContributionV1Visitor,
        )
    }
}

impl Serialize for HybridCandidateV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("HybridCandidateV1", 3)?;
        state.serialize_field("candidate", &self.candidate)?;
        state.serialize_field("fused_score", &self.fused_score)?;
        state.serialize_field("contributions", &self.contributions)?;
        state.end()
    }
}

struct HybridCandidateV1Visitor;

impl<'de> Visitor<'de> for HybridCandidateV1Visitor {
    type Value = HybridCandidateV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HybridCandidateV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut candidate: Option<LexicalCandidate> = None;
        let mut fused_score: Option<f64> = None;
        let mut contributions: Option<Vec<HybridLaneContributionV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "candidate" => {
                    if candidate.is_some() {
                        return Err(de::Error::duplicate_field("candidate"));
                    }
                    candidate = Some(map.next_value()?);
                }
                "fused_score" => {
                    if fused_score.is_some() {
                        return Err(de::Error::duplicate_field("fused_score"));
                    }
                    fused_score = Some(map.next_value()?);
                }
                "contributions" => {
                    if contributions.is_some() {
                        return Err(de::Error::duplicate_field("contributions"));
                    }
                    contributions = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, HYBRID_CANDIDATE_V1_FIELDS));
                }
            }
        }
        let row = HybridCandidateV1 {
            candidate: candidate.ok_or_else(|| de::Error::missing_field("candidate"))?,
            fused_score: fused_score.ok_or_else(|| de::Error::missing_field("fused_score"))?,
            contributions: contributions
                .ok_or_else(|| de::Error::missing_field("contributions"))?,
        };
        row.validate_v1().map_err(de::Error::custom)?;
        Ok(row)
    }
}

impl<'de> Deserialize<'de> for HybridCandidateV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HybridCandidateV1",
            HYBRID_CANDIDATE_V1_FIELDS,
            HybridCandidateV1Visitor,
        )
    }
}

impl Serialize for SeedLane {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Exact => "Exact",
            Self::Bm25 => "Bm25",
            Self::Dense => "Dense",
        })
    }
}

impl<'de> Deserialize<'de> for SeedLane {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SeedLaneVisitor;

        impl Visitor<'_> for SeedLaneVisitor {
            type Value = SeedLane;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("string enum SeedLane")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "Exact" => Ok(SeedLane::Exact),
                    "Bm25" => Ok(SeedLane::Bm25),
                    "Dense" => Ok(SeedLane::Dense),
                    other => Err(de::Error::unknown_variant(
                        other,
                        &["Exact", "Bm25", "Dense"],
                    )),
                }
            }
        }

        deserializer.deserialize_str(SeedLaneVisitor)
    }
}

impl Serialize for SeedContribution {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 2usize;
        if self.raw_score.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.corpus_kind.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("SeedContribution", field_count)?;
        state.serialize_field("lane", &self.lane)?;
        state.serialize_field("rank", &self.rank)?;
        if let Some(raw_score) = &self.raw_score {
            state.serialize_field("raw_score", raw_score)?;
        }
        if let Some(corpus_kind) = &self.corpus_kind {
            state.serialize_field("corpus_kind", corpus_kind)?;
        }
        state.end()
    }
}

struct SeedContributionVisitor;

impl<'de> Visitor<'de> for SeedContributionVisitor {
    type Value = SeedContribution;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SeedContribution map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut lane: Option<SeedLane> = None;
        let mut rank: Option<u32> = None;
        let mut raw_score: Option<f32> = None;
        let mut raw_score_seen = false;
        let mut corpus_kind: Option<Option<SemanticCorpusKindV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "lane" => {
                    if lane.is_some() {
                        return Err(de::Error::duplicate_field("lane"));
                    }
                    lane = Some(map.next_value()?);
                }
                "rank" => {
                    if rank.is_some() {
                        return Err(de::Error::duplicate_field("rank"));
                    }
                    rank = Some(map.next_value()?);
                }
                "raw_score" => {
                    if raw_score_seen {
                        return Err(de::Error::duplicate_field("raw_score"));
                    }
                    raw_score_seen = true;
                    raw_score = Some(map.next_value()?);
                }
                "corpus_kind" => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, SEED_CONTRIBUTION_V2_FIELDS));
                }
            }
        }
        Ok(SeedContribution {
            lane: lane.ok_or_else(|| de::Error::missing_field("lane"))?,
            rank: rank.ok_or_else(|| de::Error::missing_field("rank"))?,
            raw_score,
            corpus_kind: corpus_kind.unwrap_or(None),
        })
    }
}

impl<'de> Deserialize<'de> for SeedContribution {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SeedContribution",
            SEED_CONTRIBUTION_V2_FIELDS,
            SeedContributionVisitor,
        )
    }
}

impl Serialize for SeedCandidate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 9usize;
        if self.authority_digest.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("SeedCandidate", field_count)?;
        state.serialize_field("record_id", &self.record_id)?;
        state.serialize_field("entity_id", &self.entity_id)?;
        state.serialize_field("owner_kind", &self.owner_kind)?;
        state.serialize_field("corpus_kind", &self.corpus_kind)?;
        if let Some(authority_digest) = &self.authority_digest {
            state.serialize_field("authority_digest", authority_digest)?;
        }
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("snippet", &self.snippet)?;
        state.serialize_field("seed_rank", &self.seed_rank)?;
        state.serialize_field("contributions", &self.contributions)?;
        state.serialize_field("degraded_reasons", &self.degraded_reasons)?;
        state.end()
    }
}

struct SeedCandidateVisitor;

impl<'de> Visitor<'de> for SeedCandidateVisitor {
    type Value = SeedCandidate;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SeedCandidate map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut record_id: Option<String> = None;
        let mut entity_id: Option<String> = None;
        let mut owner_kind: Option<OwnerDocKind> = None;
        let mut corpus_kind: Option<Option<SemanticCorpusKindV1>> = None;
        let mut authority_digest: Option<Option<String>> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut snippet: Option<String> = None;
        let mut seed_rank: Option<u32> = None;
        let mut contributions: Option<Vec<SeedContribution>> = None;
        let mut degraded_reasons: Option<Vec<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "record_id" => {
                    if record_id.is_some() {
                        return Err(de::Error::duplicate_field("record_id"));
                    }
                    record_id = Some(map.next_value()?);
                }
                "entity_id" => {
                    if entity_id.is_some() {
                        return Err(de::Error::duplicate_field("entity_id"));
                    }
                    entity_id = Some(map.next_value()?);
                }
                "owner_kind" => {
                    if owner_kind.is_some() {
                        return Err(de::Error::duplicate_field("owner_kind"));
                    }
                    owner_kind = Some(map.next_value()?);
                }
                "corpus_kind" => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
                }
                "authority_digest" => {
                    if authority_digest.is_some() {
                        return Err(de::Error::duplicate_field("authority_digest"));
                    }
                    authority_digest = Some(map.next_value()?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                "snippet" => {
                    if snippet.is_some() {
                        return Err(de::Error::duplicate_field("snippet"));
                    }
                    snippet = Some(map.next_value()?);
                }
                "seed_rank" => {
                    if seed_rank.is_some() {
                        return Err(de::Error::duplicate_field("seed_rank"));
                    }
                    seed_rank = Some(map.next_value()?);
                }
                "contributions" => {
                    if contributions.is_some() {
                        return Err(de::Error::duplicate_field("contributions"));
                    }
                    contributions = Some(map.next_value()?);
                }
                "degraded_reasons" => {
                    if degraded_reasons.is_some() {
                        return Err(de::Error::duplicate_field("degraded_reasons"));
                    }
                    degraded_reasons = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, SEED_CANDIDATE_V2_FIELDS));
                }
            }
        }
        Ok(SeedCandidate {
            record_id: record_id.ok_or_else(|| de::Error::missing_field("record_id"))?,
            entity_id: entity_id.ok_or_else(|| de::Error::missing_field("entity_id"))?,
            owner_kind: owner_kind.ok_or_else(|| de::Error::missing_field("owner_kind"))?,
            corpus_kind: corpus_kind.unwrap_or(None),
            authority_digest: authority_digest.unwrap_or(None),
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            snippet: snippet.ok_or_else(|| de::Error::missing_field("snippet"))?,
            seed_rank: seed_rank.ok_or_else(|| de::Error::missing_field("seed_rank"))?,
            contributions: contributions
                .ok_or_else(|| de::Error::missing_field("contributions"))?,
            degraded_reasons: degraded_reasons
                .ok_or_else(|| de::Error::missing_field("degraded_reasons"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SeedCandidate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SeedCandidate",
            SEED_CANDIDATE_V2_FIELDS,
            SeedCandidateVisitor,
        )
    }
}

impl Serialize for HybridSeedQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HybridSeedQueryResponse", 5)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("seed_candidates", &self.seed_candidates)?;
        state.serialize_field("window", &self.window)?;
        state.serialize_field("explanation", &self.explanation)?;
        state.end()
    }
}

struct HybridSeedQueryResponseVisitor;

impl<'de> Visitor<'de> for HybridSeedQueryResponseVisitor {
    type Value = HybridSeedQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HybridSeedQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut manifest_digest: Option<String> = None;
        let mut seed_candidates: Option<Vec<SeedCandidate>> = None;
        let mut window: Option<QueryResultWindowV1> = None;
        let mut explanation: Option<SearchExplanation> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "seed_candidates" => {
                    if seed_candidates.is_some() {
                        return Err(de::Error::duplicate_field("seed_candidates"));
                    }
                    seed_candidates = Some(map.next_value()?);
                }
                "window" => {
                    if window.is_some() {
                        return Err(de::Error::duplicate_field("window"));
                    }
                    window = Some(map.next_value()?);
                }
                "explanation" => {
                    if explanation.is_some() {
                        return Err(de::Error::duplicate_field("explanation"));
                    }
                    explanation = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        HYBRID_SEED_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let seed_candidates =
            seed_candidates.ok_or_else(|| de::Error::missing_field("seed_candidates"))?;
        let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
        let returned = usize::try_from(window.returned()).map_err(|error| {
            de::Error::custom(format!(
                "hybrid seed window returned count cannot fit usize: {error}"
            ))
        })?;
        if returned != seed_candidates.len() {
            return Err(de::Error::custom(
                "hybrid seed window returned count does not match seed_candidates length",
            ));
        }
        Ok(HybridSeedQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            seed_candidates,
            window,
            explanation: explanation.ok_or_else(|| de::Error::missing_field("explanation"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HybridSeedQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HybridSeedQueryResponse",
            HYBRID_SEED_QUERY_RESPONSE_FIELDS,
            HybridSeedQueryResponseVisitor,
        )
    }
}

/// One page of runtime-metadata results (QI-BB-025 W4).
///
/// `results` are in candidate-id order — the total order documented on
/// [`RuntimeMetadataCursorV1`] — and `window` counts that page:
/// `returned` is the rows on it, `candidate_count` the matches after the
/// request's cursor as far as the walk looked (at least `returned + 1`
/// when the walk stopped at its continuation probe, exact when it
/// exhausted the candidate stream), `has_more` whether a next page
/// exists, in which case `next_cursor` positions it. `examined` is how
/// many candidates the walk visited after its cursor to answer.
///
/// `read_epoch` is the runtime-metadata authority epoch the page's
/// predicates were evaluated against and `universe_epoch` the structural
/// authority epoch its chunk universe was joined from (QI-BB-020 W2):
/// the current ones, taken in one ledger read, for a fresh walk; the
/// cursor's for a continuation. `next_cursor` carries both forward.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneRuntimeMetadataQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub window: QueryResultWindowV1,
    pub read_epoch: AuxEpochV1,
    pub universe_epoch: AuxEpochV1,
    pub examined: u64,
    pub next_cursor: Option<RuntimeMetadataCursorV1>,
}

const SEARCH_PLANE_RUNTIME_METADATA_QUERY_RESPONSE_FIELDS: &[&str] = &[
    "generation",
    "results",
    "window",
    "read_epoch",
    "universe_epoch",
    "examined",
    "next_cursor",
];
impl_keyset_page_response_serde!(
    SearchPlaneRuntimeMetadataQueryResponse,
    SEARCH_PLANE_RUNTIME_METADATA_QUERY_RESPONSE_FIELDS,
    SearchPlaneRuntimeMetadataQueryResponseVisitor,
    LexicalCandidate,
    RuntimeMetadataCursorV1,
    epochs = [(read_epoch, aux_epoch), (universe_epoch, universe_epoch)]
);

/// One page of structural results (QI-BB-025 W4).
///
/// `results` are in candidate-id order — the total order documented on
/// [`StructuralCursorV1`] — and `window` counts that page: `returned` is
/// the rows on it, `candidate_count` the exact number of matched
/// candidates after the request's cursor (evaluation materializes the
/// whole match set, so the count is never a bound), `has_more` whether a
/// next page exists, in which case `next_cursor` positions it.
/// `examined` is how many matched candidates the page selection walked.
///
/// `read_epoch` is the structural authority epoch every leaf of the
/// query — the pinned universe, each parse-tree match, each symbol
/// projection — was evaluated against (QI-BB-020 W2): the current one
/// for a fresh walk, the cursor's for a continuation; `next_cursor`
/// carries it forward.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneStructuralQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<StructuralCandidate>,
    pub window: QueryResultWindowV1,
    pub read_epoch: AuxEpochV1,
    pub examined: u64,
    pub next_cursor: Option<StructuralCursorV1>,
}

const SEARCH_PLANE_STRUCTURAL_QUERY_RESPONSE_FIELDS: &[&str] = &[
    "generation",
    "results",
    "window",
    "read_epoch",
    "examined",
    "next_cursor",
];
impl_keyset_page_response_serde!(
    SearchPlaneStructuralQueryResponse,
    SEARCH_PLANE_STRUCTURAL_QUERY_RESPONSE_FIELDS,
    SearchPlaneStructuralQueryResponseVisitor,
    StructuralCandidate,
    StructuralCursorV1,
    epochs = [(read_epoch, aux_epoch)]
);

/// What the search plane says about one candidate (QI-BB-022).
///
/// `presence` is an exact lookup of the candidate id in the generation's
/// lexical index. `explanation` carries the lexical score trace when the
/// request named the query the candidate came from: one contribution row
/// per signal, whose sum is the score the engine emits for the candidate
/// under that plan, and a weights hash that pins the plan's ranker inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneExplainQueryResponse {
    pub generation: GenerationPin,
    pub presence: CandidatePresenceV1,
    pub explanation: SearchExplanation,
}

const SEARCH_PLANE_EXPLAIN_QUERY_RESPONSE_FIELDS: &[&str] =
    &["generation", "presence", "explanation"];

impl Serialize for SearchPlaneExplainQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneExplainQueryResponse", 3)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("presence", &self.presence)?;
        state.serialize_field("explanation", &self.explanation)?;
        state.end()
    }
}

struct SearchPlaneExplainQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneExplainQueryResponseVisitor {
    type Value = SearchPlaneExplainQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneExplainQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut presence: Option<CandidatePresenceV1> = None;
        let mut explanation: Option<SearchExplanation> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "presence" => {
                    if presence.is_some() {
                        return Err(de::Error::duplicate_field("presence"));
                    }
                    presence = Some(map.next_value()?);
                }
                "explanation" => {
                    if explanation.is_some() {
                        return Err(de::Error::duplicate_field("explanation"));
                    }
                    explanation = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_EXPLAIN_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneExplainQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            presence: presence.ok_or_else(|| de::Error::missing_field("presence"))?,
            explanation: explanation.ok_or_else(|| de::Error::missing_field("explanation"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneExplainQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneExplainQueryResponse",
            SEARCH_PLANE_EXPLAIN_QUERY_RESPONSE_FIELDS,
            SearchPlaneExplainQueryResponseVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GenerationPin, ManifestGeneration, RepoId, RevisionId};

    fn sample_generation_pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("repo-seed"),
            RevisionId::new("rev-seed"),
            ManifestGeneration::new(7),
        )
    }

    fn sample_seed_candidate() -> SeedCandidate {
        SeedCandidate {
            record_id: "semantic-source:symbol-card:symbol:demo".to_string(),
            entity_id: "symbol:demo".to_string(),
            owner_kind: OwnerDocKind::Symbol,
            corpus_kind: Some(SemanticCorpusKindV1::SymbolCard),
            authority_digest: Some("authority:symbol-card:demo".to_string()),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            snippet: "symbol: symbol:demo".to_string(),
            seed_rank: 1,
            contributions: vec![SeedContribution {
                lane: SeedLane::Dense,
                rank: 2,
                raw_score: Some(0.5),
                corpus_kind: Some(SemanticCorpusKindV1::SymbolCard),
            }],
            degraded_reasons: vec!["lexical_only_owner_kind_fallback".to_string()],
        }
    }

    #[test]
    fn seed_fusion_identity_borrowed_comparator_matches_owned_order() {
        let identities = [
            SeedFusionIdentity::new(OwnerDocKind::Symbol, "z".to_string()),
            SeedFusionIdentity::new(OwnerDocKind::Chunk, "a".to_string()),
            SeedFusionIdentity::new(OwnerDocKind::Symbol, "a".to_string()),
            SeedFusionIdentity::new(OwnerDocKind::Module, "z".to_string()),
        ];

        for left in &identities {
            for right in &identities {
                assert_eq!(
                    left.cmp(right),
                    SeedFusionIdentity::cmp_parts_with_corpus_v2(
                        left.owner_kind(),
                        left.entity_id(),
                        left.corpus_kind(),
                        right.owner_kind(),
                        right.entity_id(),
                        right.corpus_kind(),
                    )
                );
                if left.corpus_kind().is_none() && right.corpus_kind().is_none() {
                    assert_eq!(
                        left.cmp(right),
                        SeedFusionIdentity::cmp_parts(
                            left.owner_kind(),
                            left.entity_id(),
                            right.owner_kind(),
                            right.entity_id(),
                        )
                    );
                }
            }
        }
    }

    #[test]
    fn hybrid_seed_query_response_without_required_window_fails_closed() {
        let value = serde_json::json!({
            "generation": {
                "repo_id": "repo-seed",
                "revision_id": "rev-seed",
                "manifest_generation": 7
            },
            "manifest_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "seed_candidates": [serde_json::to_value(sample_seed_candidate())
                .expect("seed candidate must serialize")],
            "explanation": serde_json::to_value(SearchExplanation::default())
                .expect("default explanation must serialize")
        });
        let decoded = serde_json::from_value::<HybridSeedQueryResponse>(value);
        assert!(
            decoded.is_err(),
            "payload without mandatory result-window semantics must fail closed"
        );
    }

    /// The pre-QI-BB-019 wire shape carried a second, legacy seed list; a
    /// payload that still does is refused rather than half-read.
    #[test]
    fn hybrid_seed_query_response_refuses_a_legacy_second_seed_list() {
        let value = serde_json::json!({
            "generation": {
                "repo_id": "repo-seed",
                "revision_id": "rev-seed",
                "manifest_generation": 7
            },
            "manifest_digest": "a".repeat(64),
            "seed_candidates": [serde_json::to_value(sample_seed_candidate())
                .expect("seed candidate must serialize")],
            "seed_candidates_v2": [],
            "window": serde_json::to_value(QueryResultWindowV1::exact(1))
                .expect("window must serialize"),
            "explanation": serde_json::to_value(SearchExplanation::default())
                .expect("default explanation must serialize")
        });
        assert!(serde_json::from_value::<HybridSeedQueryResponse>(value).is_err());
    }

    #[test]
    fn hybrid_seed_query_response_round_trips_with_seed_candidates() {
        let response = HybridSeedQueryResponse {
            generation: sample_generation_pin(),
            manifest_digest: "a".repeat(64),
            seed_candidates: vec![sample_seed_candidate()],
            window: QueryResultWindowV1::exact(1),
            explanation: SearchExplanation::default(),
        };

        let decoded_json: HybridSeedQueryResponse = serde_json::from_value(
            serde_json::to_value(&response).expect("response must serialize to JSON"),
        )
        .expect("response must deserialize from JSON");
        assert_eq!(decoded_json, response);

        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&response, &mut cbor).expect("response must serialize to CBOR");
        let decoded_cbor: HybridSeedQueryResponse = ciborium::de::from_reader(cbor.as_slice())
            .expect("response must deserialize from CBOR");
        assert_eq!(decoded_cbor, response);
    }

    #[test]
    fn seed_candidate_authority_digest_is_optional_but_round_trips_when_present() {
        let candidate = sample_seed_candidate();
        let mut encoded = serde_json::to_value(&candidate).expect("seed candidate JSON");
        assert_eq!(
            encoded
                .get("authority_digest")
                .and_then(serde_json::Value::as_str),
            Some("authority:symbol-card:demo")
        );

        let removed = encoded
            .as_object_mut()
            .expect("seed candidate object")
            .remove("authority_digest");
        assert!(removed.is_some());
        let decoded: SeedCandidate = serde_json::from_value(encoded)
            .expect("legacy candidate without digest remains readable");
        assert!(decoded.authority_digest.is_none());
    }
}
