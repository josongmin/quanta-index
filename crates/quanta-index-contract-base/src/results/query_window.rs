//! Query result-window semantics that do not require an O(N) count query.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateCountV1 {
    Exact(u64),
    AtLeast(u64),
}

const CANDIDATE_COUNT_V1_FIELDS: &[&str] = &["kind", "value"];

impl Serialize for CandidateCountV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (kind, value) = match *self {
            Self::Exact(value) => ("exact", value),
            Self::AtLeast(value) => ("at_least", value),
        };
        let mut state = serializer.serialize_struct("CandidateCountV1", 2)?;
        state.serialize_field("kind", kind)?;
        state.serialize_field("value", &value)?;
        state.end()
    }
}

struct CandidateCountV1Visitor;

impl<'de> Visitor<'de> for CandidateCountV1Visitor {
    type Value = CandidateCountV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a CandidateCountV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut value: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "value" => {
                    if value.is_some() {
                        return Err(de::Error::duplicate_field("value"));
                    }
                    value = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, CANDIDATE_COUNT_V1_FIELDS));
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        let value = value.ok_or_else(|| de::Error::missing_field("value"))?;
        match kind.as_str() {
            "exact" => Ok(CandidateCountV1::Exact(value)),
            "at_least" => Ok(CandidateCountV1::AtLeast(value)),
            other => Err(de::Error::unknown_variant(other, &["exact", "at_least"])),
        }
    }
}

impl<'de> Deserialize<'de> for CandidateCountV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "CandidateCountV1",
            CANDIDATE_COUNT_V1_FIELDS,
            CandidateCountV1Visitor,
        )
    }
}

impl CandidateCountV1 {
    #[must_use]
    pub const fn lower_bound(self) -> u64 {
        match self {
            Self::Exact(value) | Self::AtLeast(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueryResultWindowV1 {
    returned: u32,
    candidate_count: CandidateCountV1,
    has_more: bool,
}

impl QueryResultWindowV1 {
    #[must_use]
    pub fn exact(returned: u32) -> Self {
        Self {
            returned,
            candidate_count: CandidateCountV1::Exact(u64::from(returned)),
            has_more: false,
        }
    }

    /// Construct a window from a `top_k + 1` probe. `observed` is the number
    /// fetched before truncating to `requested`.
    pub fn from_probe(requested: u32, observed: usize) -> Result<Self, &'static str> {
        let requested_usize = usize::try_from(requested).map_err(|_error| "top_k exceeds usize")?;
        let returned_usize = observed.min(requested_usize);
        let returned = u32::try_from(returned_usize).map_err(|_error| "returned exceeds u32")?;
        if observed > requested_usize {
            let lower_bound =
                u64::try_from(observed).map_err(|_error| "candidate count exceeds u64")?;
            Ok(Self {
                returned,
                candidate_count: CandidateCountV1::AtLeast(lower_bound),
                has_more: true,
            })
        } else {
            Ok(Self::exact(returned))
        }
    }

    pub fn new(
        returned: u32,
        candidate_count: CandidateCountV1,
        has_more: bool,
    ) -> Result<Self, &'static str> {
        let lower_bound = candidate_count.lower_bound();
        if lower_bound < u64::from(returned) {
            return Err("candidate count lower bound is below returned row count");
        }
        match (candidate_count, has_more) {
            (CandidateCountV1::Exact(exact), false) if exact == u64::from(returned) => {}
            (CandidateCountV1::Exact(exact), true) if exact > u64::from(returned) => {}
            (CandidateCountV1::AtLeast(lower), true) if lower > u64::from(returned) => {}
            (CandidateCountV1::Exact(_), _) => {
                return Err("exact count and has_more contradict returned rows");
            }
            (CandidateCountV1::AtLeast(_), false) => {
                return Err("at-least count requires an observed continuation row");
            }
            (CandidateCountV1::AtLeast(_), true) => {
                return Err("at-least lower bound must exceed returned rows");
            }
        }
        Ok(Self {
            returned,
            candidate_count,
            has_more,
        })
    }

    #[must_use]
    pub const fn returned(&self) -> u32 {
        self.returned
    }

    #[must_use]
    pub const fn candidate_count(&self) -> CandidateCountV1 {
        self.candidate_count
    }

    #[must_use]
    pub const fn has_more(self) -> bool {
        self.has_more
    }
}

const QUERY_RESULT_WINDOW_V1_FIELDS: &[&str] = &["returned", "candidate_count", "has_more"];

impl Serialize for QueryResultWindowV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("QueryResultWindowV1", 3)?;
        state.serialize_field("returned", &self.returned)?;
        state.serialize_field("candidate_count", &self.candidate_count)?;
        state.serialize_field("has_more", &self.has_more)?;
        state.end()
    }
}

struct QueryResultWindowV1Visitor;

impl<'de> Visitor<'de> for QueryResultWindowV1Visitor {
    type Value = QueryResultWindowV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a QueryResultWindowV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut returned: Option<u32> = None;
        let mut candidate_count: Option<CandidateCountV1> = None;
        let mut has_more: Option<bool> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "returned" => {
                    if returned.is_some() {
                        return Err(de::Error::duplicate_field("returned"));
                    }
                    returned = Some(map.next_value()?);
                }
                "candidate_count" => {
                    if candidate_count.is_some() {
                        return Err(de::Error::duplicate_field("candidate_count"));
                    }
                    candidate_count = Some(map.next_value()?);
                }
                "has_more" => {
                    if has_more.is_some() {
                        return Err(de::Error::duplicate_field("has_more"));
                    }
                    has_more = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        QUERY_RESULT_WINDOW_V1_FIELDS,
                    ));
                }
            }
        }
        QueryResultWindowV1::new(
            returned.ok_or_else(|| de::Error::missing_field("returned"))?,
            candidate_count.ok_or_else(|| de::Error::missing_field("candidate_count"))?,
            has_more.ok_or_else(|| de::Error::missing_field("has_more"))?,
        )
        .map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for QueryResultWindowV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "QueryResultWindowV1",
            QUERY_RESULT_WINDOW_V1_FIELDS,
            QueryResultWindowV1Visitor,
        )
    }
}

/// Why an interrupted lane stopped before it could prove anything about
/// the remaining universe (`ExecutionOutcomeV2::InterruptedPartial`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptedReasonV2 {
    /// The request deadline elapsed before the lanes finished.
    Deadline,
    /// The caller or runtime cancelled the request mid-execution.
    Cancelled,
    /// An examined-rows budget stopped the scan.
    ExaminedBudget,
}

impl InterruptedReasonV2 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deadline => "deadline",
            Self::Cancelled => "cancelled",
            Self::ExaminedBudget => "examined_budget",
        }
    }

    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::Deadline, Self::Cancelled, Self::ExaminedBudget]
    }

    #[must_use]
    pub fn from_wire_str(value: &str) -> Option<Self> {
        match value {
            "deadline" => Some(Self::Deadline),
            "cancelled" => Some(Self::Cancelled),
            "examined_budget" => Some(Self::ExaminedBudget),
            _ => None,
        }
    }
}

/// Which approximate method produced the rows
/// (`ExecutionOutcomeV2::Approximate`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApproximateMethodV2 {
    /// ANN search over a quantized or HNSW-graph index: recall is bounded
    /// by the index, not by the request.
    AnnSearch,
    /// A filtered lane refilled from capped fetches under exact filters.
    FilteredRefill,
}

impl ApproximateMethodV2 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnnSearch => "ann_search",
            Self::FilteredRefill => "filtered_refill",
        }
    }

    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::AnnSearch, Self::FilteredRefill]
    }

    #[must_use]
    pub fn from_wire_str(value: &str) -> Option<Self> {
        match value {
            "ann_search" => Some(Self::AnnSearch),
            "filtered_refill" => Some(Self::FilteredRefill),
            _ => None,
        }
    }
}

