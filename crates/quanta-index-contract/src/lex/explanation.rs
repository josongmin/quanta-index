//! `SearchExplanation` (LEX-06 §11 GAP-05 closure) — per-row contribution +
//! weights hash + strategy tag.
//!
//! This is the canonical search-side explanation surface scaffolded for
//! downstream `lq_ranker` consolidation. It is **distinct** from the existing
//! [`crate::results::SearchExplanation`] shape which carries only `summary`;
//! reconciling the two is a downstream ticket. This scaffold lives under
//! [`crate::lex`] and is fully name-qualified to avoid clashing with the
//! existing top-level re-export.

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

/// One planner-trace step (LXE-07 §3.2: planner provenance).
///
/// `node_kind` is a short tag identifying the planner node (e.g. `"filter"`,
/// `"leaf:regex"`, `"leaf:phrase"`); `detail` is a free-form short label that
/// carries node-specific provenance (operator string, regex pattern hash,
/// etc.). Both fields are `Box<str>` to keep `SearchExplanation` cheap to
/// clone while remaining wire-explicit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannerTraceNode {
    pub node_kind: Box<str>,
    pub detail: Box<str>,
}

const PLANNER_TRACE_NODE_FIELDS: &[&str] = &["node_kind", "detail"];

impl Serialize for PlannerTraceNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PlannerTraceNode", 2)?;
        state.serialize_field("node_kind", self.node_kind.as_ref())?;
        state.serialize_field("detail", self.detail.as_ref())?;
        state.end()
    }
}

struct PlannerTraceNodeVisitor;

impl<'de> Visitor<'de> for PlannerTraceNodeVisitor {
    type Value = PlannerTraceNode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PlannerTraceNode map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut node_kind: Option<String> = None;
        let mut detail: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "node_kind" => {
                    if node_kind.is_some() {
                        return Err(de::Error::duplicate_field("node_kind"));
                    }
                    node_kind = Some(map.next_value()?);
                }
                "detail" => {
                    if detail.is_some() {
                        return Err(de::Error::duplicate_field("detail"));
                    }
                    detail = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, PLANNER_TRACE_NODE_FIELDS)),
            }
        }
        let node_kind = node_kind.ok_or_else(|| de::Error::missing_field("node_kind"))?;
        let detail = detail.ok_or_else(|| de::Error::missing_field("detail"))?;
        Ok(PlannerTraceNode {
            node_kind: node_kind.into_boxed_str(),
            detail: detail.into_boxed_str(),
        })
    }
}

impl<'de> Deserialize<'de> for PlannerTraceNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PlannerTraceNode",
            PLANNER_TRACE_NODE_FIELDS,
            PlannerTraceNodeVisitor,
        )
    }
}

/// Ranker explanation surface per LEX-06 §11 (GAP-05 closure) plus LXE-07
/// planner-provenance fields.
///
/// `ranker_weights_hash` pins the weights vector that produced these
/// contributions; downstream auditors compare it against the active weights
/// blob hash to detect drift.
///
/// `planner_trace`, `engines_touched`, `early_stop_reason`, and `summary`
/// were added in LXE-07 to carry hybrid/semantic-planner provenance. Use
/// [`SearchExplanation::empty`] for incremental population by producers, or
/// [`SearchExplanationBuilder`] when the build site wants typed push helpers.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchExplanation {
    pub contributions: Vec<ExplanationRow>,
    pub ranker_weights_hash: [u8; 32],
    pub strategy: Box<str>,
    pub planner_trace: Vec<PlannerTraceNode>,
    pub engines_touched: Vec<Box<str>>,
    pub early_stop_reason: Option<Box<str>>,
    pub summary: Option<Box<str>>,
}

impl SearchExplanation {
    /// Returns an empty explanation: no contributions, zero weights-hash,
    /// empty strategy, no trace, no engines, no stop reason, no summary.
    /// Producers populate fields incrementally as the query executes.
    ///
    /// Not `const fn` because `Box::<str>::from("")` is not yet const-stable.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            contributions: Vec::new(),
            ranker_weights_hash: [0u8; 32],
            strategy: Box::<str>::from(""),
            planner_trace: Vec::new(),
            engines_touched: Vec::new(),
            early_stop_reason: None,
            summary: None,
        }
    }
}

impl Default for SearchExplanation {
    fn default() -> Self {
        Self::empty()
    }
}

