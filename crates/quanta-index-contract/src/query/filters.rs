use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LqFilterSet {
    pub filters: Vec<LqFilter>,
}

const LQ_FILTER_SET_FIELDS: &[&str] = &["filters"];

impl Serialize for LqFilterSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LqFilterSet", 1)?;
        state.serialize_field("filters", &self.filters)?;
        state.end()
    }
}

struct LqFilterSetVisitor;

impl<'de> Visitor<'de> for LqFilterSetVisitor {
    type Value = LqFilterSet;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LqFilterSet map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut filters: Option<Vec<LqFilter>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "filters" => {
                    if filters.is_some() {
                        return Err(de::Error::duplicate_field("filters"));
                    }
                    filters = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, LQ_FILTER_SET_FIELDS)),
            }
        }
        let filters = filters.ok_or_else(|| de::Error::missing_field("filters"))?;
        Ok(LqFilterSet { filters })
    }
}

impl<'de> Deserialize<'de> for LqFilterSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("LqFilterSet", LQ_FILTER_SET_FIELDS, LqFilterSetVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LqFilter {
    Repo(String),
    File(String),
    Path(String),
    Lang(String),
    Rev(String),
    Select(String),
    Type(String),
    Custom { key: String, value: String },
}

impl LqFilter {
    const VARIANTS: &'static [&'static str] = &[
        "Repo", "File", "Path", "Lang", "Rev", "Select", "Type", "Custom",
    ];

    const fn kind(&self) -> &'static str {
        match self {
            Self::Repo(_) => "Repo",
            Self::File(_) => "File",
            Self::Path(_) => "Path",
            Self::Lang(_) => "Lang",
            Self::Rev(_) => "Rev",
            Self::Select(_) => "Select",
            Self::Type(_) => "Type",
            Self::Custom { .. } => "Custom",
        }
    }
}

const LQ_FILTER_FIELDS: &[&str] = &["kind", "payload"];
const LQ_FILTER_CUSTOM_FIELDS: &[&str] = &["key", "value"];

struct CustomFilterPayloadSer<'a> {
    key: &'a str,
    value: &'a str,
}

impl Serialize for CustomFilterPayloadSer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("CustomFilterPayload", 2)?;
        state.serialize_field("key", self.key)?;
        state.serialize_field("value", self.value)?;
        state.end()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CustomFilterPayloadDe {
    key: String,
    value: String,
}

struct CustomFilterPayloadVisitor;

impl<'de> Visitor<'de> for CustomFilterPayloadVisitor {
    type Value = CustomFilterPayloadDe;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a Custom filter payload map with key and value")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut key_acc: Option<String> = None;
        let mut value_acc: Option<String> = None;
        while let Some(field_key) = map.next_key::<String>()? {
            match field_key.as_str() {
                "key" => {
                    if key_acc.is_some() {
                        return Err(de::Error::duplicate_field("key"));
                    }
                    key_acc = Some(map.next_value()?);
                }
                "value" => {
                    if value_acc.is_some() {
                        return Err(de::Error::duplicate_field("value"));
                    }
                    value_acc = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, LQ_FILTER_CUSTOM_FIELDS));
                }
            }
        }
        let key = key_acc.ok_or_else(|| de::Error::missing_field("key"))?;
        let value = value_acc.ok_or_else(|| de::Error::missing_field("value"))?;
        Ok(CustomFilterPayloadDe { key, value })
    }
}

impl<'de> Deserialize<'de> for CustomFilterPayloadDe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "CustomFilterPayload",
            LQ_FILTER_CUSTOM_FIELDS,
            CustomFilterPayloadVisitor,
        )
    }
}

impl Serialize for LqFilter {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LqFilter", 2)?;
        state.serialize_field("kind", self.kind())?;
        match self {
            Self::Repo(value)
            | Self::File(value)
            | Self::Path(value)
            | Self::Lang(value)
            | Self::Rev(value)
            | Self::Select(value)
            | Self::Type(value) => {
                state.serialize_field("payload", value)?;
            }
            Self::Custom { key, value } => {
                state.serialize_field("payload", &CustomFilterPayloadSer { key, value })?;
            }
        }
        state.end()
    }
}

struct LqFilterVisitor;

impl<'de> Visitor<'de> for LqFilterVisitor {
    type Value = LqFilter;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LqFilter map with kind and payload fields")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut value: Option<LqFilter> = None;
        let mut payload_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if payload_seen {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    payload_seen = true;
                    let Some(current_kind) = kind.as_deref() else {
                        return Err(de::Error::custom(
                            "`kind` must appear before `payload` in LqFilter",
                        ));
                    };
                    let parsed = match current_kind {
                        "Repo" => LqFilter::Repo(map.next_value()?),
                        "File" => LqFilter::File(map.next_value()?),
                        "Path" => LqFilter::Path(map.next_value()?),
                        "Lang" => LqFilter::Lang(map.next_value()?),
                        "Rev" => LqFilter::Rev(map.next_value()?),
                        "Select" => LqFilter::Select(map.next_value()?),
                        "Type" => LqFilter::Type(map.next_value()?),
                        "Custom" => {
                            let payload: CustomFilterPayloadDe = map.next_value()?;
                            LqFilter::Custom {
                                key: payload.key,
                                value: payload.value,
                            }
                        }
                        other => {
                            return Err(de::Error::unknown_variant(other, LqFilter::VARIANTS));
                        }
                    };
                    value = Some(parsed);
                }
                other => return Err(de::Error::unknown_field(other, LQ_FILTER_FIELDS)),
            }
        }
        value.ok_or_else(|| de::Error::missing_field("payload"))
    }
}

impl<'de> Deserialize<'de> for LqFilter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("LqFilter", LQ_FILTER_FIELDS, LqFilterVisitor)
    }
}