/// The quality contract an approximate outcome carries: what the caller
/// can still rely on when the method gave up exactness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApproximateQualityContractV2 {
    /// Lower bound on how many rows the method examined before answering.
    examined_lower_bound: u64,
}

impl ApproximateQualityContractV2 {
    /// Construct a contract. The lower bound is mandatory: zero is a valid
    /// observation, but the field itself must be an observation, not a
    /// missing value.
    #[must_use]
    pub const fn new(examined_lower_bound: u64) -> Self {
        Self {
            examined_lower_bound,
        }
    }

    #[must_use]
    pub const fn examined_lower_bound(self) -> u64 {
        self.examined_lower_bound
    }
}

/// Closed execution outcome of one query (S21-06).
///
/// The row count never infers the outcome: every variant is a typed
/// statement the executor proved (or explicitly failed to prove) about the
/// universe it examined. `ExactExhausted` requires an exhaustion proof on
/// the window; `CappedUnknown`, `InterruptedPartial` and `Approximate`
/// can never be promoted to `ExactExhausted` — no such conversion exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionOutcomeV2 {
    /// The executor proved the whole route universe was observed: an
    /// exhaustion proof must accompany this variant on the window.
    ExactExhausted,
    /// The executor observed a lower bound only. `continuation` is whether
    /// a continuation boundary was observed (a `top_k + 1` probe row).
    LowerBound { continuation: bool },
    /// An internal cap stopped the scan before exhaustion could be known
    /// either way. `cap` is the cap that was hit.
    CappedUnknown { cap: u32 },
    /// Execution stopped early for a typed reason; the returned rows are a
    /// prefix of an unknown universe.
    InterruptedPartial { reason: InterruptedReasonV2 },
    /// The method itself is approximate; the quality contract says what
    /// was examined.
    Approximate {
        method: ApproximateMethodV2,
        quality_contract: ApproximateQualityContractV2,
    },
}

impl ExecutionOutcomeV2 {
    /// Whether this outcome is a verified exhaustion of the route
    /// universe. Only `ExactExhausted` qualifies; the window additionally
    /// requires the proof object.
    #[must_use]
    pub const fn is_exhausted(self) -> bool {
        matches!(self, Self::ExactExhausted)
    }

    /// The three-way `has_more` answer. `None` means "unknown" — the
    /// outcome does not authorize any claim in either direction. A route
    /// must report `None` as unknown, never as `false`.
    #[must_use]
    pub const fn has_more(self) -> Option<bool> {
        match self {
            Self::ExactExhausted => Some(false),
            Self::LowerBound { continuation: true } => Some(true),
            Self::LowerBound {
                continuation: false,
            }
            | Self::CappedUnknown { .. }
            | Self::InterruptedPartial { .. }
            | Self::Approximate { .. } => None,
        }
    }
}

/// How the executor proved that nothing remains after the returned rows.
/// `has_more = false` is constructible only when one of these is carried.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExhaustionProofV1 {
    /// Pure request constraints prove an empty universe without backend execution.
    LogicalEmpty,
    /// A `top_k + 1` probe observed every row the universe had: `fetched`
    /// rows came back for a fetch of `top_k + 1`, and `fetched <= top_k`.
    ProbeExhausted { fetched: u32 },
    /// The backend reported the exact match total and it equals the
    /// returned rows.
    ExactCount { total: u64 },
    /// A sealed immutable universe was scanned end to end: `scanned` rows
    /// were enumerated with no cutoff.
    UniverseScanned { scanned: u64 },
}

/// What was examined, as a bounded claim. `Unknown` is a typed state, not
/// a missing value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExaminedUniverseV1 {
    Exact(u64),
    AtLeast(u64),
    #[default]
    Unknown,
}

/// Why a window that executed carries zero rows. The three empty states
/// stay distinct; an unavailable backend is a typed refusal and never a
/// window variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmptyProvenanceV2 {
    /// A valid request has contradictory constraints; no backend search ran.
    LogicalEmpty,
    /// The universe executed and holds zero matching rows.
    AvailableEmpty,
    /// Rows existed but every one was excluded by filters.
    FilteredEmpty,
    /// Engines executed and scored, returning zero hits (cost and lanes
    /// still recorded on the coverage).
    ZeroHitExecuted,
}

impl EmptyProvenanceV2 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LogicalEmpty => "logical_empty",
            Self::AvailableEmpty => "available_empty",
            Self::FilteredEmpty => "filtered_empty",
            Self::ZeroHitExecuted => "zero_hit_executed",
        }
    }

    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::LogicalEmpty,
            Self::AvailableEmpty,
            Self::FilteredEmpty,
            Self::ZeroHitExecuted,
        ]
    }

    #[must_use]
    pub fn from_wire_str(value: &str) -> Option<Self> {
        match value {
            "logical_empty" => Some(Self::LogicalEmpty),
            "available_empty" => Some(Self::AvailableEmpty),
            "filtered_empty" => Some(Self::FilteredEmpty),
            "zero_hit_executed" => Some(Self::ZeroHitExecuted),
            _ => None,
        }
    }
}

/// One query lane the executor ran.
///
/// Whether it executed at all, whether it contributed rows, how many
/// candidates it held, and what it cost. An executed zero-hit lane is
/// `executed = true, contributed = false` with its candidate count and
/// cost recorded — never dropped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaneTraceV1 {
    lane: &'static str,
    executed: bool,
    contributed: bool,
    filtered_out: u64,
    candidates: CandidateCountV1,
    cost: Option<u64>,
    profile: Option<String>,
}

impl LaneTraceV1 {
    /// Construct a lane trace. `lane` is the lane's stable wire name.
    #[must_use]
    pub fn new(lane: &'static str, executed: bool, contributed: bool) -> Self {
        Self {
            lane,
            executed,
            contributed,
            filtered_out: 0,
            candidates: CandidateCountV1::Exact(0),
            cost: None,
            profile: None,
        }
    }

    #[must_use]
    pub const fn lane(&self) -> &'static str {
        self.lane
    }

    #[must_use]
    pub const fn executed(&self) -> bool {
        self.executed
    }

    #[must_use]
    pub const fn contributed(&self) -> bool {
        self.contributed
    }

    #[must_use]
    pub const fn filtered_out(&self) -> u64 {
        self.filtered_out
    }

    #[must_use]
    pub const fn candidates(&self) -> CandidateCountV1 {
        self.candidates
    }

    #[must_use]
    pub const fn cost(&self) -> Option<u64> {
        self.cost
    }

    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// Set the filtered-out row count.
    #[must_use]
    pub const fn with_filtered_out(mut self, filtered_out: u64) -> Self {
        self.filtered_out = filtered_out;
        self
    }

    /// Set the lane's candidate count.
    #[must_use]
    pub const fn with_candidates(mut self, candidates: CandidateCountV1) -> Self {
        self.candidates = candidates;
        self
    }

    /// Set the lane's cost observation.
    #[must_use]
    pub const fn with_cost(mut self, cost: u64) -> Self {
        self.cost = Some(cost);
        self
    }

    /// Set the lane's model/profile name.
    #[must_use]
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = Some(profile.into());
        self
    }
}

/// Coverage of one executed query: the examined universe as a bounded
/// claim, the exhaustion proof when one exists, and the per-lane traces.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoverageV1 {
    examined: ExaminedUniverseV1,
    exhaustion_proof: Option<ExhaustionProofV1>,
    lanes: Vec<LaneTraceV1>,
}

