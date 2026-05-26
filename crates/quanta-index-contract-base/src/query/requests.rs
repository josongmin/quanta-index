//! Wire-leaf query request shapes that depend only on core base types
//! (`TextQuerySyntax`, `GenerationPin`, `GenerationSelector`).
//!
//! Heavier request variants (semantic, hybrid, etc.) that still depend on
//! higher-level contract surfaces such as `BridgeTarget` stay in
//! `quanta-index-contract`.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use super::{GenerationPin, GenerationSelector, TextQuerySyntax};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextQueryRequest {
    pub syntax: TextQuerySyntax,
    pub query_text: String,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    /// QI-QRY-01: required result cap. Wire field is mandatory; missing
    /// `top_k` fails-closed at deserialization via `missing_field`. No
    /// caller-side default — the SDK builder enforces this is set.
    pub top_k: u32,
}

const TEXT_QUERY_REQUEST_FIELDS: &[&str] = &[
    "syntax",
    "query_text",
    "generation",
    "generation_selector",
    "top_k",
];

impl Serialize for TextQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 3;
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.generation_selector.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("TextQueryRequest", field_count)?;
        state.serialize_field("syntax", &self.syntax)?;
        state.serialize_field("query_text", &self.query_text)?;
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        if let Some(generation_selector) = &self.generation_selector {
            state.serialize_field("generation_selector", generation_selector)?;
        }
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct TextQueryRequestVisitor;

impl<'de> Visitor<'de> for TextQueryRequestVisitor {
    type Value = TextQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a TextQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut syntax: Option<TextQuerySyntax> = None;
        let mut query_text: Option<String> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        let mut generation_selector: Option<GenerationSelector> = None;
        let mut generation_selector_seen = false;
        let mut top_k: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "syntax" => {
                    if syntax.is_some() {
                        return Err(de::Error::duplicate_field("syntax"));
                    }
                    syntax = Some(map.next_value()?);
                }
                "query_text" => {
                    if query_text.is_some() {
                        return Err(de::Error::duplicate_field("query_text"));
                    }
                    query_text = Some(map.next_value()?);
                }
                "generation" => {
                    if generation_seen {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation_seen = true;
                    generation = Some(map.next_value()?);
                }
                "generation_selector" => {
                    if generation_selector_seen {
                        return Err(de::Error::duplicate_field("generation_selector"));
                    }
                    generation_selector_seen = true;
                    generation_selector = Some(map.next_value()?);
                }
                "top_k" => {
                    if top_k.is_some() {
                        return Err(de::Error::duplicate_field("top_k"));
                    }
                    top_k = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, TEXT_QUERY_REQUEST_FIELDS));
                }
            }
        }
        Ok(TextQueryRequest {
            syntax: syntax.ok_or_else(|| de::Error::missing_field("syntax"))?,
            query_text: query_text.ok_or_else(|| de::Error::missing_field("query_text"))?,
            generation,
            generation_selector,
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
        })
    }
}

impl<'de> Deserialize<'de> for TextQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "TextQueryRequest",
            TEXT_QUERY_REQUEST_FIELDS,
            TextQueryRequestVisitor,
        )
    }
}
