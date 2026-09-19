//! Canonical ranker-explanation + planner-trace surface (LEX-06 §11 +
//! LXE-07 §3.2).
//!
//! This module is the single source of truth for search-side explanations.
//! The previous `lex::explanation` duplicate has been deleted; downstream
//! callers either import from `quanta_index_contract::results::*` directly
//! or via the `lex::` re-export (see `crate::lex`).
//!
//! Per `CLAUDE.md` D18, every type below carries a manual `impl Serialize` /
//! `impl<'de> Deserialize<'de>`. No proc-macro derives.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

/// One contribution row: a named signal, its raw value, its weight, and the
/// product (`signal_value * weight`) recorded by the ranker.
///
/// `contribution` is producer-supplied (ranker-side) rather than recomputed
/// here; the ranker is the authority for the final number. This matches the
/// "no heuristic authority" rule in `CLAUDE.md`.
///
/// # Composition rule (QI-BB-022)
///
/// Rows are grouped by the namespace of `signal_name`, and each namespace
/// carries one unit:
///
/// - `lexical.<engine>` — the lexical lane's score in the engine's units
///   (BM25 for `lexical.bm25`): `signal_value` is the engine score,
///   `weight` the plan's boost factor, `contribution` the emitted lexical
///   score. A lexical explanation has exactly this row when the plan
///   matches the candidate, and its `contribution` is the emitted score.
/// - `dense.cosine` — the dense lane's score, a cosine similarity in
///   `[-1, 1]` re-derived from the candidate's stored vector: `weight` is
///   `1.0` and `contribution` equals `signal_value`.
/// - `hybrid.rrf.<lane>` — one row per lane whose re-run ranked the
///   candidate, carrying that lane's reciprocal-rank term
///   `1 / (k + rank)` (dimensionless): `signal_value` is `1.0` (the lane
///   saw it), `weight` and `contribution` are the term. The sum of the
///   `hybrid.rrf.*` rows is the fused RRF score the hybrid route ranks by,
///   to within `f32` rounding of each term.
///
/// Rows of different namespaces are never summed together: a lane score
/// row and an RRF row are in different units. A hybrid explanation's
/// fused score is `Σ hybrid.rrf.*`, not the sum of every row.
#[derive(Clone, Debug, PartialEq)]
pub struct ExplanationRow {
    pub signal_name: Box<str>,
    pub signal_value: f32,
    pub weight: f32,
    pub contribution: f32,
}

const EXPLANATION_ROW_FIELDS: &[&str] = &["signal_name", "signal_value", "weight", "contribution"];

impl Serialize for ExplanationRow {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ExplanationRow", 4)?;
        state.serialize_field("signal_name", self.signal_name.as_ref())?;
        state.serialize_field("signal_value", &self.signal_value)?;
        state.serialize_field("weight", &self.weight)?;
        state.serialize_field("contribution", &self.contribution)?;
        state.end()
    }
}

struct ExplanationRowVisitor;

impl<'de> Visitor<'de> for ExplanationRowVisitor {
    type Value = ExplanationRow;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ExplanationRow map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut signal_name: Option<String> = None;
        let mut signal_value: Option<f32> = None;
        let mut weight: Option<f32> = None;
        let mut contribution: Option<f32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "signal_name" => {
                    if signal_name.is_some() {
                        return Err(de::Error::duplicate_field("signal_name"));
                    }
                    signal_name = Some(map.next_value()?);
                }
                "signal_value" => {
                    if signal_value.is_some() {
                        return Err(de::Error::duplicate_field("signal_value"));
                    }
                    signal_value = Some(map.next_value()?);
                }
                "weight" => {
                    if weight.is_some() {
                        return Err(de::Error::duplicate_field("weight"));
                    }
                    weight = Some(map.next_value()?);
                }
                "contribution" => {
                    if contribution.is_some() {
                        return Err(de::Error::duplicate_field("contribution"));
                    }
                    contribution = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, EXPLANATION_ROW_FIELDS)),
            }
        }
        let signal_name = signal_name.ok_or_else(|| de::Error::missing_field("signal_name"))?;
        let signal_value = signal_value.ok_or_else(|| de::Error::missing_field("signal_value"))?;
        let weight = weight.ok_or_else(|| de::Error::missing_field("weight"))?;
        let contribution = contribution.ok_or_else(|| de::Error::missing_field("contribution"))?;
        Ok(ExplanationRow {
            signal_name: signal_name.into_boxed_str(),
            signal_value,
            weight,
            contribution,
        })
    }
}