const SEARCH_EXPLANATION_FIELDS: &[&str] = &[
    "contributions",
    "ranker_weights_hash",
    "strategy",
    "planner_trace",
    "engines_touched",
    "early_stop_reason",
    "summary",
];

impl Serialize for SearchExplanation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 5;
        if self.early_stop_reason.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.summary.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("SearchExplanation", field_count)?;
        state.serialize_field("contributions", &self.contributions)?;
        state.serialize_field("ranker_weights_hash", &self.ranker_weights_hash)?;
        state.serialize_field("strategy", self.strategy.as_ref())?;
        state.serialize_field("planner_trace", &self.planner_trace)?;
        state.serialize_field("engines_touched", &self.engines_touched)?;
        if let Some(early_stop_reason) = &self.early_stop_reason {
            state.serialize_field("early_stop_reason", early_stop_reason.as_ref())?;
        }
        if let Some(summary) = &self.summary {
            state.serialize_field("summary", summary.as_ref())?;
        }
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
        let mut contributions: Option<Vec<ExplanationRow>> = None;
        let mut ranker_weights_hash: Option<[u8; 32]> = None;
        let mut strategy: Option<String> = None;
        let mut planner_trace: Option<Vec<PlannerTraceNode>> = None;
        let mut engines_touched: Option<Vec<String>> = None;
        let mut early_stop_reason: Option<Option<String>> = None;
        let mut summary: Option<Option<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
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
                "summary" => {
                    if summary.is_some() {
                        return Err(de::Error::duplicate_field("summary"));
                    }
                    summary = Some(Some(map.next_value()?));
                }
                other => {
                    return Err(de::Error::unknown_field(other, SEARCH_EXPLANATION_FIELDS));
                }
            }
        }
        let contributions =
            contributions.ok_or_else(|| de::Error::missing_field("contributions"))?;
        let ranker_weights_hash =
            ranker_weights_hash.ok_or_else(|| de::Error::missing_field("ranker_weights_hash"))?;
        let strategy = strategy.ok_or_else(|| de::Error::missing_field("strategy"))?;
        let planner_trace =
            planner_trace.ok_or_else(|| de::Error::missing_field("planner_trace"))?;
        let engines_touched =
            engines_touched.ok_or_else(|| de::Error::missing_field("engines_touched"))?;
        Ok(SearchExplanation {
            contributions,
            ranker_weights_hash,
            strategy: strategy.into_boxed_str(),
            planner_trace,
            engines_touched: engines_touched
                .into_iter()
                .map(String::into_boxed_str)
                .collect(),
            early_stop_reason: early_stop_reason
                .unwrap_or(None)
                .map(String::into_boxed_str),
            summary: summary.unwrap_or(None).map(String::into_boxed_str),
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
/// The builder is intentionally minimal: push planner-trace nodes, push
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

    /// Sets contributions (overwrite). Builder helpers are intentionally
    /// non-fluent setters rather than push-style — contributions arrive as
    /// a full vector from the ranker; the builder does not synthesize them.
    #[must_use]
    pub fn contributions(mut self, rows: Vec<ExplanationRow>) -> Self {
        self.inner.contributions = rows;
        self
    }

    #[must_use]
    pub fn ranker_weights_hash(mut self, hash: [u8; 32]) -> Self {
        self.inner.ranker_weights_hash = hash;
        self
    }

    #[must_use]
    pub fn strategy(mut self, strategy: Box<str>) -> Self {
        self.inner.strategy = strategy;
        self
    }

    /// Appends one planner-trace node.
    #[must_use]
    pub fn push_trace(mut self, node: PlannerTraceNode) -> Self {
        self.inner.planner_trace.push(node);
        self
    }

    /// Appends one engine identifier (e.g. `"tantivy"`, `"lq-trigram"`).
    #[must_use]
    pub fn push_engine(mut self, engine: Box<str>) -> Self {
        self.inner.engines_touched.push(engine);
        self
    }

    #[must_use]
    pub fn early_stop_reason(mut self, reason: Option<Box<str>>) -> Self {
        self.inner.early_stop_reason = reason;
        self
    }

    #[must_use]
    pub fn summary(mut self, summary: Option<Box<str>>) -> Self {
        self.inner.summary = summary;
        self
    }

    #[must_use]
    pub fn build(self) -> SearchExplanation {
        self.inner
    }
}
