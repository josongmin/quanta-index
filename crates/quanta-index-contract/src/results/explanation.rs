use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchExplanation {
    pub summary: String,
}

const SEARCH_EXPLANATION_FIELDS: &[&str] = &["summary"];

impl Serialize for SearchExplanation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchExplanation", 1)?;
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
        let mut summary: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "summary" => {
                    if summary.is_some() {
                        return Err(de::Error::duplicate_field("summary"));
                    }
                    summary = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, SEARCH_EXPLANATION_FIELDS)),
            }
        }
        let summary = summary.ok_or_else(|| de::Error::missing_field("summary"))?;
        Ok(SearchExplanation { summary })
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