impl CoverageV1 {
    /// Construct coverage from its parts.
    #[must_use]
    pub fn new(
        examined: ExaminedUniverseV1,
        exhaustion_proof: Option<ExhaustionProofV1>,
        lanes: Vec<LaneTraceV1>,
    ) -> Self {
        Self {
            examined,
            exhaustion_proof,
            lanes,
        }
    }

    #[must_use]
    pub const fn examined(&self) -> ExaminedUniverseV1 {
        self.examined
    }

    #[must_use]
    pub const fn exhaustion_proof(&self) -> Option<ExhaustionProofV1> {
        self.exhaustion_proof
    }

    #[must_use]
    pub fn lanes(&self) -> &[LaneTraceV1] {
        &self.lanes
    }
}

/// The V2 result window: rows plus a typed outcome and its proof.
///
/// `has_more = false` is only reachable through
/// `ExecutionOutcomeV2::ExactExhausted` with an exhaustion proof on the
/// coverage; capped, partial and approximate outcomes never become exact
/// because no such conversion exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryResultWindowV2 {
    returned: u32,
    candidate_count: CandidateCountV1,
    outcome: ExecutionOutcomeV2,
    coverage: CoverageV1,
    empty_provenance: Option<EmptyProvenanceV2>,
}

impl QueryResultWindowV2 {
    /// Construct a window, validating every outcome/coverage/row-count
    /// invariant. Fails closed on any contradiction.
    pub fn new(
        returned: u32,
        candidate_count: CandidateCountV1,
        outcome: ExecutionOutcomeV2,
        coverage: CoverageV1,
        empty_provenance: Option<EmptyProvenanceV2>,
    ) -> Result<Self, &'static str> {
        if candidate_count.lower_bound() < u64::from(returned) {
            return Err("candidate count lower bound is below returned row count");
        }
        if empty_provenance.is_some() {
            if returned != 0 {
                return Err("empty provenance is only valid on a zero-row window");
            }
            if coverage.lanes.iter().any(LaneTraceV1::contributed) {
                return Err("empty provenance contradicts a lane that contributed rows");
            }
        } else if returned == 0 {
            return Err("a zero-row window must state its empty provenance");
        }
        let logical_proof = coverage.exhaustion_proof == Some(ExhaustionProofV1::LogicalEmpty);
        let logical_provenance = empty_provenance == Some(EmptyProvenanceV2::LogicalEmpty);
        if logical_proof || logical_provenance {
            if !logical_proof || !logical_provenance {
                return Err("logical empty requires matching proof and provenance");
            }
            if returned != 0
                || candidate_count != CandidateCountV1::Exact(0)
                || outcome != ExecutionOutcomeV2::ExactExhausted
                || coverage.examined != ExaminedUniverseV1::Exact(0)
                || coverage.lanes.iter().any(|lane| {
                    lane.executed()
                        || lane.contributed()
                        || lane.filtered_out() != 0
                        || lane.candidates() != CandidateCountV1::Exact(0)
                })
            {
                return Err("logical empty contradicts rows, counts or backend execution");
            }
        }
        match outcome {
            ExecutionOutcomeV2::ExactExhausted => {
                let proof = coverage.exhaustion_proof.ok_or(
                    "exact exhausted outcome requires an exhaustion proof on the coverage",
                )?;
                let exact = match candidate_count {
                    CandidateCountV1::Exact(exact) => exact,
                    CandidateCountV1::AtLeast(_) => {
                        return Err("exact exhausted outcome requires an exact candidate count");
                    }
                };
                if exact != u64::from(returned) {
                    return Err("exact exhausted count must equal returned rows");
                }
                match proof {
                    ExhaustionProofV1::LogicalEmpty => {}
                    ExhaustionProofV1::ProbeExhausted { fetched } => {
                        if fetched != returned {
                            return Err("probe exhaustion proof must match returned rows");
                        }
                    }
                    ExhaustionProofV1::ExactCount { total } => {
                        if total != exact {
                            return Err("exact-count exhaustion proof must match candidate count");
                        }
                    }
                    ExhaustionProofV1::UniverseScanned { scanned } => {
                        if scanned != exact {
                            return Err(
                                "universe-scan exhaustion proof must match candidate count",
                            );
                        }
                    }
                }
            }
            ExecutionOutcomeV2::LowerBound { continuation: true } => {
                if candidate_count.lower_bound() <= u64::from(returned) {
                    return Err("observed continuation requires a lower bound above returned rows");
                }
                if coverage.exhaustion_proof.is_some() {
                    return Err("a continuation row contradicts an exhaustion proof");
                }
            }
            ExecutionOutcomeV2::LowerBound {
                continuation: false,
            } => {
                if coverage.exhaustion_proof.is_some() {
                    return Err("lower-bound outcome without continuation cannot claim exhaustion");
                }
            }
            ExecutionOutcomeV2::CappedUnknown { cap } => {
                if cap == 0 {
                    return Err("capped outcome requires a positive cap");
                }
                if coverage.exhaustion_proof.is_some() {
                    return Err("a capped scan cannot carry an exhaustion proof");
                }
            }
            ExecutionOutcomeV2::InterruptedPartial { .. } => {
                if coverage.exhaustion_proof.is_some() {
                    return Err("an interrupted scan cannot carry an exhaustion proof");
                }
            }
            ExecutionOutcomeV2::Approximate { .. } => {
                if coverage.exhaustion_proof.is_some() {
                    return Err("an approximate method cannot carry an exhaustion proof");
                }
            }
        }
        Ok(Self {
            returned,
            candidate_count,
            outcome,
            coverage,
            empty_provenance,
        })
    }

    /// The exact window, refusing an inconsistent proof or lane observation.
    pub fn exact_exhausted(
        returned: u32,
        proof: ExhaustionProofV1,
        lanes: Vec<LaneTraceV1>,
    ) -> Result<Self, &'static str> {
        Self::new(
            returned,
            CandidateCountV1::Exact(u64::from(returned)),
            ExecutionOutcomeV2::ExactExhausted,
            CoverageV1::new(
                ExaminedUniverseV1::Exact(u64::from(returned)),
                Some(proof),
                lanes,
            ),
            (returned == 0).then_some(if proof == ExhaustionProofV1::LogicalEmpty {
                EmptyProvenanceV2::LogicalEmpty
            } else {
                EmptyProvenanceV2::AvailableEmpty
            }),
        )
    }

    /// Exact probe fixture. All fields are derived from the same row count.
    #[must_use]
    pub fn exact_probe(returned: u32) -> Self {
        Self {
            returned,
            candidate_count: CandidateCountV1::Exact(u64::from(returned)),
            outcome: ExecutionOutcomeV2::ExactExhausted,
            coverage: CoverageV1::new(
                ExaminedUniverseV1::Exact(u64::from(returned)),
                Some(ExhaustionProofV1::ProbeExhausted { fetched: returned }),
                Vec::new(),
            ),
            empty_provenance: (returned == 0).then_some(EmptyProvenanceV2::AvailableEmpty),
        }
    }

    /// A valid request whose constraints alone prove no result can exist.
    /// No backend execution, count collector or probe is claimed.
    #[must_use]
    pub fn logical_empty(lane: &'static str) -> Self {
        Self {
            returned: 0,
            candidate_count: CandidateCountV1::Exact(0),
            outcome: ExecutionOutcomeV2::ExactExhausted,
            coverage: CoverageV1::new(
                ExaminedUniverseV1::Exact(0),
                Some(ExhaustionProofV1::LogicalEmpty),
                vec![LaneTraceV1::new(lane, false, false)],
            ),
            empty_provenance: Some(EmptyProvenanceV2::LogicalEmpty),
        }
    }

    /// Build the V2 authority from a pageable adapter observation. A
    /// continuation is a lower-bound proof; no continuation is exact only
    /// when the adapter supplied an exact count equal to the returned rows.
    pub fn pageable(
        returned: u32,
        candidate_count: CandidateCountV1,
        continuation: bool,
        lanes: Vec<LaneTraceV1>,
    ) -> Result<Self, &'static str> {
        let (outcome, proof) = if continuation {
            (ExecutionOutcomeV2::LowerBound { continuation: true }, None)
        } else {
            match candidate_count {
                CandidateCountV1::Exact(exact) if exact == u64::from(returned) => (
                    ExecutionOutcomeV2::ExactExhausted,
                    Some(ExhaustionProofV1::ExactCount { total: exact }),
                ),
                CandidateCountV1::Exact(_) => {
                    return Err(
                        "a page without continuation has an exact count above returned rows",
                    );
                }
                CandidateCountV1::AtLeast(_) => (
                    ExecutionOutcomeV2::LowerBound {
                        continuation: false,
                    },
                    None,
                ),
            }
        };
        let examined = match candidate_count {
            CandidateCountV1::Exact(exact) => ExaminedUniverseV1::Exact(exact),
            CandidateCountV1::AtLeast(lower) => ExaminedUniverseV1::AtLeast(lower),
        };
        Self::new(
            returned,
            candidate_count,
            outcome,
            CoverageV1::new(examined, proof, lanes),
            (returned == 0).then_some(EmptyProvenanceV2::AvailableEmpty),
        )
    }

    #[must_use]
    pub const fn returned(&self) -> u32 {
        self.returned
    }

    #[must_use]
    pub const fn candidate_count(&self) -> CandidateCountV1 {
        self.candidate_count
    }

    #[must_use]
    pub const fn outcome(&self) -> ExecutionOutcomeV2 {
        self.outcome
    }

    #[must_use]
    pub const fn coverage(&self) -> &CoverageV1 {
        &self.coverage
    }

    #[must_use]
    pub const fn empty_provenance(&self) -> Option<EmptyProvenanceV2> {
        self.empty_provenance
    }

    /// Three-way `has_more` from the typed outcome. `None` is unknown;
    /// callers must not coerce it to `false`.
    #[must_use]
    pub const fn has_more(&self) -> Option<bool> {
        self.outcome.has_more()
    }
}

