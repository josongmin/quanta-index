use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

/// Repair class for a typed query failure (J7Q-06).
///
/// Keeps the distinct failure families the bridge / lexical layers already
/// separate from collapsing into one generic "error" at the wire: a UI or
/// operator can branch on the class without string-matching the code. The class
/// is advisory repair metadata only — it never changes the fail-closed outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RepairClass {
    /// The query matched more than one supported target and was refused
    /// fail-closed; the caller must disambiguate.
    Ambiguous,
    /// The construct is not supported on this surface; the caller should switch
    /// to a supported shape.
    Unsupported,
    /// The query was sent to the wrong route family for its intent.
    WrongRoute,
    /// The query shape itself is malformed (e.g. a bad version pin).
    Malformed,
}

impl RepairClass {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Ambiguous => "AMBIGUOUS",
            Self::Unsupported => "UNSUPPORTED",
            Self::WrongRoute => "WRONG_ROUTE",
            Self::Malformed => "MALFORMED",
        }
    }

    /// Inverse of [`RepairClass::as_code_str`]; `None` on unknown input.
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "AMBIGUOUS" => Self::Ambiguous,
            "UNSUPPORTED" => Self::Unsupported,
            "WRONG_ROUTE" => Self::WrongRoute,
            "MALFORMED" => Self::Malformed,
            _ => return None,
        };
        Some(v)
    }
}

impl Serialize for RepairClass {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for RepairClass {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RepairClassVisitor;
        impl Visitor<'_> for RepairClassVisitor {
            type Value = RepairClass;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("RepairClass SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E>(self, value: &str) -> Result<RepairClass, E>
            where
                E: de::Error,
            {
                RepairClass::from_code_str(value).ok_or_else(|| {
                    de::Error::unknown_variant(
                        value,
                        &["AMBIGUOUS", "UNSUPPORTED", "WRONG_ROUTE", "MALFORMED"],
                    )
                })
            }
        }
        deserializer.deserialize_str(RepairClassVisitor)
    }
}

/// Typed repair metadata attached to a query failure (J7Q-06).
///
/// Carries the failure [`RepairClass`], the supported alternative shapes /
/// example queries the caller can switch to, and an optional docs anchor — all
/// as typed fields so CLI and SDK consumers render the same guidance from one
/// payload instead of re-deriving hints from prose. This is additive, advisory
/// metadata: it never rewrites the query and never softens the fail-closed code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryErrorRepair {
    pub class: RepairClass,
    pub supported_alternatives: Vec<String>,
    pub docs_anchor: Option<String>,
}

const QUERY_ERROR_REPAIR_FIELDS: &[&str] = &["class", "supported_alternatives", "docs_anchor"];

impl Serialize for QueryErrorRepair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 2usize;
        if self.docs_anchor.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("QueryErrorRepair", field_count)?;
        state.serialize_field("class", &self.class)?;
        state.serialize_field("supported_alternatives", &self.supported_alternatives)?;
        if let Some(anchor) = &self.docs_anchor {
            state.serialize_field("docs_anchor", anchor)?;
        }
        state.end()
    }
}

struct QueryErrorRepairVisitor;

impl<'de> Visitor<'de> for QueryErrorRepairVisitor {
    type Value = QueryErrorRepair;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a QueryErrorRepair map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut class: Option<RepairClass> = None;
        let mut supported_alternatives: Option<Vec<String>> = None;
        let mut docs_anchor: Option<String> = None;
        let mut docs_anchor_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "class" => {
                    if class.is_some() {
                        return Err(de::Error::duplicate_field("class"));
                    }
                    class = Some(map.next_value()?);
                }
                "supported_alternatives" => {
                    if supported_alternatives.is_some() {
                        return Err(de::Error::duplicate_field("supported_alternatives"));
                    }
                    supported_alternatives = Some(map.next_value()?);
                }
                "docs_anchor" => {
                    if docs_anchor_seen {
                        return Err(de::Error::duplicate_field("docs_anchor"));
                    }
                    docs_anchor_seen = true;
                    docs_anchor = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, QUERY_ERROR_REPAIR_FIELDS));
                }
            }
        }
        Ok(QueryErrorRepair {
            class: class.ok_or_else(|| de::Error::missing_field("class"))?,
            supported_alternatives: supported_alternatives
                .ok_or_else(|| de::Error::missing_field("supported_alternatives"))?,
            docs_anchor,
        })
    }
}

impl<'de> Deserialize<'de> for QueryErrorRepair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "QueryErrorRepair",
            QUERY_ERROR_REPAIR_FIELDS,
            QueryErrorRepairVisitor,
        )
    }
}

/// Wire code for a response the server computed but cannot put on the wire
/// because its encoded body exceeds the frame limit (QI-BB-005).
///
/// Before this code the connection was simply closed after the work was
/// done, so the caller could not tell an oversized answer from a crash. The
/// typed refusal names both sizes so the caller can narrow `top_k` or the
/// projection.
pub const ERR_RESULT_TOO_LARGE: &str = "RESULT_TOO_LARGE";

/// Wire-level typed error carried in every search-plane IPC response.
///
/// `code` + `message` are the load-bearing fail-closed fields; `repair` is
/// optional, additive typed guidance (J7Q-06). Old peers that never wrote
/// `repair` still decode (the field reads as `None`); unknown fields remain
/// rejected so the decoder stays fail-closed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneIpcError {
    pub code: String,
    pub message: String,
    pub repair: Option<QueryErrorRepair>,
}

