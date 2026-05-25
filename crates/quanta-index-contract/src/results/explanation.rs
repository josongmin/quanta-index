use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::ExplanationRow;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannerStage {
    Parse,
    Normalize,
    Plan,
    ExecFanout,
    Merge,
    Rerank,
    Bridge,
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
                "stage" => stage = Some(map.next_value()?),
                "detail" => detail = Some(map.next_value()?),
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
                "planner_trace" => planner_trace = Some(map.next_value()?),
                "engines_touched" => engines_touched = Some(map.next_value()?),
                "early_stop_reason" => early_stop_reason = Some(Some(map.next_value()?)),
                "contributions" => contributions = Some(map.next_value()?),
                "ranker_weights_hash" => ranker_weights_hash = Some(map.next_value()?),
                "strategy" => strategy = Some(map.next_value()?),
                "summary" => summary = Some(map.next_value()?),
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