const APPROXIMATE_QUALITY_V2_FIELDS: &[&str] = &["examined_lower_bound"];

impl Serialize for ApproximateQualityContractV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ApproximateQualityContractV2", 1)?;
        state.serialize_field("examined_lower_bound", &self.examined_lower_bound)?;
        state.end()
    }
}

struct ApproximateQualityContractV2Visitor;

impl<'de> Visitor<'de> for ApproximateQualityContractV2Visitor {
    type Value = ApproximateQualityContractV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ApproximateQualityContractV2 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut examined_lower_bound: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "examined_lower_bound" => {
                    if examined_lower_bound.is_some() {
                        return Err(de::Error::duplicate_field("examined_lower_bound"));
                    }
                    examined_lower_bound = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        APPROXIMATE_QUALITY_V2_FIELDS,
                    ));
                }
            }
        }
        Ok(ApproximateQualityContractV2::new(
            examined_lower_bound.ok_or_else(|| de::Error::missing_field("examined_lower_bound"))?,
        ))
    }
}

impl<'de> Deserialize<'de> for ApproximateQualityContractV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ApproximateQualityContractV2",
            APPROXIMATE_QUALITY_V2_FIELDS,
            ApproximateQualityContractV2Visitor,
        )
    }
}

macro_rules! serialize_str_enum {
    ($ty:ty, $name:literal) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }
    };
}

serialize_str_enum!(InterruptedReasonV2, "InterruptedReasonV2");
serialize_str_enum!(ApproximateMethodV2, "ApproximateMethodV2");
serialize_str_enum!(EmptyProvenanceV2, "EmptyProvenanceV2");

struct InterruptedReasonV2Visitor;

impl Visitor<'_> for InterruptedReasonV2Visitor {
    type Value = InterruptedReasonV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an InterruptedReasonV2 string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        InterruptedReasonV2::from_wire_str(value).ok_or_else(|| {
            de::Error::unknown_variant(value, &["deadline", "cancelled", "examined_budget"])
        })
    }
}

impl<'de> Deserialize<'de> for InterruptedReasonV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(InterruptedReasonV2Visitor)
    }
}

struct ApproximateMethodV2Visitor;

impl Visitor<'_> for ApproximateMethodV2Visitor {
    type Value = ApproximateMethodV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ApproximateMethodV2 string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        ApproximateMethodV2::from_wire_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, &["ann_search", "filtered_refill"]))
    }
}

impl<'de> Deserialize<'de> for ApproximateMethodV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(ApproximateMethodV2Visitor)
    }
}

struct EmptyProvenanceV2Visitor;

impl Visitor<'_> for EmptyProvenanceV2Visitor {
    type Value = EmptyProvenanceV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EmptyProvenanceV2 string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        EmptyProvenanceV2::from_wire_str(value).ok_or_else(|| {
            de::Error::unknown_variant(
                value,
                &[
                    "logical_empty",
                    "available_empty",
                    "filtered_empty",
                    "zero_hit_executed",
                ],
            )
        })
    }
}

impl<'de> Deserialize<'de> for EmptyProvenanceV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(EmptyProvenanceV2Visitor)
    }
}

const EXECUTION_OUTCOME_V2_FIELDS: &[&str] = &[
    "kind",
    "continuation",
    "cap",
    "reason",
    "method",
    "quality_contract",
];

impl Serialize for ExecutionOutcomeV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Self::ExactExhausted => {
                let mut state = serializer.serialize_struct("ExecutionOutcomeV2", 1)?;
                state.serialize_field("kind", "exact_exhausted")?;
                state.end()
            }
            Self::LowerBound { continuation } => {
                let mut state = serializer.serialize_struct("ExecutionOutcomeV2", 2)?;
                state.serialize_field("kind", "lower_bound")?;
                state.serialize_field("continuation", &continuation)?;
                state.end()
            }
            Self::CappedUnknown { cap } => {
                let mut state = serializer.serialize_struct("ExecutionOutcomeV2", 2)?;
                state.serialize_field("kind", "capped_unknown")?;
                state.serialize_field("cap", &cap)?;
                state.end()
            }
            Self::InterruptedPartial { reason } => {
                let mut state = serializer.serialize_struct("ExecutionOutcomeV2", 2)?;
                state.serialize_field("kind", "interrupted_partial")?;
                state.serialize_field("reason", &reason)?;
                state.end()
            }
            Self::Approximate {
                method,
                quality_contract,
            } => {
                let mut state = serializer.serialize_struct("ExecutionOutcomeV2", 3)?;
                state.serialize_field("kind", "approximate")?;
                state.serialize_field("method", &method)?;
                state.serialize_field("quality_contract", &quality_contract)?;
                state.end()
            }
        }
    }
}

struct ExecutionOutcomeV2Visitor;

