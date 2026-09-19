//! Wire-leaf query request shapes that depend only on core base types
//! (`TextQuerySyntax`, `GenerationPin`, `GenerationSelector`).
//!
//! Heavier request variants (semantic, hybrid, etc.) stay in
//! `quanta-index-contract`.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use super::{
    GenerationPin, GenerationSelector, LexicalCursor, QueryConstraintSetV1, TextQuerySyntax,
    validate_public_top_k,
};

/// The `top_k` gate every wire request shape applies on encode (QI-BB-025).
///
/// A typed client cannot emit a request whose `top_k` is outside the public
/// range: the encoder refuses under the shared [`TopKOutOfRangeV1`] code
/// before any round trip. The decoder deliberately does not refuse the
/// value: a raw caller's out-of-range request must come back as a typed
/// answer carrying that same code (the dispatcher's
/// [`validate_public_top_k`]), not as a closed connection that names no
/// code. The SDK, the encoder and the dispatcher run the one validator, so
/// they cannot drift on the range or on the code.
///
/// [`TopKOutOfRangeV1`]: super::TopKOutOfRangeV1
pub fn wire_top_k<E>(top_k: u32, custom: impl FnOnce(String) -> E) -> Result<u32, E> {
    validate_public_top_k(top_k).map_err(|refused| custom(format!("{}: {refused}", refused.code())))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextQueryRequest {
    pub syntax: TextQuerySyntax,
    pub query_text: String,
    /// Candidate-generation constraints. This field is mandatory on the wire;
    /// an empty set explicitly means unconstrained.
    pub constraints: QueryConstraintSetV1,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    /// QI-QRY-01: required result cap. Wire field is mandatory; missing
    /// `top_k` fails-closed at deserialization via `missing_field`. A value
    /// outside the public range does not encode ([`wire_top_k`]) and, when
    /// a raw caller sends one anyway, is refused typed by the dispatcher
    /// under the same code (QI-BB-025). No caller-side default — the SDK
    /// builder enforces this is set.
    pub top_k: u32,
    /// Continue after this row of an earlier page (QI-BB-005 보완 #4): the
    /// page holds the rows strictly after it in the ranked order, in the
    /// generation it names. Absent on the wire for a first page.
    pub cursor: Option<LexicalCursor>,
}

const TEXT_QUERY_REQUEST_FIELDS: &[&str] = &[
    "syntax",
    "query_text",
    "constraints",
    "generation",
    "generation_selector",
    "top_k",
    "cursor",
];

impl Serialize for TextQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 4;
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.generation_selector.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.cursor.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let top_k = wire_top_k(self.top_k, serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("TextQueryRequest", field_count)?;
        state.serialize_field("syntax", &self.syntax)?;
        state.serialize_field("query_text", &self.query_text)?;
        state.serialize_field("constraints", &self.constraints)?;
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        if let Some(generation_selector) = &self.generation_selector {
            state.serialize_field("generation_selector", generation_selector)?;
        }
        state.serialize_field("top_k", &top_k)?;
        if let Some(cursor) = &self.cursor {
            state.serialize_field("cursor", cursor)?;
        }
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
        let mut constraints: Option<QueryConstraintSetV1> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        let mut generation_selector: Option<GenerationSelector> = None;
        let mut generation_selector_seen = false;
        let mut top_k: Option<u32> = None;
        let mut cursor: Option<LexicalCursor> = None;
        let mut cursor_seen = false;
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
                "constraints" => {
                    if constraints.is_some() {
                        return Err(de::Error::duplicate_field("constraints"));
                    }
                    constraints = Some(map.next_value()?);
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
                "cursor" => {
                    if cursor_seen {
                        return Err(de::Error::duplicate_field("cursor"));
                    }
                    cursor_seen = true;
                    cursor = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, TEXT_QUERY_REQUEST_FIELDS));
                }
            }
        }
        Ok(TextQueryRequest {
            syntax: syntax.ok_or_else(|| de::Error::missing_field("syntax"))?,
            query_text: query_text.ok_or_else(|| de::Error::missing_field("query_text"))?,
            constraints: constraints.ok_or_else(|| de::Error::missing_field("constraints"))?,
            generation,
            generation_selector,
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
            cursor,
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

#[cfg(test)]
mod tests {
    //! The wire `top_k` gate (QI-BB-025): a request outside the public range
    //! neither encodes nor decodes, under the shared code.

    use super::{TextQueryRequest, wire_top_k};
    use crate::query::{
        PUBLIC_TOP_K_MAX, QueryConstraintSetV1, TOP_K_OUT_OF_RANGE_CODE, TextQuerySyntax,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn request(top_k: u32) -> TextQueryRequest {
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k,
            cursor: None,
        }
    }

    /// The JSON of an in-range request with its `top_k` replaced by `top_k`,
    /// so the decoder sees bytes no encoder would have produced.
    fn raw_json_with_top_k(top_k: u32) -> Result<String, Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(request(1))?;
        let object = value
            .as_object_mut()
            .ok_or("an encoded request is a JSON object")?;
        let _previous = object.insert("top_k".to_string(), serde_json::json!(top_k));
        Ok(serde_json::to_string(&value)?)
    }

    #[test]
    fn in_range_top_k_round_trips_including_the_public_maximum() -> TestResult {
        for top_k in [1, 2, PUBLIC_TOP_K_MAX - 1, PUBLIC_TOP_K_MAX] {
            let encoded = serde_json::to_string(&request(top_k))?;
            let decoded: TextQueryRequest = serde_json::from_str(&encoded)?;
            if decoded != request(top_k) {
                return Err(format!("top_k={top_k} did not round-trip: {decoded:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn out_of_range_top_k_is_refused_on_encode_with_the_shared_code() -> TestResult {
        for top_k in [0, PUBLIC_TOP_K_MAX + 1, u32::MAX] {
            let Err(error) = serde_json::to_string(&request(top_k)) else {
                return Err(format!("top_k={top_k} must not encode").into());
            };
            let message = error.to_string();
            if !message.contains(TOP_K_OUT_OF_RANGE_CODE) {
                return Err(format!("top_k={top_k}: {message}").into());
            }
        }
        Ok(())
    }

    /// Decode does not gate the range: the value reaches the dispatcher,
    /// which answers the raw caller typed under the same code the encoder
    /// and the SDK name.
    #[test]
    fn out_of_range_top_k_decodes_so_the_dispatcher_can_answer_it_typed() -> TestResult {
        for top_k in [0, PUBLIC_TOP_K_MAX + 1, u32::MAX] {
            let raw = raw_json_with_top_k(top_k)?;
            let decoded = serde_json::from_str::<TextQueryRequest>(&raw)?;
            if decoded.top_k != top_k {
                return Err(format!("top_k={top_k} decoded as {}", decoded.top_k).into());
            }
        }
        Ok(())
    }

    #[test]
    fn the_gate_names_the_code_and_the_value() {
        assert_eq!(
            wire_top_k(0, |message| message),
            Err(format!(
                "{TOP_K_OUT_OF_RANGE_CODE}: top_k must be within 1..={PUBLIC_TOP_K_MAX}, got 0"
            ))
        );
        assert_eq!(wire_top_k(7, |message| message), Ok(7));
    }
}
