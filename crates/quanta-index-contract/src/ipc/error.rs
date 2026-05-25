use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneIpcError {
    pub code: String,
    pub message: String,
}

const SEARCH_PLANE_IPC_ERROR_FIELDS: &[&str] = &["code", "message"];

impl Serialize for SearchPlaneIpcError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneIpcError", 2)?;
        state.serialize_field("code", &self.code)?;
        state.serialize_field("message", &self.message)?;
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
