use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LqDirectiveSet {
    pub directives: Vec<LqDirective>,
}

const LQ_DIRECTIVE_SET_FIELDS: &[&str] = &["directives"];

impl Serialize for LqDirectiveSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LqDirectiveSet", 1)?;
        state.serialize_field("directives", &self.directives)?;
        state.end()
    }
}

struct LqDirectiveSetVisitor;

impl<'de> Visitor<'de> for LqDirectiveSetVisitor {
    type Value = LqDirectiveSet;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LqDirectiveSet map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut directives: Option<Vec<LqDirective>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "directives" => {
                    if directives.is_some() {
                        return Err(de::Error::duplicate_field("directives"));
                    }
                    directives = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, LQ_DIRECTIVE_SET_FIELDS)),
            }
        }
        let directives = directives.ok_or_else(|| de::Error::missing_field("directives"))?;
        Ok(LqDirectiveSet { directives })
    }
}

impl<'de> Deserialize<'de> for LqDirectiveSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LqDirectiveSet",
            LQ_DIRECTIVE_SET_FIELDS,
            LqDirectiveSetVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LqDirective {
    IntoCodeQl,
    ScopeResults,
    WithLexical,
    Custom(String),
}

impl LqDirective {
    const VARIANTS: &'static [&'static str] =
        &["IntoCodeQl", "ScopeResults", "WithLexical", "Custom"];

    const fn kind(&self) -> &'static str {
        match self {
            Self::IntoCodeQl => "IntoCodeQl",
            Self::ScopeResults => "ScopeResults",
            Self::WithLexical => "WithLexical",
            Self::Custom(_) => "Custom",
        }
    }
}

const LQ_DIRECTIVE_FIELDS: &[&str] = &["kind", "payload"];

impl Serialize for LqDirective {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::IntoCodeQl | Self::ScopeResults | Self::WithLexical => {
                let mut state = serializer.serialize_struct("LqDirective", 1)?;
                state.serialize_field("kind", self.kind())?;
                state.end()
            }
            Self::Custom(value) => {
                let mut state = serializer.serialize_struct("LqDirective", 2)?;
                state.serialize_field("kind", "Custom")?;
                state.serialize_field("payload", value)?;
                state.end()
            }
        }
    }
}

struct LqDirectiveVisitor;

impl<'de> Visitor<'de> for LqDirectiveVisitor {
    type Value = LqDirective;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an LqDirective map with kind and optional payload fields")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut value: Option<LqDirective> = None;
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
                            "`kind` must appear before `payload` in LqDirective",
                        ));
                    };
                    let parsed = match current_kind {
                        "IntoCodeQl" | "ScopeResults" | "WithLexical" => {
                            return Err(de::Error::custom(
                                "unit LqDirective variants must not carry a `payload`",
                            ));
                        }
                        "Custom" => {
                            let inner: String = map.next_value()?;
                            LqDirective::Custom(inner)
                        }
                        other => {
                            return Err(de::Error::unknown_variant(other, LqDirective::VARIANTS));
                        }
                    };
                    value = Some(parsed);
                }
                other => return Err(de::Error::unknown_field(other, LQ_DIRECTIVE_FIELDS)),
            }
        }
        if let Some(parsed) = value {
            return Ok(parsed);
        }
        let Some(current_kind) = kind.as_deref() else {
            return Err(de::Error::missing_field("kind"));
        };
        match current_kind {
            "IntoCodeQl" => Ok(LqDirective::IntoCodeQl),
            "ScopeResults" => Ok(LqDirective::ScopeResults),
            "WithLexical" => Ok(LqDirective::WithLexical),
            "Custom" => Err(de::Error::missing_field("payload")),
            other => Err(de::Error::unknown_variant(other, LqDirective::VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for LqDirective {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("LqDirective", LQ_DIRECTIVE_FIELDS, LqDirectiveVisitor)
    }
}
