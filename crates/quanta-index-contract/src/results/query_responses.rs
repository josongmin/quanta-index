use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    AuxEpochV1, CommitCandidate, ContinuationTokenV2, DiffCandidate, GenerationPin, HistoryOrderV1,
    LexicalCandidate, LexicalRowOrderKey, ManifestGeneration, OwnerDocKind, QueryResultWindowV2,
    RepoId, RepoRelativePath, RevisionId, SemanticCorpusKindV1, StructuralCandidate,
    lex::{SymbolKindCode, SymbolKindFamily},
};

use super::{CandidatePresenceV1, SearchExplanation};

/// One ranked page of text rows.
///
/// The rows are in the ranked lexical order ([`LexicalRowOrderKey`]);
/// when the window's outcome authorizes a continuation,
/// `next_cursor` carries the opaque token continuing it, and a request
/// carrying that token continues strictly after the last row. The
/// decoder holds a page to that fail-closed: the token is present
/// exactly when the outcome says more rows exist.
#[derive(Clone, Debug, PartialEq)]
pub struct TextQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub window: QueryResultWindowV2,
    pub file_owner_rows: Option<Vec<FileOwnerProjectionRow>>,
    pub next_cursor: Option<ContinuationTokenV2>,
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

impl SymbolCandidate {
    /// This row's position in the ranked lexical page order.
    #[must_use]
    pub fn order_key(&self) -> LexicalRowOrderKey<'_> {
        LexicalRowOrderKey {
            score: self.score,
            repo_relative_path: self.repo_relative_path.as_str(),
            start_line: self.start_line,
            end_line: self.end_line,
            candidate_id: self.candidate_id.as_str(),
        }
    }
}

/// One ranked page of symbol rows, under the same order and continuation
/// rule as [`TextQueryResponse`].
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<SymbolCandidate>,
    pub window: QueryResultWindowV2,
    pub next_cursor: Option<ContinuationTokenV2>,
}

const SYMBOL_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "window", "next_cursor"];

const TEXT_QUERY_RESPONSE_FIELDS: &[&str] = &[
    "generation",
    "results",
    "window",
    "file_owner_rows",
    "next_cursor",
];
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
    /// The single completeness authority (S21-06): the typed execution
    /// outcome and coverage. A bounded top-k page that filled without
    /// an observed continuation is `CappedUnknown`, never exact. The
    /// semantic route is not pageable and never carries a continuation.
    pub window: QueryResultWindowV2,
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
    /// The single completeness authority (S21-06): a capped dense
    /// admission, an interrupted scan or an approximate method stays
    /// typed here and can never be read as exact exhaustion. The hybrid
    /// route is not pageable and never carries a continuation.
    pub window: QueryResultWindowV2,
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

/// Why a [`TextQueryResponse`]'s file-owner projection does not pair one
/// to one with its ranked results (S21-07).
#[derive(Clone, Debug, PartialEq)]
pub enum FileOwnerProjectionErrorV1 {
    RowCountMismatch { rows: usize, results: usize },
    CandidateMismatch { position: usize },
}

impl fmt::Display for FileOwnerProjectionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RowCountMismatch { rows, results } => write!(
                formatter,
                "file owner projection rows do not pair one to one with the results: \
                 {rows} rows for {results} results"
            ),
            Self::CandidateMismatch { position } => write!(
                formatter,
                "file owner projection row {position} does not match the ranked candidate \
                 at the same position"
            ),
        }
    }
}

impl std::error::Error for FileOwnerProjectionErrorV1 {}

