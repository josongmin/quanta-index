use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LqOptionSet {
    pub limit: Option<u32>,
    pub count_all: bool,
    pub timeout_ms: Option<u64>,
}

const LQ_OPTION_SET_FIELDS: &[&str] = &["limit", "count_all", "timeout_ms"];

impl Serialize for LqOptionSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 1;
        if self.limit.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.timeout_ms.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("LqOptionSet", field_count)?;
        if let Some(limit) = &self.limit {
            state.serialize_field("limit", limit)?;
        }
        state.serialize_field("count_all", &self.count_all)?;
        if let Some(timeout_ms) = &self.timeout_ms {
            state.serialize_field("timeout_ms", timeout_ms)?;
        }
        state.end()
    }
}

struct LqOptionSetVisitor;

impl<'de> Visitor<'de> for LqOptionSetVisitor {
    type Value = LqOptionSet;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LqOptionSet map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut limit: Option<u32> = None;
        let mut limit_seen = false;
        let mut count_all: Option<bool> = None;
        let mut timeout_ms: Option<u64> = None;
        let mut timeout_ms_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "limit" => {
                    if limit_seen {
                        return Err(de::Error::duplicate_field("limit"));
                    }
                    limit_seen = true;
                    limit = Some(map.next_value()?);
                }
                "count_all" => {
                    if count_all.is_some() {
                        return Err(de::Error::duplicate_field("count_all"));
                    }
                    count_all = Some(map.next_value()?);
                }
                "timeout_ms" => {
                    if timeout_ms_seen {
                        return Err(de::Error::duplicate_field("timeout_ms"));
                    }
                    timeout_ms_seen = true;
                    timeout_ms = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, LQ_OPTION_SET_FIELDS)),
            }
        }
        let count_all = count_all.ok_or_else(|| de::Error::missing_field("count_all"))?;
        Ok(LqOptionSet {
            limit,
            count_all,
            timeout_ms,
        })
    }
}

impl<'de> Deserialize<'de> for LqOptionSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("LqOptionSet", LQ_OPTION_SET_FIELDS, LqOptionSetVisitor)
    }
}
