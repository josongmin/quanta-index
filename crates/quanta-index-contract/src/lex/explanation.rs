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

const EXPLANATION_ROW_FIELDS: &[&str] =
    &["signal_name", "signal_value", "weight", "contribution"];

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
        let signal_name =
            signal_name.ok_or_else(|| de::Error::missing_field("signal_name"))?;
        let signal_value =
            signal_value.ok_or_else(|| de::Error::missing_field("signal_value"))?;
        let weight = weight.ok_or_else(|| de::Error::missing_field("weight"))?;
        let contribution =
            contribution.ok_or_else(|| de::Error::missing_field("contribution"))?;
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

/// Ranker explanation surface per LEX-06 §11 (GAP-05 closure).
///
/// `ranker_weights_hash` pins the weights vector that produced these
/// contributions; downstream auditors compare it against the active weights
/// blob hash to detect drift.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchExplanation {
    pub contributions: Vec<ExplanationRow>,
    pub ranker_weights_hash: [u8; 32],
    pub strategy: Box<str>,
}

const SEARCH_EXPLANATION_FIELDS: &[&str] =
    &["contributions", "ranker_weights_hash", "strategy"];

impl Serialize for SearchExplanation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchExplanation", 3)?;
        state.serialize_field("contributions", &self.contributions)?;
        state.serialize_field("ranker_weights_hash", &self.ranker_weights_hash)?;
        state.serialize_field("strategy", self.strategy.as_ref())?;
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
                other => {
                    return Err(de::Error::unknown_field(other, SEARCH_EXPLANATION_FIELDS));
                }
            }
        }
        let contributions =
            contributions.ok_or_else(|| de::Error::missing_field("contributions"))?;
        let ranker_weights_hash = ranker_weights_hash
            .ok_or_else(|| de::Error::missing_field("ranker_weights_hash"))?;
        let strategy = strategy.ok_or_else(|| de::Error::missing_field("strategy"))?;
        Ok(SearchExplanation {
            contributions,
            ranker_weights_hash,
            strategy: strategy.into_boxed_str(),
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