impl<'de> Visitor<'de> for ExecutionOutcomeV2Visitor {
    type Value = ExecutionOutcomeV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ExecutionOutcomeV2 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut continuation: Option<bool> = None;
        let mut cap: Option<u32> = None;
        let mut reason: Option<InterruptedReasonV2> = None;
        let mut method: Option<ApproximateMethodV2> = None;
        let mut quality_contract: Option<ApproximateQualityContractV2> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "continuation" => {
                    if continuation.is_some() {
                        return Err(de::Error::duplicate_field("continuation"));
                    }
                    continuation = Some(map.next_value()?);
                }
                "cap" => {
                    if cap.is_some() {
                        return Err(de::Error::duplicate_field("cap"));
                    }
                    cap = Some(map.next_value()?);
                }
                "reason" => {
                    if reason.is_some() {
                        return Err(de::Error::duplicate_field("reason"));
                    }
                    reason = Some(map.next_value()?);
                }
                "method" => {
                    if method.is_some() {
                        return Err(de::Error::duplicate_field("method"));
                    }
                    method = Some(map.next_value()?);
                }
                "quality_contract" => {
                    if quality_contract.is_some() {
                        return Err(de::Error::duplicate_field("quality_contract"));
                    }
                    quality_contract = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, EXECUTION_OUTCOME_V2_FIELDS));
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        let unexpected = |field: &str| -> String {
            format!("field `{field}` is not valid for outcome kind `{kind}`")
        };
        match kind.as_str() {
            "exact_exhausted" => {
                if continuation.is_some()
                    || cap.is_some()
                    || reason.is_some()
                    || method.is_some()
                    || quality_contract.is_some()
                {
                    return Err(de::Error::custom(unexpected("continuation/cap/...")));
                }
                Ok(ExecutionOutcomeV2::ExactExhausted)
            }
            "lower_bound" => {
                if cap.is_some()
                    || reason.is_some()
                    || method.is_some()
                    || quality_contract.is_some()
                {
                    return Err(de::Error::custom(unexpected("cap/reason/...")));
                }
                Ok(ExecutionOutcomeV2::LowerBound {
                    continuation: continuation
                        .ok_or_else(|| de::Error::missing_field("continuation"))?,
                })
            }
            "capped_unknown" => {
                if continuation.is_some()
                    || reason.is_some()
                    || method.is_some()
                    || quality_contract.is_some()
                {
                    return Err(de::Error::custom(unexpected("continuation/reason/...")));
                }
                Ok(ExecutionOutcomeV2::CappedUnknown {
                    cap: cap.ok_or_else(|| de::Error::missing_field("cap"))?,
                })
            }
            "interrupted_partial" => {
                if continuation.is_some()
                    || cap.is_some()
                    || method.is_some()
                    || quality_contract.is_some()
                {
                    return Err(de::Error::custom(unexpected("continuation/cap/...")));
                }
                Ok(ExecutionOutcomeV2::InterruptedPartial {
                    reason: reason.ok_or_else(|| de::Error::missing_field("reason"))?,
                })
            }
            "approximate" => {
                if continuation.is_some() || cap.is_some() || reason.is_some() {
                    return Err(de::Error::custom(unexpected("continuation/cap/reason")));
                }
                Ok(ExecutionOutcomeV2::Approximate {
                    method: method.ok_or_else(|| de::Error::missing_field("method"))?,
                    quality_contract: quality_contract
                        .ok_or_else(|| de::Error::missing_field("quality_contract"))?,
                })
            }
            other => Err(de::Error::unknown_variant(
                other,
                &[
                    "exact_exhausted",
                    "lower_bound",
                    "capped_unknown",
                    "interrupted_partial",
                    "approximate",
                ],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for ExecutionOutcomeV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ExecutionOutcomeV2",
            EXECUTION_OUTCOME_V2_FIELDS,
            ExecutionOutcomeV2Visitor,
        )
    }
}

const EXHAUSTION_PROOF_V1_FIELDS: &[&str] = &["kind", "fetched", "total", "scanned"];

impl Serialize for ExhaustionProofV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Self::LogicalEmpty => {
                let mut state = serializer.serialize_struct("ExhaustionProofV1", 1)?;
                state.serialize_field("kind", "logical_empty")?;
                state.end()
            }
            Self::ProbeExhausted { fetched } => {
                let mut state = serializer.serialize_struct("ExhaustionProofV1", 2)?;
                state.serialize_field("kind", "probe_exhausted")?;
                state.serialize_field("fetched", &fetched)?;
                state.end()
            }
            Self::ExactCount { total } => {
                let mut state = serializer.serialize_struct("ExhaustionProofV1", 2)?;
                state.serialize_field("kind", "exact_count")?;
                state.serialize_field("total", &total)?;
                state.end()
            }
            Self::UniverseScanned { scanned } => {
                let mut state = serializer.serialize_struct("ExhaustionProofV1", 2)?;
                state.serialize_field("kind", "universe_scanned")?;
                state.serialize_field("scanned", &scanned)?;
                state.end()
            }
        }
    }
}

struct ExhaustionProofV1Visitor;

impl<'de> Visitor<'de> for ExhaustionProofV1Visitor {
    type Value = ExhaustionProofV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ExhaustionProofV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut fetched: Option<u32> = None;
        let mut total: Option<u64> = None;
        let mut scanned: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "fetched" => {
                    if fetched.is_some() {
                        return Err(de::Error::duplicate_field("fetched"));
                    }
                    fetched = Some(map.next_value()?);
                }
                "total" => {
                    if total.is_some() {
                        return Err(de::Error::duplicate_field("total"));
                    }
                    total = Some(map.next_value()?);
                }
                "scanned" => {
                    if scanned.is_some() {
                        return Err(de::Error::duplicate_field("scanned"));
                    }
                    scanned = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, EXHAUSTION_PROOF_V1_FIELDS));
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        match kind.as_str() {
            "logical_empty" if fetched.is_none() && total.is_none() && scanned.is_none() => {
                Ok(ExhaustionProofV1::LogicalEmpty)
            }
            "probe_exhausted" if total.is_none() && scanned.is_none() => {
                Ok(ExhaustionProofV1::ProbeExhausted {
                    fetched: fetched.ok_or_else(|| de::Error::missing_field("fetched"))?,
                })
            }
            "exact_count" if fetched.is_none() && scanned.is_none() => {
                Ok(ExhaustionProofV1::ExactCount {
                    total: total.ok_or_else(|| de::Error::missing_field("total"))?,
                })
            }
            "universe_scanned" if fetched.is_none() && total.is_none() => {
                Ok(ExhaustionProofV1::UniverseScanned {
                    scanned: scanned.ok_or_else(|| de::Error::missing_field("scanned"))?,
                })
            }
            "logical_empty" | "probe_exhausted" | "exact_count" | "universe_scanned" => Err(
                de::Error::custom("exhaustion proof contains fields for another proof kind"),
            ),
            other => Err(de::Error::unknown_variant(
                other,
                &[
                    "logical_empty",
                    "probe_exhausted",
                    "exact_count",
                    "universe_scanned",
                ],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for ExhaustionProofV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ExhaustionProofV1",
            EXHAUSTION_PROOF_V1_FIELDS,
            ExhaustionProofV1Visitor,
        )
    }
}

const EXAMINED_UNIVERSE_V1_FIELDS: &[&str] = &["kind", "value"];

impl Serialize for ExaminedUniverseV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let (kind, value) = match *self {
            Self::Exact(value) => ("exact", Some(value)),
            Self::AtLeast(value) => ("at_least", Some(value)),
            Self::Unknown => ("unknown", None),
        };
        let field_count = if value.is_some() { 2 } else { 1 };
        let mut state = serializer.serialize_struct("ExaminedUniverseV1", field_count)?;
        state.serialize_field("kind", kind)?;
        if let Some(value) = value {
            state.serialize_field("value", &value)?;
        }
        state.end()
    }
}

struct ExaminedUniverseV1Visitor;