impl<'de> Deserialize<'de> for ExplanationRow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ExplanationRow",
            EXPLANATION_ROW_FIELDS,
            ExplanationRowVisitor,
        )
    }
}

/// Planner-trace stage tag (LXE-07 §3.2).
///
/// Each variant identifies one planner step. Extending this enum is the
/// canonical way to model new planner-node kinds; do not regress to
/// stringly-typed dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannerStage {
    Parse,
    Normalize,
    Plan,
    ExecFanout,
    Merge,
    Rerank,
    Bridge,
    /// Filter sub-expression evaluated at planning time (e.g. `repo:`,
    /// `lang:`).
    Filter,
    /// Leaf node executing a regex match against the index.
    LeafRegex,
    /// Leaf node executing a phrase / positional match against the index.
    LeafPhrase,
}

impl PlannerStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Normalize => "normalize",
            Self::Plan => "plan",
            Self::ExecFanout => "exec.fanout",
            Self::Merge => "merge",
            Self::Rerank => "rerank",
            Self::Bridge => "bridge",
            Self::Filter => "filter",
            Self::LeafRegex => "leaf.regex",
            Self::LeafPhrase => "leaf.phrase",
        }
    }
}

impl Serialize for PlannerStage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct PlannerStageVisitor;

impl Visitor<'_> for PlannerStageVisitor {
    type Value = PlannerStage;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PlannerStage string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "parse" => Ok(PlannerStage::Parse),
            "normalize" => Ok(PlannerStage::Normalize),
            "plan" => Ok(PlannerStage::Plan),
            "exec.fanout" => Ok(PlannerStage::ExecFanout),
            "merge" => Ok(PlannerStage::Merge),
            "rerank" => Ok(PlannerStage::Rerank),
            "bridge" => Ok(PlannerStage::Bridge),
            "filter" => Ok(PlannerStage::Filter),
            "leaf.regex" => Ok(PlannerStage::LeafRegex),
            "leaf.phrase" => Ok(PlannerStage::LeafPhrase),
            other => Err(de::Error::unknown_variant(
                other,
                &[
                    "parse",
                    "normalize",
                    "plan",
                    "exec.fanout",
                    "merge",
                    "rerank",
                    "bridge",
                    "filter",
                    "leaf.regex",
                    "leaf.phrase",
                ],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for PlannerStage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(PlannerStageVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannerTraceEntry {
    pub stage: PlannerStage,
    pub detail: String,
}

const PLANNER_TRACE_ENTRY_FIELDS: &[&str] = &["stage", "detail"];

impl Serialize for PlannerTraceEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PlannerTraceEntry", 2)?;
        state.serialize_field("stage", &self.stage)?;
        state.serialize_field("detail", &self.detail)?;
        state.end()
    }
}

struct PlannerTraceEntryVisitor;

impl<'de> Visitor<'de> for PlannerTraceEntryVisitor {
    type Value = PlannerTraceEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PlannerTraceEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut stage: Option<PlannerStage> = None;
        let mut detail: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "stage" => {
                    if stage.is_some() {
                        return Err(de::Error::duplicate_field("stage"));
                    }
                    stage = Some(map.next_value()?);
                }
                "detail" => {
                    if detail.is_some() {
                        return Err(de::Error::duplicate_field("detail"));
                    }
                    detail = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, PLANNER_TRACE_ENTRY_FIELDS)),
            }
        }
        Ok(PlannerTraceEntry {
            stage: stage.ok_or_else(|| de::Error::missing_field("stage"))?,
            detail: detail.ok_or_else(|| de::Error::missing_field("detail"))?,
        })
    }
}

