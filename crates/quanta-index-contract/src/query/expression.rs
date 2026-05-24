use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{LqDirectiveSet, LqFilterSet, LqOptionSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LqQuery {
    pub expr: LqExpr,
    pub filters: LqFilterSet,
    pub options: LqOptionSet,
    pub directives: LqDirectiveSet,
}

const LQ_QUERY_FIELDS: &[&str] = &["expr", "filters", "options", "directives"];

impl Serialize for LqQuery {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LqQuery", 4)?;
        state.serialize_field("expr", &self.expr)?;
        state.serialize_field("filters", &self.filters)?;
        state.serialize_field("options", &self.options)?;
        state.serialize_field("directives", &self.directives)?;
        state.end()
    }
}

struct LqQueryVisitor;

impl<'de> Visitor<'de> for LqQueryVisitor {
    type Value = LqQuery;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LqQuery map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut expr: Option<LqExpr> = None;
        let mut filters: Option<LqFilterSet> = None;
        let mut options: Option<LqOptionSet> = None;
        let mut directives: Option<LqDirectiveSet> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "expr" => {
                    if expr.is_some() {
                        return Err(de::Error::duplicate_field("expr"));
                    }
                    expr = Some(map.next_value()?);
                }
                "filters" => {
                    if filters.is_some() {
                        return Err(de::Error::duplicate_field("filters"));
                    }
                    filters = Some(map.next_value()?);
                }
                "options" => {
                    if options.is_some() {
                        return Err(de::Error::duplicate_field("options"));
                    }
                    options = Some(map.next_value()?);
                }
                "directives" => {
                    if directives.is_some() {
                        return Err(de::Error::duplicate_field("directives"));
                    }
                    directives = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, LQ_QUERY_FIELDS)),
            }
        }
        let expr = expr.ok_or_else(|| de::Error::missing_field("expr"))?;
        let filters = filters.ok_or_else(|| de::Error::missing_field("filters"))?;
        let options = options.ok_or_else(|| de::Error::missing_field("options"))?;
        let directives = directives.ok_or_else(|| de::Error::missing_field("directives"))?;
        Ok(LqQuery {
            expr,
            filters,
            options,
            directives,
        })
    }
}

impl<'de> Deserialize<'de> for LqQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("LqQuery", LQ_QUERY_FIELDS, LqQueryVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LqExpr {
    MatchAll,
    Raw(String),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
}

impl LqExpr {
    const VARIANTS: &'static [&'static str] = &["MatchAll", "Raw", "All", "Any", "Not"];
}

const LQ_EXPR_FIELDS: &[&str] = &["kind", "payload"];

struct LqExprListSer<'a> {
    items: &'a [LqExpr],
}

impl Serialize for LqExprListSer<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.items.serialize(serializer)
    }
}

impl Serialize for LqExpr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::MatchAll => {
                let mut state = serializer.serialize_struct("LqExpr", 1)?;
                state.serialize_field("kind", "MatchAll")?;
                state.end()
            }
            Self::Raw(value) => {
                let mut state = serializer.serialize_struct("LqExpr", 2)?;
                state.serialize_field("kind", "Raw")?;
                state.serialize_field("payload", value)?;
                state.end()
            }
            Self::All(items) => {
                let mut state = serializer.serialize_struct("LqExpr", 2)?;
                state.serialize_field("kind", "All")?;
                state.serialize_field("payload", &LqExprListSer { items })?;
                state.end()
            }
            Self::Any(items) => {
                let mut state = serializer.serialize_struct("LqExpr", 2)?;
                state.serialize_field("kind", "Any")?;
                state.serialize_field("payload", &LqExprListSer { items })?;
                state.end()
            }
            Self::Not(inner) => {
                let mut state = serializer.serialize_struct("LqExpr", 2)?;
                state.serialize_field("kind", "Not")?;
                state.serialize_field("payload", inner.as_ref())?;
                state.end()
            }
        }
    }
}

struct LqExprVisitor;

impl<'de> Visitor<'de> for LqExprVisitor {
    type Value = LqExpr;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LqExpr map with kind and optional payload")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut value: Option<LqExpr> = None;
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
                            "`kind` must appear before `payload` in LqExpr",
                        ));
                    };
                    let parsed = match current_kind {
                        "MatchAll" => {
                            return Err(de::Error::custom(
                                "LqExpr::MatchAll must not carry a `payload`",
                            ));
                        }
                        "Raw" => {
                            let inner: String = map.next_value()?;
                            LqExpr::Raw(inner)
                        }
                        "All" => {
                            let inner: Vec<LqExpr> = map.next_value()?;
                            LqExpr::All(inner)
                        }
                        "Any" => {
                            let inner: Vec<LqExpr> = map.next_value()?;
                            LqExpr::Any(inner)
                        }
                        "Not" => {
                            let inner: LqExpr = map.next_value()?;
                            LqExpr::Not(Box::new(inner))
                        }
                        other => {
                            return Err(de::Error::unknown_variant(other, LqExpr::VARIANTS));
                        }
                    };
                    value = Some(parsed);
                }
                other => return Err(de::Error::unknown_field(other, LQ_EXPR_FIELDS)),
            }
        }
        if let Some(parsed) = value {
            return Ok(parsed);
        }
        let Some(current_kind) = kind.as_deref() else {
            return Err(de::Error::missing_field("kind"));
        };
        match current_kind {
            "MatchAll" => Ok(LqExpr::MatchAll),
            "Raw" | "All" | "Any" | "Not" => Err(de::Error::missing_field("payload")),
            other => Err(de::Error::unknown_variant(other, LqExpr::VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for LqExpr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("LqExpr", LQ_EXPR_FIELDS, LqExprVisitor)
    }
}