impl<'de> Visitor<'de> for ExaminedUniverseV1Visitor {
    type Value = ExaminedUniverseV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ExaminedUniverseV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut value: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "value" => {
                    if value.is_some() {
                        return Err(de::Error::duplicate_field("value"));
                    }
                    value = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, EXAMINED_UNIVERSE_V1_FIELDS));
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        match kind.as_str() {
            "exact" => Ok(ExaminedUniverseV1::Exact(
                value.ok_or_else(|| de::Error::missing_field("value"))?,
            )),
            "at_least" => Ok(ExaminedUniverseV1::AtLeast(
                value.ok_or_else(|| de::Error::missing_field("value"))?,
            )),
            "unknown" => {
                if value.is_some() {
                    return Err(de::Error::custom(
                        "field `value` is not valid for examined kind `unknown`",
                    ));
                }
                Ok(ExaminedUniverseV1::Unknown)
            }
            other => Err(de::Error::unknown_variant(
                other,
                &["exact", "at_least", "unknown"],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for ExaminedUniverseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ExaminedUniverseV1",
            EXAMINED_UNIVERSE_V1_FIELDS,
            ExaminedUniverseV1Visitor,
        )
    }
}

const LANE_TRACE_V1_FIELDS: &[&str] = &[
    "lane",
    "executed",
    "contributed",
    "filtered_out",
    "candidates",
    "cost",
    "profile",
];

impl Serialize for LaneTraceV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = match (self.cost.is_some(), self.profile.is_some()) {
            (true, true) => 7,
            (true, false) | (false, true) => 6,
            (false, false) => 5,
        };
        let mut state = serializer.serialize_struct("LaneTraceV1", field_count)?;
        state.serialize_field("lane", self.lane)?;
        state.serialize_field("executed", &self.executed)?;
        state.serialize_field("contributed", &self.contributed)?;
        state.serialize_field("filtered_out", &self.filtered_out)?;
        state.serialize_field("candidates", &self.candidates)?;
        if let Some(cost) = self.cost {
            state.serialize_field("cost", &cost)?;
        }
        if let Some(profile) = &self.profile {
            state.serialize_field("profile", profile)?;
        }
        state.end()
    }
}

struct LaneTraceV1Visitor;

impl<'de> Visitor<'de> for LaneTraceV1Visitor {
    type Value = LaneTraceV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LaneTraceV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut lane: Option<String> = None;
        let mut executed: Option<bool> = None;
        let mut contributed: Option<bool> = None;
        let mut filtered_out: Option<u64> = None;
        let mut candidates: Option<CandidateCountV1> = None;
        let mut cost: Option<u64> = None;
        let mut profile: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "lane" => {
                    if lane.is_some() {
                        return Err(de::Error::duplicate_field("lane"));
                    }
                    lane = Some(map.next_value()?);
                }
                "executed" => {
                    if executed.is_some() {
                        return Err(de::Error::duplicate_field("executed"));
                    }
                    executed = Some(map.next_value()?);
                }
                "contributed" => {
                    if contributed.is_some() {
                        return Err(de::Error::duplicate_field("contributed"));
                    }
                    contributed = Some(map.next_value()?);
                }
                "filtered_out" => {
                    if filtered_out.is_some() {
                        return Err(de::Error::duplicate_field("filtered_out"));
                    }
                    filtered_out = Some(map.next_value()?);
                }
                "candidates" => {
                    if candidates.is_some() {
                        return Err(de::Error::duplicate_field("candidates"));
                    }
                    candidates = Some(map.next_value()?);
                }
                "cost" => {
                    if cost.is_some() {
                        return Err(de::Error::duplicate_field("cost"));
                    }
                    cost = Some(map.next_value()?);
                }
                "profile" => {
                    if profile.is_some() {
                        return Err(de::Error::duplicate_field("profile"));
                    }
                    profile = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, LANE_TRACE_V1_FIELDS));
                }
            }
        }
        let lane = lane.ok_or_else(|| de::Error::missing_field("lane"))?;
        if lane.is_empty() {
            return Err(de::Error::custom("lane name must not be empty"));
        }
        // The lane name is a stable wire identifier; the trace type only
        // hands out `&'static str`, so leak the decoded name to preserve
        // that contract. Lane traces are few per response.
        let lane: &'static str = Box::leak(lane.into_boxed_str());
        let mut trace = LaneTraceV1::new(
            lane,
            executed.ok_or_else(|| de::Error::missing_field("executed"))?,
            contributed.ok_or_else(|| de::Error::missing_field("contributed"))?,
        )
        .with_filtered_out(filtered_out.unwrap_or(0))
        .with_candidates(candidates.ok_or_else(|| de::Error::missing_field("candidates"))?);
        if let Some(cost) = cost {
            trace = trace.with_cost(cost);
        }
        if let Some(profile) = profile {
            trace = trace.with_profile(profile);
        }
        Ok(trace)
    }
}

impl<'de> Deserialize<'de> for LaneTraceV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("LaneTraceV1", LANE_TRACE_V1_FIELDS, LaneTraceV1Visitor)
    }
}

const COVERAGE_V1_FIELDS: &[&str] = &["examined", "exhaustion_proof", "lanes"];

impl Serialize for CoverageV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.exhaustion_proof.is_some() {
            3
        } else {
            2
        };
        let mut state = serializer.serialize_struct("CoverageV1", field_count)?;
        state.serialize_field("examined", &self.examined)?;
        if let Some(proof) = self.exhaustion_proof {
            state.serialize_field("exhaustion_proof", &proof)?;
        }
        state.serialize_field("lanes", &self.lanes)?;
        state.end()
    }
}

struct CoverageV1Visitor;

impl<'de> Visitor<'de> for CoverageV1Visitor {
    type Value = CoverageV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a CoverageV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut examined: Option<ExaminedUniverseV1> = None;
        let mut exhaustion_proof: Option<ExhaustionProofV1> = None;
        let mut lanes: Option<Vec<LaneTraceV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "examined" => {
                    if examined.is_some() {
                        return Err(de::Error::duplicate_field("examined"));
                    }
                    examined = Some(map.next_value()?);
                }
                "exhaustion_proof" => {
                    if exhaustion_proof.is_some() {
                        return Err(de::Error::duplicate_field("exhaustion_proof"));
                    }
                    exhaustion_proof = Some(map.next_value()?);
                }
                "lanes" => {
                    if lanes.is_some() {
                        return Err(de::Error::duplicate_field("lanes"));
                    }
                    lanes = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, COVERAGE_V1_FIELDS));
                }
            }
        }
        Ok(CoverageV1::new(
            examined.unwrap_or(ExaminedUniverseV1::Unknown),
            exhaustion_proof,
            lanes.ok_or_else(|| de::Error::missing_field("lanes"))?,
        ))
    }
}

impl<'de> Deserialize<'de> for CoverageV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("CoverageV1", COVERAGE_V1_FIELDS, CoverageV1Visitor)
    }
}

const QUERY_RESULT_WINDOW_V2_FIELDS: &[&str] = &[
    "returned",
    "candidate_count",
    "outcome",
    "coverage",
    "empty_provenance",
];

impl Serialize for QueryResultWindowV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.empty_provenance.is_some() {
            5
        } else {
            4
        };
        let mut state = serializer.serialize_struct("QueryResultWindowV2", field_count)?;
        state.serialize_field("returned", &self.returned)?;
        state.serialize_field("candidate_count", &self.candidate_count)?;
        state.serialize_field("outcome", &self.outcome)?;
        state.serialize_field("coverage", &self.coverage)?;
        if let Some(provenance) = self.empty_provenance {
            state.serialize_field("empty_provenance", &provenance)?;
        }
        state.end()
    }
}

struct QueryResultWindowV2Visitor;