impl<'de> Deserialize<'de> for PlannerTraceEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PlannerTraceEntry",
            PLANNER_TRACE_ENTRY_FIELDS,
            PlannerTraceEntryVisitor,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineTouched {
    Lexical,
    Semantic,
    Structural,
    History,
    Bridge,
}

impl EngineTouched {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
            Self::Structural => "structural",
            Self::History => "history",
            Self::Bridge => "bridge",
        }
    }
}

impl Serialize for EngineTouched {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct EngineTouchedVisitor;

impl Visitor<'_> for EngineTouchedVisitor {
    type Value = EngineTouched;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EngineTouched string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "lexical" => Ok(EngineTouched::Lexical),
            "semantic" => Ok(EngineTouched::Semantic),
            "structural" => Ok(EngineTouched::Structural),
            "history" => Ok(EngineTouched::History),
            "bridge" => Ok(EngineTouched::Bridge),
            other => Err(de::Error::unknown_variant(
                other,
                &["lexical", "semantic", "structural", "history", "bridge"],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for EngineTouched {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(EngineTouchedVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EarlyStopReason {
    CountReached,
    NotReady,
    Unsupported,
}

impl EarlyStopReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CountReached => "count_reached",
            Self::NotReady => "not_ready",
            Self::Unsupported => "unsupported",
        }
    }
}

impl Serialize for EarlyStopReason {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct EarlyStopReasonVisitor;

impl Visitor<'_> for EarlyStopReasonVisitor {
    type Value = EarlyStopReason;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EarlyStopReason string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "count_reached" => Ok(EarlyStopReason::CountReached),
            "not_ready" => Ok(EarlyStopReason::NotReady),
            "unsupported" => Ok(EarlyStopReason::Unsupported),
            other => Err(de::Error::unknown_variant(
                other,
                &["count_reached", "not_ready", "unsupported"],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for EarlyStopReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(EarlyStopReasonVisitor)
    }
}

/// Error returned by ranker-weights digest producers.
///
/// Whether an explained candidate exists in the generation's lexical index
/// (QI-BB-022).
///
/// Decided by an exact lookup of the candidate id, never by re-running a
/// ranked query, so the answer does not depend on how many other documents
/// outrank the candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidatePresenceV1 {
    Indexed,
    NotIndexed,
}

impl CandidatePresenceV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::NotIndexed => "not_indexed",
        }
    }
}

impl Serialize for CandidatePresenceV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct CandidatePresenceVisitor;

impl Visitor<'_> for CandidatePresenceVisitor {
    type Value = CandidatePresenceV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a CandidatePresenceV1 string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "indexed" => Ok(CandidatePresenceV1::Indexed),
            "not_indexed" => Ok(CandidatePresenceV1::NotIndexed),
            other => Err(de::Error::unknown_variant(
                other,
                &["indexed", "not_indexed"],
            )),
        }
    }
}

impl<'de> Deserialize<'de> for CandidatePresenceV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(CandidatePresenceVisitor)
    }
}

/// Digest production is fallible.
///
/// Callers must pass a `Result<[u8; 32], WeightsHashError>` into
/// [`SearchExplanationBuilder::ranker_weights_hash`].
///
/// The builder forwards this error instead of allowing an all-zero fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WeightsHashError {
    message: String,
}

impl WeightsHashError {
    /// Construct a new `WeightsHashError` with the supplied diagnostic.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Borrow the diagnostic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WeightsHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ranker weights hash: ")?;
        f.write_str(&self.message)
    }
}

impl std::error::Error for WeightsHashError {}