impl SearchPlaneIpcError {
    /// The refusal a transport sends in place of a response whose encoded
    /// body of `encoded_bytes` exceeds `limit_bytes`.
    #[must_use]
    pub fn result_too_large(encoded_bytes: u64, limit_bytes: u64) -> Self {
        Self {
            code: ERR_RESULT_TOO_LARGE.to_string(),
            message: format!(
                "response body of {encoded_bytes} bytes exceeds the {limit_bytes}-byte frame limit; narrow top_k or the projection"
            ),
            repair: None,
        }
    }
}

const SEARCH_PLANE_IPC_ERROR_FIELDS: &[&str] = &["code", "message", "repair"];

impl Serialize for SearchPlaneIpcError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 2usize;
        if self.repair.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("SearchPlaneIpcError", field_count)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("message", &self.message)?;
        if let Some(repair) = &self.repair {
            state.serialize_field("repair", repair)?;
        }
        state.end()
    }
}

struct SearchPlaneIpcErrorVisitor;

impl<'de> Visitor<'de> for SearchPlaneIpcErrorVisitor {
    type Value = SearchPlaneIpcError;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneIpcError map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut code: Option<String> = None;
        let mut message: Option<String> = None;
        let mut repair: Option<QueryErrorRepair> = None;
        let mut repair_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "code" => {
                    if code.is_some() {
                        return Err(de::Error::duplicate_field("code"));
                    }
                    code = Some(map.next_value()?);
                }
                "message" => {
                    if message.is_some() {
                        return Err(de::Error::duplicate_field("message"));
                    }
                    message = Some(map.next_value()?);
                }
                "repair" => {
                    if repair_seen {
                        return Err(de::Error::duplicate_field("repair"));
                    }
                    repair_seen = true;
                    repair = map.next_value()?;
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_IPC_ERROR_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneIpcError {
            code: code.ok_or_else(|| de::Error::missing_field("code"))?,
            message: message.ok_or_else(|| de::Error::missing_field("message"))?,
            repair,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneIpcError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneIpcError",
            SEARCH_PLANE_IPC_ERROR_FIELDS,
            SearchPlaneIpcErrorVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{QueryErrorRepair, RepairClass, SearchPlaneIpcError};

    fn cbor_roundtrip_error(value: &SearchPlaneIpcError) -> SearchPlaneIpcError {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(value, &mut buf).expect("serialize");
        ciborium::de::from_reader(buf.as_slice()).expect("deserialize")
    }

    #[test]
    fn repair_class_roundtrip_all_variants() {
        for class in [
            RepairClass::Ambiguous,
            RepairClass::Unsupported,
            RepairClass::WrongRoute,
            RepairClass::Malformed,
        ] {
            assert_eq!(RepairClass::from_code_str(class.as_code_str()), Some(class));
        }
        assert_eq!(RepairClass::from_code_str("NOPE"), None);
    }

    #[test]
    fn error_without_repair_roundtrips() {
        let err = SearchPlaneIpcError {
            code: "BRIDGE_TRANSLATE_FAIL".to_string(),
            message: "boom".to_string(),
            repair: None,
        };
        assert_eq!(cbor_roundtrip_error(&err), err);
    }

    #[test]
    fn error_with_repair_roundtrips() {
        let err = SearchPlaneIpcError {
            code: "BRIDGE_AMBIGUOUS_FILTER".to_string(),
            message: "filter resolves to 2 targets".to_string(),
            repair: Some(QueryErrorRepair {
                class: RepairClass::Ambiguous,
                supported_alternatives: vec!["repo:".to_string(), "file:".to_string()],
                docs_anchor: Some("docs/query#ambiguous".to_string()),
            }),
        };
        assert_eq!(cbor_roundtrip_error(&err), err);
    }

    #[test]
    fn repair_without_docs_anchor_roundtrips() {
        let err = SearchPlaneIpcError {
            code: "BRIDGE_UNSUPPORTED_FILTER".to_string(),
            message: "no projection".to_string(),
            repair: Some(QueryErrorRepair {
                class: RepairClass::Unsupported,
                supported_alternatives: vec!["content:".to_string()],
                docs_anchor: None,
            }),
        };
        assert_eq!(cbor_roundtrip_error(&err), err);
    }

    #[test]
    fn legacy_two_field_wire_decodes_with_none_repair() {
        // A peer that predates J7Q-06 writes only {code, message}; it must still
        // decode, with `repair` reading as None (back-compat, fail-closed). We
        // hand-roll the two-field CBOR map via a BTreeMap to mimic the old wire.
        let map: std::collections::BTreeMap<String, String> = [
            ("code".to_string(), "NOT_READY".to_string()),
            ("message".to_string(), "warming".to_string()),
        ]
        .into_iter()
        .collect();
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(&map, &mut buf).expect("serialize legacy");
        let decoded: SearchPlaneIpcError =
            ciborium::de::from_reader(buf.as_slice()).expect("decode legacy");
        assert_eq!(decoded.code, "NOT_READY");
        assert_eq!(decoded.message, "warming");
        assert_eq!(decoded.repair, None);
    }
}