impl<'de> Visitor<'de> for QueryResultWindowV2Visitor {
    type Value = QueryResultWindowV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a QueryResultWindowV2 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut returned: Option<u32> = None;
        let mut candidate_count: Option<CandidateCountV1> = None;
        let mut outcome: Option<ExecutionOutcomeV2> = None;
        let mut coverage: Option<CoverageV1> = None;
        let mut empty_provenance: Option<EmptyProvenanceV2> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "returned" => {
                    if returned.is_some() {
                        return Err(de::Error::duplicate_field("returned"));
                    }
                    returned = Some(map.next_value()?);
                }
                "candidate_count" => {
                    if candidate_count.is_some() {
                        return Err(de::Error::duplicate_field("candidate_count"));
                    }
                    candidate_count = Some(map.next_value()?);
                }
                "outcome" => {
                    if outcome.is_some() {
                        return Err(de::Error::duplicate_field("outcome"));
                    }
                    outcome = Some(map.next_value()?);
                }
                "coverage" => {
                    if coverage.is_some() {
                        return Err(de::Error::duplicate_field("coverage"));
                    }
                    coverage = Some(map.next_value()?);
                }
                "empty_provenance" => {
                    if empty_provenance.is_some() {
                        return Err(de::Error::duplicate_field("empty_provenance"));
                    }
                    empty_provenance = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        QUERY_RESULT_WINDOW_V2_FIELDS,
                    ));
                }
            }
        }
        QueryResultWindowV2::new(
            returned.ok_or_else(|| de::Error::missing_field("returned"))?,
            candidate_count.ok_or_else(|| de::Error::missing_field("candidate_count"))?,
            outcome.ok_or_else(|| de::Error::missing_field("outcome"))?,
            coverage.ok_or_else(|| de::Error::missing_field("coverage"))?,
            empty_provenance,
        )
        .map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for QueryResultWindowV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "QueryResultWindowV2",
            QUERY_RESULT_WINDOW_V2_FIELDS,
            QueryResultWindowV2Visitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{CandidateCountV1, QueryResultWindowV1};

    #[test]
    fn probe_reports_exact_or_lower_bound_without_full_count() {
        assert_eq!(
            QueryResultWindowV1::from_probe(10, 7).expect("valid probe"),
            QueryResultWindowV1::exact(7)
        );
        assert_eq!(
            QueryResultWindowV1::from_probe(10, 11).expect("valid probe"),
            QueryResultWindowV1::new(10, CandidateCountV1::AtLeast(11), true)
                .expect("valid lower bound")
        );
    }

    #[test]
    fn contradictory_windows_fail_closed() {
        assert!(QueryResultWindowV1::new(10, CandidateCountV1::Exact(9), false).is_err());
        assert!(QueryResultWindowV1::new(10, CandidateCountV1::AtLeast(10), true).is_err());
        assert!(QueryResultWindowV1::new(10, CandidateCountV1::AtLeast(11), false).is_err());
    }

    #[test]
    fn result_window_preserves_wire_shape_and_validates_on_decode() {
        let window = QueryResultWindowV1::new(10, CandidateCountV1::AtLeast(11), true)
            .expect("valid result window");
        let encoded = serde_json::to_string(&window).expect("serialize result window");
        assert_eq!(
            encoded,
            r#"{"returned":10,"candidate_count":{"kind":"at_least","value":11},"has_more":true}"#
        );
        assert_eq!(
            serde_json::from_str::<QueryResultWindowV1>(&encoded)
                .expect("deserialize result window"),
            window
        );
        assert!(
            serde_json::from_str::<QueryResultWindowV1>(
                r#"{"returned":10,"candidate_count":{"kind":"exact","value":9},"has_more":false}"#,
            )
            .is_err()
        );
    }

    mod outcome_v2 {
        use super::super::{
            ApproximateMethodV2, ApproximateQualityContractV2, CandidateCountV1, CoverageV1,
            EmptyProvenanceV2, ExaminedUniverseV1, ExecutionOutcomeV2, ExhaustionProofV1,
            InterruptedReasonV2, LaneTraceV1, QueryResultWindowV2,
        };

        fn lanes() -> Vec<LaneTraceV1> {
            vec![
                LaneTraceV1::new("lexical", true, true)
                    .with_candidates(CandidateCountV1::Exact(7))
                    .with_cost(12),
            ]
        }

        #[test]
        fn exact_exhausted_requires_and_matches_its_proof() {
            let window = QueryResultWindowV2::new(
                7,
                CandidateCountV1::Exact(7),
                ExecutionOutcomeV2::ExactExhausted,
                CoverageV1::new(
                    ExaminedUniverseV1::Exact(7),
                    Some(ExhaustionProofV1::ProbeExhausted { fetched: 7 }),
                    lanes(),
                ),
                None,
            )
            .expect("valid exact window");
            assert_eq!(window.has_more(), Some(false));
            // A proof that disagrees with the rows fails closed.
            assert!(
                QueryResultWindowV2::new(
                    7,
                    CandidateCountV1::Exact(7),
                    ExecutionOutcomeV2::ExactExhausted,
                    CoverageV1::new(
                        ExaminedUniverseV1::Exact(7),
                        Some(ExhaustionProofV1::ProbeExhausted { fetched: 6 }),
                        lanes(),
                    ),
                    None,
                )
                .is_err()
            );
            // Missing proof fails closed.
            assert!(
                QueryResultWindowV2::new(
                    7,
                    CandidateCountV1::Exact(7),
                    ExecutionOutcomeV2::ExactExhausted,
                    CoverageV1::new(ExaminedUniverseV1::Exact(7), None, lanes()),
                    None,
                )
                .is_err()
            );
        }

        #[test]
        fn capped_partial_and_approximate_never_report_exhaustion() {
            for outcome in [
                ExecutionOutcomeV2::CappedUnknown { cap: 50 },
                ExecutionOutcomeV2::InterruptedPartial {
                    reason: InterruptedReasonV2::Deadline,
                },
                ExecutionOutcomeV2::Approximate {
                    method: ApproximateMethodV2::AnnSearch,
                    quality_contract: ApproximateQualityContractV2::new(512),
                },
            ] {
                let window = QueryResultWindowV2::new(
                    7,
                    CandidateCountV1::AtLeast(7),
                    outcome,
                    CoverageV1::new(ExaminedUniverseV1::AtLeast(7), None, lanes()),
                    None,
                )
                .expect("valid inexact window");
                assert_eq!(window.has_more(), None);
                assert!(!window.outcome().is_exhausted());
                // None of them may carry an exhaustion proof.
                assert!(
                    QueryResultWindowV2::new(
                        7,
                        CandidateCountV1::Exact(7),
                        outcome,
                        CoverageV1::new(
                            ExaminedUniverseV1::Exact(7),
                            Some(ExhaustionProofV1::ExactCount { total: 7 }),
                            lanes(),
                        ),
                        None,
                    )
                    .is_err()
                );
            }
        }

        #[test]
        fn lower_bound_continuation_requires_an_observed_extra_row() {
            assert!(
                QueryResultWindowV2::new(
                    7,
                    CandidateCountV1::AtLeast(8),
                    ExecutionOutcomeV2::LowerBound { continuation: true },
                    CoverageV1::new(ExaminedUniverseV1::AtLeast(8), None, lanes()),
                    None,
                )
                .is_ok()
            );
            assert!(
                QueryResultWindowV2::new(
                    7,
                    CandidateCountV1::AtLeast(7),
                    ExecutionOutcomeV2::LowerBound { continuation: true },
                    CoverageV1::new(ExaminedUniverseV1::AtLeast(7), None, lanes()),
                    None,
                )
                .is_err()
            );
        }

        #[test]
        fn empty_windows_carry_distinct_mandatory_provenance() {
            for provenance in [
                EmptyProvenanceV2::AvailableEmpty,
                EmptyProvenanceV2::FilteredEmpty,
                EmptyProvenanceV2::ZeroHitExecuted,
            ] {
                let window = QueryResultWindowV2::new(
                    0,
                    CandidateCountV1::Exact(0),
                    ExecutionOutcomeV2::ExactExhausted,
                    CoverageV1::new(
                        ExaminedUniverseV1::Exact(0),
                        Some(ExhaustionProofV1::ExactCount { total: 0 }),
                        vec![LaneTraceV1::new("lexical", true, false)],
                    ),
                    Some(provenance),
                )
                .expect("valid empty window");
                assert_eq!(window.has_more(), Some(false));
                assert_eq!(window.empty_provenance(), Some(provenance));
            }
            // Zero rows without provenance fail closed.
            assert!(
                QueryResultWindowV2::new(
                    0,
                    CandidateCountV1::Exact(0),
                    ExecutionOutcomeV2::ExactExhausted,
                    CoverageV1::new(
                        ExaminedUniverseV1::Exact(0),
                        Some(ExhaustionProofV1::ExactCount { total: 0 }),
                        Vec::new(),
                    ),
                    None,
                )
                .is_err()
            );
        }

        #[test]
        fn outcome_wire_shape_round_trips_and_rejects_cross_fields() {
            let window = QueryResultWindowV2::new(
                7,
                CandidateCountV1::AtLeast(8),
                ExecutionOutcomeV2::Approximate {
                    method: ApproximateMethodV2::FilteredRefill,
                    quality_contract: ApproximateQualityContractV2::new(128),
                },
                CoverageV1::new(ExaminedUniverseV1::AtLeast(8), None, lanes()),
                None,
            )
            .expect("valid approximate window");
            let encoded = serde_json::to_string(&window).expect("serialize v2 window");
            assert_eq!(
                encoded,
                r#"{"returned":7,"candidate_count":{"kind":"at_least","value":8},"outcome":{"kind":"approximate","method":"filtered_refill","quality_contract":{"examined_lower_bound":128}},"coverage":{"examined":{"kind":"at_least","value":8},"lanes":[{"lane":"lexical","executed":true,"contributed":true,"filtered_out":0,"candidates":{"kind":"exact","value":7},"cost":12}]}}"#
            );
            assert_eq!(
                serde_json::from_str::<QueryResultWindowV2>(&encoded).expect("decode v2 window"),
                window
            );
            // A capped kind carrying an approximate field fails decode.
            assert!(
                serde_json::from_str::<QueryResultWindowV2>(
                    r#"{"returned":7,"candidate_count":{"kind":"at_least","value":8},"outcome":{"kind":"capped_unknown","cap":50,"method":"ann_search"},"coverage":{"examined":{"kind":"unknown"},"lanes":[]}}"#
                )
                .is_err()
            );
            // Exact exhaustion without a coverage proof fails decode.
            assert!(
                serde_json::from_str::<QueryResultWindowV2>(
                    r#"{"returned":7,"candidate_count":{"kind":"exact","value":7},"outcome":{"kind":"exact_exhausted"},"coverage":{"examined":{"kind":"exact","value":7},"lanes":[]}}"#
                )
                .is_err()
            );
        }
    }
}