/// Ranker explanation surface (LEX-06 §11, LXE-07 §3.2).
///
/// `ranker_weights_hash` pins the weights vector that produced the
/// contributions; downstream auditors compare it against the active weights
/// blob hash to detect drift. Producers should populate this hash from a
/// fallible digest helper (see [`WeightsHashError`] and
/// [`SearchExplanationBuilder::ranker_weights_hash`]).
///
/// `planner_trace`, `engines_touched`, `early_stop_reason`, and `summary`
/// carry hybrid/semantic-planner provenance. Use [`SearchExplanation::empty`]
/// for incremental population by producers, or [`SearchExplanationBuilder`]
/// when the build site wants typed push helpers.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchExplanation {
    pub planner_trace: Vec<PlannerTraceEntry>,
    pub engines_touched: Vec<EngineTouched>,
    pub early_stop_reason: Option<EarlyStopReason>,
    pub contributions: Vec<ExplanationRow>,
    pub ranker_weights_hash: [u8; 32],
    pub strategy: String,
    pub summary: String,
}

impl SearchExplanation {
    /// Returns an empty explanation: no trace, no engines, no stop reason,
    /// no contributions, zero weights-hash, empty strategy + summary.
    /// Producers populate fields incrementally as the query executes.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            planner_trace: Vec::new(),
            engines_touched: Vec::new(),
            early_stop_reason: None,
            contributions: Vec::new(),
            ranker_weights_hash: [0u8; 32],
            strategy: String::new(),
            summary: String::new(),
        }
    }
}

impl Default for SearchExplanation {
    fn default() -> Self {
        Self::empty()
    }
}

const SEARCH_EXPLANATION_FIELDS: &[&str] = &[
    "planner_trace",
    "engines_touched",
    "early_stop_reason",
    "contributions",
    "ranker_weights_hash",
    "strategy",
    "summary",
];

impl Serialize for SearchExplanation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 6;
        if self.early_stop_reason.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("SearchExplanation", field_count)?;
        state.serialize_field("planner_trace", &self.planner_trace)?;
        state.serialize_field("engines_touched", &self.engines_touched)?;
        if let Some(early_stop_reason) = &self.early_stop_reason {
            state.serialize_field("early_stop_reason", early_stop_reason)?;
        }
        state.serialize_field("contributions", &self.contributions)?;
        state.serialize_field("ranker_weights_hash", &self.ranker_weights_hash)?;
        state.serialize_field("strategy", &self.strategy)?;
        state.serialize_field("summary", &self.summary)?;
        state.end()
    }
}

struct SearchExplanationVisitor;

