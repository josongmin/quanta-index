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
    pub const fn returned(self) -> u32 {
        self.returned
    }

    #[must_use]
    pub const fn candidate_count(self) -> CandidateCountV1 {
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
                    return Err(de::Error::unknown_field(other, QUERY_RESULT_WINDOW_V1_FIELDS));
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
}