/// Check that `file_owner_rows` pairs one to one with `results`.
///
/// Absent is fine, present must carry exactly one row per result, each
/// row naming the same candidate identity in the same order. A swapped
/// or foreign projection row is a malformed page, never a partial one.
pub fn validate_file_owner_projection_v1(
    results: &[LexicalCandidate],
    file_owner_rows: Option<&[FileOwnerProjectionRow]>,
) -> Result<(), FileOwnerProjectionErrorV1> {
    let Some(rows) = file_owner_rows else {
        return Ok(());
    };
    if rows.len() != results.len() {
        return Err(FileOwnerProjectionErrorV1::RowCountMismatch {
            rows: rows.len(),
            results: results.len(),
        });
    }
    for (position, (candidate, row)) in results.iter().zip(rows.iter()).enumerate() {
        if row.candidate_id != candidate.candidate_id
            || row.repo_id != candidate.repo_id
            || row.revision_id != candidate.revision_id
            || row.manifest_generation != candidate.manifest_generation
            || row.repo_relative_path != candidate.repo_relative_path
        {
            return Err(FileOwnerProjectionErrorV1::CandidateMismatch { position });
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
    /// The single completeness authority (S21-06), as
    /// [`HybridQueryResponse::window`].
    pub window: QueryResultWindowV2,
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
/// `type:`. Both are in the request's `order`, which the page echoes;
/// under `relevance` every row carries its score, under `recency` none
/// does. `window` is the single completeness authority: `returned` is
/// the rows on it and the outcome says whether a next page exists, in
/// which case `next_cursor` carries the opaque token continuing it.
/// `examined` is how many records the plane evaluated to answer.
/// `read_epoch` is the history authority epoch the page was cut from
/// (QI-BB-020 W2): the current one for a fresh walk, the token's for a
/// continuation. The token binds the order and the epoch inside; the
/// wire carries neither beside it.
pub struct SearchPlaneHistoryQueryResponse {
    pub generation: GenerationPin,
    pub order: HistoryOrderV1,
    pub commits: Vec<CommitCandidate>,
    pub diffs: Vec<DiffCandidate>,
    pub window: QueryResultWindowV2,
    pub read_epoch: AuxEpochV1,
    pub examined: u64,
    pub next_cursor: Option<ContinuationTokenV2>,
}

const SEARCH_PLANE_HISTORY_QUERY_RESPONSE_FIELDS: &[&str] = &[
    "generation",
    "order",
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
        let field_count = if self.next_cursor.is_some() { 8 } else { 7 };
        let mut state =
            serializer.serialize_struct("SearchPlaneHistoryQueryResponse", field_count)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("order", &self.order)?;
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
        let mut order: Option<HistoryOrderV1> = None;
        let mut commits: Option<Vec<CommitCandidate>> = None;
        let mut diffs: Option<Vec<DiffCandidate>> = None;
        let mut window: Option<QueryResultWindowV2> = None;
        let mut read_epoch: Option<AuxEpochV1> = None;
        let mut examined: Option<u64> = None;
        let mut next_cursor: Option<ContinuationTokenV2> = None;
        let mut next_cursor_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "order" => {
                    if order.is_some() {
                        return Err(de::Error::duplicate_field("order"));
                    }
                    order = Some(map.next_value()?);
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
        let order = order.ok_or_else(|| de::Error::missing_field("order"))?;
        let commits = commits.ok_or_else(|| de::Error::missing_field("commits"))?;
        let diffs = diffs.ok_or_else(|| de::Error::missing_field("diffs"))?;
        let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
        // A page carries one kind of row; the window counts exactly those.
        if !commits.is_empty() && !diffs.is_empty() {
            return Err(de::Error::custom(
                "history page carries both commits and diffs",
            ));
        }
        // A relevance page scores every row; a recency page scores none.
        let scored = commits
            .iter()
            .map(|commit| commit.score.is_some())
            .chain(diffs.iter().map(|diff| diff.score.is_some()));
        for row_scored in scored {
            match order {
                HistoryOrderV1::Relevance if !row_scored => {
                    return Err(de::Error::custom(
                        "relevance history page carries a row without a score",
                    ));
                }
                HistoryOrderV1::Recency if row_scored => {
                    return Err(de::Error::custom(
                        "recency history page carries a scored row",
                    ));
                }
                HistoryOrderV1::Relevance | HistoryOrderV1::Recency => {}
            }
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
        // The token is present exactly when the outcome authorizes a
        // continuation; order and epoch agreement live inside the token.
        if continuation_authorized(&window) != next_cursor.is_some() {
            return Err(de::Error::custom(
                "history page outcome and next_cursor disagree",
            ));
        }
        let read_epoch = read_epoch.ok_or_else(|| de::Error::missing_field("read_epoch"))?;
        Ok(SearchPlaneHistoryQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            order,
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

/// Whether the window's outcome authorizes a continuation token.
///
/// Only an observed continuation does. Every other outcome — exhausted,
/// capped, partial, approximate, or a lower bound without an observed
/// row — travels tokenless.
fn continuation_authorized(window: &QueryResultWindowV2) -> bool {
    window.outcome().has_more() == Some(true)
}

/// Manual serde for a ranked lexical page without projection rows.
///
/// `{ generation, results, window, next_cursor? }`, held to the ranked
/// order and the single continuation authority: the opaque token is
/// present exactly when the window's outcome authorizes it.
macro_rules! impl_ranked_lexical_page_serde {
    ($ty:ident, $fields:ident, $visitor:ident, $result_ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                check_ranked_page_v2(
                    &self.window,
                    self.results.len(),
                    self.results.iter().map(<$result_ty>::order_key),
                    self.next_cursor.as_ref(),
                )
                .map_err(serde::ser::Error::custom)?;
                let field_count = if self.next_cursor.is_some() { 4 } else { 3 };
                let mut state = serializer.serialize_struct(stringify!($ty), field_count)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field("results", &self.results)?;
                state.serialize_field("window", &self.window)?;
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
                let mut window: Option<QueryResultWindowV2> = None;
                let mut next_cursor: Option<ContinuationTokenV2> = None;
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
                        "next_cursor" => {
                            if next_cursor_seen {
                                return Err(de::Error::duplicate_field("next_cursor"));
                            }
                            next_cursor_seen = true;
                            next_cursor = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                let generation =
                    generation.ok_or_else(|| de::Error::missing_field("generation"))?;
                let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
                let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
                check_ranked_page_v2(
                    &window,
                    results.len(),
                    results.iter().map(<$result_ty>::order_key),
                    next_cursor.as_ref(),
                )
                .map_err(de::Error::custom)?;
                Ok($ty {
                    generation,
                    results,
                    window,
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

/// Manual serde for one keyset page (QI-BB-025 W4, S21-06): `{
/// generation, results, window, <epochs...>, examined, next_cursor? }`.
///
/// The rows are in the route's total order — `candidate_id` ascending —
/// and the decoder holds the page to it fail-closed: `window.returned`
/// counts the rows, the rows strictly ascend by candidate id, and the
/// opaque token is present exactly when the window's outcome authorizes
/// a continuation. Order and epoch agreement live inside the token,
/// verified by the cursor codec against the executing request — never
/// as cursor-shaped fields beside it.
macro_rules! impl_keyset_page_response_serde {
    (
        $ty:ident,
        $fields:ident,
        $visitor:ident,
        $result_ty:ty,
        epochs = [$($response_epoch:ident),+ $(,)?]
    ) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                check_keyset_page_v2(&self.window, self.results.len(), self.next_cursor.as_ref())
                    .map_err(serde::ser::Error::custom)?;
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
                let mut window: Option<QueryResultWindowV2> = None;
                $(let mut $response_epoch: Option<AuxEpochV1> = None;)+
                let mut examined: Option<u64> = None;
                let mut next_cursor: Option<ContinuationTokenV2> = None;
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
                check_keyset_page_v2(&window, results.len(), next_cursor.as_ref())
                    .map_err(de::Error::custom)?;
                // The page is in the total order the token continues.
                if results
                    .iter()
                    .zip(results.iter().skip(1))
                    .any(|(row, next)| row.candidate_id.as_str() >= next.candidate_id.as_str())
                {
                    return Err(de::Error::custom(
                        "keyset page rows are not strictly ascending by candidate id",
                    ));
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

/// The wire rule every keyset page shares: `returned` counts the rows
/// and the opaque token is present exactly when the window's outcome
/// authorizes a continuation.
fn check_keyset_page_v2(
    window: &QueryResultWindowV2,
    rows: usize,
    next_cursor: Option<&ContinuationTokenV2>,
) -> Result<(), String> {
    let returned = usize::try_from(window.returned())
        .map_err(|error| format!("query result window returned count cannot fit usize: {error}"))?;
    if returned != rows {
        return Err("query result window returned count does not match results length".to_string());
    }
    if continuation_authorized(window) != next_cursor.is_some() {
        return Err("keyset page outcome and next_cursor disagree: the token is present exactly when the outcome authorizes a continuation".to_string());
    }
    Ok(())
}

/// Serde for a `{ generation, results, window, explanation }` response:
/// the single V2 window is the only completeness authority.
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
                let mut window: Option<QueryResultWindowV2> = None;
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

/// Check one ranked page against its V2 window.
///
/// `returned` counts the rows, every row carries a finite score in
/// strict page order, and the opaque token is present exactly when the
/// window's outcome authorizes a continuation. Cursor-content agreement
/// (last row, generation) lives inside the token, verified by the cursor
/// codec — never on the wire.
fn check_ranked_page_v2<'a>(
    window: &QueryResultWindowV2,
    rows: usize,
    keys: impl IntoIterator<Item = LexicalRowOrderKey<'a>>,
    next_cursor: Option<&ContinuationTokenV2>,
) -> Result<(), String> {
    use core::cmp::Ordering;
    let returned = usize::try_from(window.returned())
        .map_err(|error| format!("query result window returned count cannot fit usize: {error}"))?;
    if returned != rows {
        return Err("query result window returned count does not match results length".to_string());
    }
    let mut last: Option<LexicalRowOrderKey<'a>> = None;
    for row in keys {
        if !row.score.is_finite() {
            return Err("a ranked row carries a non-finite score".to_string());
        }
        if last.is_some_and(|previous| previous.order(&row) != Ordering::Less) {
            return Err("ranked rows are not in page order".to_string());
        }
        last = Some(row);
    }
    if continuation_authorized(window) != next_cursor.is_some() {
        return Err("ranked page outcome and next_cursor disagree: the token is present exactly when the outcome authorizes a continuation".to_string());
    }
    Ok(())
}

impl Serialize for TextQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        validate_file_owner_projection_v1(&self.results, self.file_owner_rows.as_deref())
            .map_err(serde::ser::Error::custom)?;
        let mut field_count = 3usize;
        if self.file_owner_rows.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.next_cursor.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("TextQueryResponse", field_count)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("window", &self.window)?;
        if let Some(file_owner_rows) = &self.file_owner_rows {
            state.serialize_field("file_owner_rows", file_owner_rows)?;
        }
        if let Some(next_cursor) = &self.next_cursor {
            state.serialize_field("next_cursor", next_cursor)?;
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
        let mut window: Option<QueryResultWindowV2> = None;
        let mut file_owner_rows: Option<Vec<FileOwnerProjectionRow>> = None;
        let mut file_owner_rows_seen = false;
        let mut next_cursor: Option<ContinuationTokenV2> = None;
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
                "file_owner_rows" => {
                    if file_owner_rows_seen {
                        return Err(de::Error::duplicate_field("file_owner_rows"));
                    }
                    file_owner_rows_seen = true;
                    file_owner_rows = Some(map.next_value()?);
                }
                "next_cursor" => {
                    if next_cursor_seen {
                        return Err(de::Error::duplicate_field("next_cursor"));
                    }
                    next_cursor_seen = true;
                    next_cursor = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, TEXT_QUERY_RESPONSE_FIELDS));
                }
            }
        }
        let generation = generation.ok_or_else(|| de::Error::missing_field("generation"))?;
        let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
        let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
        check_ranked_page_v2(
            &window,
            results.len(),
            results.iter().map(LexicalCandidate::order_key),
            next_cursor.as_ref(),
        )
        .map_err(de::Error::custom)?;
        validate_file_owner_projection_v1(&results, file_owner_rows.as_deref())
            .map_err(de::Error::custom)?;
        Ok(TextQueryResponse {
            generation,
            results,
            window,
            file_owner_rows,
            next_cursor,
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

impl_ranked_lexical_page_serde!(
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
        let mut window: Option<QueryResultWindowV2> = None;
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

/// One page of runtime-metadata results (QI-BB-025 W4, S21-06).
///
/// `results` are in candidate-id order and `window` is the single
/// completeness authority: `returned` is the rows on it and the outcome
/// says whether a next page exists, in which case `next_cursor` carries
/// the opaque token continuing it. `examined` is how many candidates
/// the walk visited after its cursor to answer.
///
/// `read_epoch` is the runtime-metadata authority epoch the page's
/// predicates were evaluated against and `universe_epoch` the structural
/// authority epoch its chunk universe was joined from (QI-BB-020 W2):
/// the current ones, taken in one ledger read, for a fresh walk; the
/// token's for a continuation. The token binds both inside.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneRuntimeMetadataQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub window: QueryResultWindowV2,
    pub read_epoch: AuxEpochV1,
    pub universe_epoch: AuxEpochV1,
    pub examined: u64,
    pub next_cursor: Option<ContinuationTokenV2>,
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
    epochs = [read_epoch, universe_epoch]
);

/// One page of structural results (QI-BB-025 W4, S21-06).
///
/// `results` are in candidate-id order and `window` is the single
/// completeness authority: `returned` is the rows on it and the outcome
/// says whether a next page exists, in which case `next_cursor` carries
/// the opaque token continuing it. `examined` is how many matched
/// candidates the page selection walked.
///
/// `read_epoch` is the structural authority epoch every leaf of the
/// query — the pinned universe, each parse-tree match, each symbol
/// projection — was evaluated against (QI-BB-020 W2): the current one
/// for a fresh walk, the token's for a continuation. The token binds it
/// inside.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneStructuralQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<StructuralCandidate>,
    pub window: QueryResultWindowV2,
    pub read_epoch: AuxEpochV1,
    pub examined: u64,
    pub next_cursor: Option<ContinuationTokenV2>,
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
    epochs = [read_epoch]
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
    use crate::{ExhaustionProofV1, GenerationPin, ManifestGeneration, RepoId, RevisionId};

    fn sample_generation_pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("repo-seed").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("rev-seed").expect("static fixture ID satisfies canonical policy"),
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
            "window": serde_json::to_value(QueryResultWindowV2::exact_exhausted(
                1,
                ExhaustionProofV1::ProbeExhausted { fetched: 1 },
                Vec::new(),
            ))
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
            window: QueryResultWindowV2::exact_exhausted(
                1,
                ExhaustionProofV1::ProbeExhausted { fetched: 1 },
                Vec::new(),
            ),
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

    fn projection_fixture_row(candidate_id: &str, score: f32) -> LexicalCandidate {
        LexicalCandidate {
            candidate_id: candidate_id.to_string(),
            repo_id: RepoId::new("repo-seed").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-seed")
                .expect("static fixture ID satisfies canonical policy"),
            manifest_generation: ManifestGeneration::new(7),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            start_line: 1,
            end_line: 2,
            score,
            snippet: "needle".to_string(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }

    fn projection_fixture_owner(candidate: &LexicalCandidate) -> FileOwnerProjectionRow {
        FileOwnerProjectionRow {
            candidate_id: candidate.candidate_id.clone(),
            repo_id: candidate.repo_id.clone(),
            revision_id: candidate.revision_id.clone(),
            manifest_generation: candidate.manifest_generation,
            repo_relative_path: candidate.repo_relative_path.clone(),
            owners: vec!["ada".to_string()],
        }
    }

    #[test]
    fn file_owner_projection_absent_is_valid() {
        let results = vec![projection_fixture_row("cand-1", 2.0)];
        assert!(validate_file_owner_projection_v1(&results, None).is_ok());
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "fixture vec literal has exactly two rows; indices 0 and 1 are total by construction"
    )]
    #[test]
    fn file_owner_projection_exact_pairing_is_valid() {
        let results = vec![
            projection_fixture_row("cand-1", 2.0),
            projection_fixture_row("cand-2", 1.0),
        ];
        let rows = vec![
            projection_fixture_owner(&results[0]),
            projection_fixture_owner(&results[1]),
        ];
        assert!(validate_file_owner_projection_v1(&results, Some(&rows)).is_ok());
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "fixture vec literal has exactly two rows; indices 0 and 1 are total by construction"
    )]
    #[test]
    fn file_owner_projection_count_mismatch_fails() {
        let results = vec![
            projection_fixture_row("cand-1", 2.0),
            projection_fixture_row("cand-2", 1.0),
        ];
        let rows = vec![projection_fixture_owner(&results[0])];
        assert_eq!(
            validate_file_owner_projection_v1(&results, Some(&rows)),
            Err(FileOwnerProjectionErrorV1::RowCountMismatch { rows: 1, results: 2 })
        );
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "fixture vec literal has exactly two rows; indices 0 and 1 are total by construction"
    )]
    #[test]
    fn file_owner_projection_swapped_rows_fail() {
        let results = vec![
            projection_fixture_row("cand-1", 2.0),
            projection_fixture_row("cand-2", 1.0),
        ];
        let rows = vec![
            projection_fixture_owner(&results[1]),
            projection_fixture_owner(&results[0]),
        ];
        assert_eq!(
            validate_file_owner_projection_v1(&results, Some(&rows)),
            Err(FileOwnerProjectionErrorV1::CandidateMismatch { position: 0 })
        );
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "fixture vec literal has exactly two rows; indices 0 and 1 are total by construction"
    )]
    #[test]
    fn text_response_with_swapped_projection_refuses_encode_and_decode() {
        let results = vec![
            projection_fixture_row("cand-1", 2.0),
            projection_fixture_row("cand-2", 1.0),
        ];
        let valid = TextQueryResponse {
            generation: sample_generation_pin(),
            results: results.clone(),
            window: QueryResultWindowV2::exact_probe(2),
            file_owner_rows: Some(vec![
                projection_fixture_owner(&results[0]),
                projection_fixture_owner(&results[1]),
            ]),
            next_cursor: None,
        };
        let mut swapped = serde_json::to_value(&valid).expect("valid page must serialize");
        let rows = swapped
            .get_mut("file_owner_rows")
            .and_then(serde_json::Value::as_array_mut)
            .expect("projection rows serialize as an array");
        assert_eq!(rows.len(), 2);
        rows.swap(0, 1);
        assert!(
            serde_json::from_value::<TextQueryResponse>(swapped).is_err(),
            "a swapped projection must fail decode"
        );

        let mistyped = TextQueryResponse {
            file_owner_rows: Some(vec![
                projection_fixture_owner(&results[1]),
                projection_fixture_owner(&results[0]),
            ]),
            ..valid
        };
        assert!(
            serde_json::to_value(&mistyped).is_err(),
            "a swapped projection must fail encode"
        );
    }
}