impl<'de> Visitor<'de> for SearchExplanationVisitor {
    type Value = SearchExplanation;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchExplanation map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut planner_trace: Option<Vec<PlannerTraceEntry>> = None;
        let mut engines_touched: Option<Vec<EngineTouched>> = None;
        let mut early_stop_reason: Option<Option<EarlyStopReason>> = None;
        let mut contributions: Option<Vec<ExplanationRow>> = None;
        let mut ranker_weights_hash: Option<[u8; 32]> = None;
        let mut strategy: Option<String> = None;
        let mut summary: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "planner_trace" => {
                    if planner_trace.is_some() {
                        return Err(de::Error::duplicate_field("planner_trace"));
                    }
                    planner_trace = Some(map.next_value()?);
                }
                "engines_touched" => {
                    if engines_touched.is_some() {
                        return Err(de::Error::duplicate_field("engines_touched"));
                    }
                    engines_touched = Some(map.next_value()?);
                }
                "early_stop_reason" => {
                    if early_stop_reason.is_some() {
                        return Err(de::Error::duplicate_field("early_stop_reason"));
                    }
                    early_stop_reason = Some(Some(map.next_value()?));
                }
                "contributions" => {
                    if contributions.is_some() {
                        return Err(de::Error::duplicate_field("contributions"));
                    }
                    contributions = Some(map.next_value()?);
                }
                "ranker_weights_hash" => {
                    if ranker_weights_hash.is_some() {
                        return Err(de::Error::duplicate_field("ranker_weights_hash"));
                    }
                    ranker_weights_hash = Some(map.next_value()?);
                }
                "strategy" => {
                    if strategy.is_some() {
                        return Err(de::Error::duplicate_field("strategy"));
                    }
                    strategy = Some(map.next_value()?);
                }
                "summary" => {
                    if summary.is_some() {
                        return Err(de::Error::duplicate_field("summary"));
                    }
                    summary = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, SEARCH_EXPLANATION_FIELDS)),
            }
        }
        Ok(SearchExplanation {
            planner_trace: planner_trace
                .ok_or_else(|| de::Error::missing_field("planner_trace"))?,
            engines_touched: engines_touched
                .ok_or_else(|| de::Error::missing_field("engines_touched"))?,
            early_stop_reason: early_stop_reason.unwrap_or(None),
            contributions: contributions
                .ok_or_else(|| de::Error::missing_field("contributions"))?,
            ranker_weights_hash: ranker_weights_hash
                .ok_or_else(|| de::Error::missing_field("ranker_weights_hash"))?,
            strategy: strategy.ok_or_else(|| de::Error::missing_field("strategy"))?,
            summary: summary.ok_or_else(|| de::Error::missing_field("summary"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchExplanation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchExplanation",
            SEARCH_EXPLANATION_FIELDS,
            SearchExplanationVisitor,
        )
    }
}

/// Builder for incrementally populating a [`SearchExplanation`].
///
/// The builder is intentionally minimal: push planner-trace entries, push
/// engine identifiers, set early-stop reason / summary, and finalize via
/// [`SearchExplanationBuilder::build`]. Use this from query pipelines that
/// emit provenance step-by-step; one-shot constructions should set fields
/// on a literal `SearchExplanation` directly.
#[derive(Clone, Debug, Default)]
pub struct SearchExplanationBuilder {
    inner: SearchExplanation,
}

impl SearchExplanationBuilder {
    /// Starts with an empty explanation; see [`SearchExplanation::empty`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: SearchExplanation::empty(),
        }
    }

    /// Sets contributions (overwrite). Contributions arrive as a full vector
    /// from the ranker; the builder does not synthesize them.
    #[must_use]
    pub fn contributions(mut self, rows: Vec<ExplanationRow>) -> Self {
        self.inner.contributions = rows;
        self
    }

    /// Sets the ranker-weights digest.
    ///
    /// The argument is `Result<[u8; 32], WeightsHashError>` rather than a
    /// raw `[u8; 32]`: per CLAUDE.md, every public digest function returning
    /// `[u8; N]` for `N ∈ {16, 20, 32, 48, 64}` must be fallible at the
    /// boundary so callers cannot silently substitute an all-zero
    /// placeholder when the underlying codec step fails.
    ///
    /// # Errors
    ///
    /// Forwards the producer's `WeightsHashError` unchanged.
    pub fn ranker_weights_hash(
        mut self,
        hash: Result<[u8; 32], WeightsHashError>,
    ) -> Result<Self, WeightsHashError> {
        self.inner.ranker_weights_hash = hash?;
        Ok(self)
    }

    #[must_use]
    pub fn strategy(mut self, strategy: String) -> Self {
        self.inner.strategy = strategy;
        self
    }

    /// Appends one planner-trace entry.
    #[must_use]
    pub fn push_trace(mut self, entry: PlannerTraceEntry) -> Self {
        self.inner.planner_trace.push(entry);
        self
    }

    /// Appends one engine identifier (e.g. [`EngineTouched::Lexical`],
    /// [`EngineTouched::Semantic`]).
    #[must_use]
    pub fn push_engine(mut self, engine: EngineTouched) -> Self {
        self.inner.engines_touched.push(engine);
        self
    }

    #[must_use]
    pub fn early_stop_reason(mut self, reason: Option<EarlyStopReason>) -> Self {
        self.inner.early_stop_reason = reason;
        self
    }

    #[must_use]
    pub fn summary(mut self, summary: String) -> Self {
        self.inner.summary = summary;
        self
    }

    #[must_use]
    pub fn build(self) -> SearchExplanation {
        self.inner
    }
}