#[cfg(test)]
mod l1_logical_empty_tests {
    use super::*;

    #[test]
    fn logical_empty_has_explicit_proof_and_never_claims_backend_execution()
    -> Result<(), Box<dyn std::error::Error>> {
        let window = QueryResultWindowV2::logical_empty("symbol");
        if window.candidate_count() != CandidateCountV1::Exact(0)
            || window.has_more() != Some(false)
            || window.coverage().examined() != ExaminedUniverseV1::Exact(0)
            || window
                .coverage()
                .lanes()
                .iter()
                .any(|lane| lane.executed() || lane.contributed())
        {
            return Err("logical empty contradicts execution facts".into());
        }
        let encoded = serde_json::to_value(&window)?;
        if encoded.get("empty_provenance") != Some(&serde_json::json!("logical_empty"))
            || encoded
                .get("coverage")
                .and_then(|coverage| coverage.get("exhaustion_proof"))
                != Some(&serde_json::json!({"kind":"logical_empty"}))
            || serde_json::from_value::<QueryResultWindowV2>(encoded)? != window
        {
            return Err("logical empty wire proof differs from fixed oracle".into());
        }
        Ok(())
    }

    #[test]
    fn logical_empty_refuses_mixed_execution_and_proof_claims() {
        for (returned, count, outcome, examined, proof, provenance, lanes) in [
            (
                0,
                CandidateCountV1::Exact(0),
                ExecutionOutcomeV2::ExactExhausted,
                ExaminedUniverseV1::Exact(0),
                Some(ExhaustionProofV1::LogicalEmpty),
                Some(EmptyProvenanceV2::LogicalEmpty),
                vec![LaneTraceV1::new("symbol", true, false)],
            ),
            (
                1,
                CandidateCountV1::Exact(1),
                ExecutionOutcomeV2::ExactExhausted,
                ExaminedUniverseV1::Exact(1),
                Some(ExhaustionProofV1::LogicalEmpty),
                Some(EmptyProvenanceV2::LogicalEmpty),
                vec![],
            ),
            (
                0,
                CandidateCountV1::Exact(0),
                ExecutionOutcomeV2::ExactExhausted,
                ExaminedUniverseV1::Unknown,
                Some(ExhaustionProofV1::LogicalEmpty),
                Some(EmptyProvenanceV2::LogicalEmpty),
                vec![],
            ),
            (
                0,
                CandidateCountV1::Exact(0),
                ExecutionOutcomeV2::ExactExhausted,
                ExaminedUniverseV1::Exact(0),
                Some(ExhaustionProofV1::ExactCount { total: 0 }),
                Some(EmptyProvenanceV2::LogicalEmpty),
                vec![],
            ),
            (
                0,
                CandidateCountV1::Exact(0),
                ExecutionOutcomeV2::ExactExhausted,
                ExaminedUniverseV1::Exact(0),
                Some(ExhaustionProofV1::LogicalEmpty),
                Some(EmptyProvenanceV2::AvailableEmpty),
                vec![],
            ),
            (
                0,
                CandidateCountV1::AtLeast(0),
                ExecutionOutcomeV2::LowerBound {
                    continuation: false,
                },
                ExaminedUniverseV1::Exact(0),
                Some(ExhaustionProofV1::LogicalEmpty),
                Some(EmptyProvenanceV2::LogicalEmpty),
                vec![],
            ),
        ] {
            assert!(
                QueryResultWindowV2::new(
                    returned,
                    count,
                    outcome,
                    CoverageV1::new(examined, proof, lanes),
                    provenance
                )
                .is_err()
            );
        }
        assert!(
            QueryResultWindowV2::exact_exhausted(1, ExhaustionProofV1::LogicalEmpty, vec![])
                .is_err()
        );
        assert!(
            QueryResultWindowV2::exact_exhausted(
                0,
                ExhaustionProofV1::LogicalEmpty,
                vec![LaneTraceV1::new("symbol", true, false)]
            )
            .is_err()
        );
    }

    #[test]
    fn exhaustion_proof_decode_rejects_extra_missing_duplicate_or_unknown_fields() {
        for malformed in [
            r#"{"kind":"logical_empty","total":0}"#,
            r#"{"kind":"logical_empty","scanned":0}"#,
            r#"{"kind":"logical_empty","kind":"logical_empty"}"#,
            r#"{"kind":"logical_empty","extra":0}"#,
            r#"{"kind":"exact_count","total":0,"fetched":0}"#,
            r#"{"kind":"exact_count"}"#,
            r#"{"kind":"unknown"}"#,
        ] {
            assert!(
                serde_json::from_str::<ExhaustionProofV1>(malformed).is_err(),
                "{malformed}"
            );
        }
    }
}
