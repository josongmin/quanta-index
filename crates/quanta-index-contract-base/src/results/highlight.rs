use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

/// A byte range within an emitted snippet that a UI should highlight.
///
/// `start` is the byte offset of a matched hit within `LexicalCandidate::snippet`
/// and `len` its byte length; both are UTF-8 char-boundary aligned. Spans are
/// typed so a consumer renders highlights without regex-parsing the snippet text
/// (J7Q-07).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HighlightSpan {
    pub start: u32,
    pub len: u32,
}

const HIGHLIGHT_SPAN_FIELDS: &[&str] = &["start", "len"];

impl Serialize for HighlightSpan {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HighlightSpan", 2)?;
        state.serialize_field("start", &self.start)?;
        state.serialize_field("len", &self.len)?;
        state.end()
    }
}

struct HighlightSpanVisitor;

impl<'de> Visitor<'de> for HighlightSpanVisitor {
    type Value = HighlightSpan;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HighlightSpan map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut start: Option<u32> = None;
        let mut len: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "start" => {
                    if start.is_some() {
                        return Err(de::Error::duplicate_field("start"));
                    }
                    start = Some(map.next_value()?);
                }
                "len" => {
                    if len.is_some() {
                        return Err(de::Error::duplicate_field("len"));
                    }
                    len = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, HIGHLIGHT_SPAN_FIELDS)),
            }
        }
        let start = start.ok_or_else(|| de::Error::missing_field("start"))?;
        let len = len.ok_or_else(|| de::Error::missing_field("len"))?;
        Ok(HighlightSpan { start, len })
    }
}

impl<'de> Deserialize<'de> for HighlightSpan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HighlightSpan",
            HIGHLIGHT_SPAN_FIELDS,
            HighlightSpanVisitor,
        )
    }
}
