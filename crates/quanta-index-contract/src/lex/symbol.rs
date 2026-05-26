//! `SymbolKindCode` / `SymbolKindFamily` / `SymbolRelationship` /
//! `SymbolSpan` / `SymbolRecord`.
//!
//! Wire shape: producer-authored symbol rows for lexical publish.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use super::lang::LanguageCode;
use crate::query::LqVisibility;
use crate::{RepoRelativePath, SymbolId};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolKindCode(Box<str>);

impl SymbolKindCode {
    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        Self::new(value).into_iter().next()
    }

    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        validate_symbol_kind_code(value.as_str())?;
        Ok(Self(value.into_boxed_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    #[must_use]
    pub fn into_inner(self) -> Box<str> {
        self.0
    }
}

impl fmt::Display for SymbolKindCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for SymbolKindCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct SymbolKindCodeVisitor;

impl Visitor<'_> for SymbolKindCodeVisitor {
    type Value = SymbolKindCode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a canonical lowercase snake_case SymbolKindCode string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SymbolKindCode::new(value).map_err(de::Error::custom)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SymbolKindCode::new(value).map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for SymbolKindCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(SymbolKindCodeVisitor)
    }
}

fn validate_symbol_kind_code(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("symbol kind code must not be empty");
    }
    let Some(first) = value.as_bytes().first().copied() else {
        return Err("symbol kind code must not be empty");
    };
    if !first.is_ascii_lowercase() {
        return Err("symbol kind code must start with a lowercase ASCII letter");
    }
    for byte in value.bytes() {
        let ok = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_';
        if !ok {
            return Err("symbol kind code must be lowercase snake_case ASCII");
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolKindFamily {
    Callable,
    Type,
    Module,
    Value,
    Macro,
}

impl SymbolKindFamily {
    pub const ALL: &'static [Self] = &[
        Self::Callable,
        Self::Type,
        Self::Module,
        Self::Value,
        Self::Macro,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Callable => "Callable",
            Self::Type => "Type",
            Self::Module => "Module",
            Self::Value => "Value",
            Self::Macro => "Macro",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "Callable" => Some(Self::Callable),
            "Type" => Some(Self::Type),
            "Module" => Some(Self::Module),
            "Value" => Some(Self::Value),
            "Macro" => Some(Self::Macro),
            _ => None,
        }
    }
}

impl Serialize for SymbolKindFamily {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct SymbolKindFamilyVisitor;

impl Visitor<'_> for SymbolKindFamilyVisitor {
    type Value = SymbolKindFamily;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolKindFamily code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SymbolKindFamily::from_code_str(value).ok_or_else(|| {
            de::Error::unknown_variant(value, &["Callable", "Type", "Module", "Value", "Macro"])
        })
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for SymbolKindFamily {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SymbolKindFamilyVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolRelationship {
    Def,
    Ref,
}

impl SymbolRelationship {
    pub const ALL: &'static [Self] = &[Self::Def, Self::Ref];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Def => "Def",
            Self::Ref => "Ref",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "Def" => Some(Self::Def),
            "Ref" => Some(Self::Ref),
            _ => None,
        }
    }
}

impl fmt::Display for SymbolRelationship {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

impl Serialize for SymbolRelationship {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct SymbolRelationshipVisitor;

impl Visitor<'_> for SymbolRelationshipVisitor {
    type Value = SymbolRelationship;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolRelationship code string (\"Def\" | \"Ref\")")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SymbolRelationship::from_code_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, &["Def", "Ref"]))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for SymbolRelationship {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SymbolRelationshipVisitor)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolSpan {
    pub path: Box<str>,
    pub byte_start: u32,
    pub byte_end: u32,
    pub line_start: u32,
    pub line_end: u32,
}

const SYMBOL_SPAN_FIELDS: &[&str] = &["path", "byte_start", "byte_end", "line_start", "line_end"];

impl Serialize for SymbolSpan {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolSpan", 5)?;
        state.serialize_field("path", self.path.as_ref())?;
        state.serialize_field("byte_start", &self.byte_start)?;
        state.serialize_field("byte_end", &self.byte_end)?;
        state.serialize_field("line_start", &self.line_start)?;
        state.serialize_field("line_end", &self.line_end)?;
        state.end()
    }
}

struct SymbolSpanVisitor;

impl<'de> Visitor<'de> for SymbolSpanVisitor {
    type Value = SymbolSpan;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolSpan map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut path: Option<String> = None;
        let mut byte_start: Option<u32> = None;
        let mut byte_end: Option<u32> = None;
        let mut line_start: Option<u32> = None;
        let mut line_end: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "path" => {
                    if path.is_some() {
                        return Err(de::Error::duplicate_field("path"));
                    }
                    path = Some(map.next_value()?);
                }
                "byte_start" => {
                    if byte_start.is_some() {
                        return Err(de::Error::duplicate_field("byte_start"));
                    }
                    byte_start = Some(map.next_value()?);
                }
                "byte_end" => {
                    if byte_end.is_some() {
                        return Err(de::Error::duplicate_field("byte_end"));
                    }
                    byte_end = Some(map.next_value()?);
                }
                "line_start" => {
                    if line_start.is_some() {
                        return Err(de::Error::duplicate_field("line_start"));
                    }
                    line_start = Some(map.next_value()?);
                }
                "line_end" => {
                    if line_end.is_some() {
                        return Err(de::Error::duplicate_field("line_end"));
                    }
                    line_end = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, SYMBOL_SPAN_FIELDS)),
            }
        }
        Ok(SymbolSpan {
            path: path
                .ok_or_else(|| de::Error::missing_field("path"))?
                .into_boxed_str(),
            byte_start: byte_start.ok_or_else(|| de::Error::missing_field("byte_start"))?,
            byte_end: byte_end.ok_or_else(|| de::Error::missing_field("byte_end"))?,
            line_start: line_start.ok_or_else(|| de::Error::missing_field("line_start"))?,
            line_end: line_end.ok_or_else(|| de::Error::missing_field("line_end"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SymbolSpan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("SymbolSpan", SYMBOL_SPAN_FIELDS, SymbolSpanVisitor)
    }
}

impl fmt::Display for SymbolSpan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}..{}:{}",
            self.path, self.line_start, self.byte_start, self.line_end, self.byte_end,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolRecord {
    pub symbol_id: SymbolId,
    pub repo_relative_path: RepoRelativePath,
    pub language: LanguageCode,
    pub symbol_kind: SymbolKindCode,
    pub symbol_kind_family: Option<SymbolKindFamily>,
    pub local_name: Box<str>,
    pub qualified_name: Box<str>,
    pub signature: Option<Box<str>>,
    pub visibility: Option<LqVisibility>,
    pub definition_span: SymbolSpan,
    pub container_qualified_name: Option<Box<str>>,
    pub relationship: SymbolRelationship,
}

const SYMBOL_RECORD_FIELDS: &[&str] = &[
    "symbol_id",
    "repo_relative_path",
    "language",
    "symbol_kind",
    "symbol_kind_family",
    "local_name",
    "qualified_name",
    "signature",
    "visibility",
    "definition_span",
    "container_qualified_name",
    "relationship",
];

impl Serialize for SymbolRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolRecord", 12)?;
        state.serialize_field("symbol_id", &self.symbol_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("language", &self.language)?;
        state.serialize_field("symbol_kind", &self.symbol_kind)?;
        state.serialize_field("symbol_kind_family", &self.symbol_kind_family)?;
        state.serialize_field("local_name", self.local_name.as_ref())?;
        state.serialize_field("qualified_name", self.qualified_name.as_ref())?;
        state.serialize_field("signature", &self.signature.as_deref())?;
        state.serialize_field("visibility", &self.visibility)?;
        state.serialize_field("definition_span", &self.definition_span)?;
        state.serialize_field(
            "container_qualified_name",
            &self.container_qualified_name.as_deref(),
        )?;
        state.serialize_field("relationship", &self.relationship)?;
        state.end()
    }
}

struct SymbolRecordVisitor;

impl<'de> Visitor<'de> for SymbolRecordVisitor {
    type Value = SymbolRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut symbol_id: Option<SymbolId> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut language: Option<LanguageCode> = None;
        let mut symbol_kind: Option<SymbolKindCode> = None;
        let mut symbol_kind_family: Option<Option<SymbolKindFamily>> = None;
        let mut local_name: Option<String> = None;
        let mut qualified_name: Option<String> = None;
        let mut signature: Option<Option<String>> = None;
        let mut visibility: Option<Option<LqVisibility>> = None;
        let mut definition_span: Option<SymbolSpan> = None;
        let mut container_qualified_name: Option<Option<String>> = None;
        let mut relationship: Option<SymbolRelationship> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "symbol_id" => {
                    if symbol_id.is_some() {
                        return Err(de::Error::duplicate_field("symbol_id"));
                    }
                    symbol_id = Some(map.next_value()?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                "language" => {
                    if language.is_some() {
                        return Err(de::Error::duplicate_field("language"));
                    }
                    language = Some(map.next_value()?);
                }
                "symbol_kind" => {
                    if symbol_kind.is_some() {
                        return Err(de::Error::duplicate_field("symbol_kind"));
                    }
                    symbol_kind = Some(map.next_value()?);
                }
                "symbol_kind_family" => {
                    if symbol_kind_family.is_some() {
                        return Err(de::Error::duplicate_field("symbol_kind_family"));
                    }
                    symbol_kind_family = Some(map.next_value()?);
                }
                "local_name" => {
                    if local_name.is_some() {
                        return Err(de::Error::duplicate_field("local_name"));
                    }
                    local_name = Some(map.next_value()?);
                }
                "qualified_name" => {
                    if qualified_name.is_some() {
                        return Err(de::Error::duplicate_field("qualified_name"));
                    }
                    qualified_name = Some(map.next_value()?);
                }
                "signature" => {
                    if signature.is_some() {
                        return Err(de::Error::duplicate_field("signature"));
                    }
                    signature = Some(map.next_value()?);
                }
                "visibility" => {
                    if visibility.is_some() {
                        return Err(de::Error::duplicate_field("visibility"));
                    }
                    visibility = Some(map.next_value()?);
                }
                "definition_span" => {
                    if definition_span.is_some() {
                        return Err(de::Error::duplicate_field("definition_span"));
                    }
                    definition_span = Some(map.next_value()?);
                }
                "container_qualified_name" => {
                    if container_qualified_name.is_some() {
                        return Err(de::Error::duplicate_field("container_qualified_name"));
                    }
                    container_qualified_name = Some(map.next_value()?);
                }
                "relationship" => {
                    if relationship.is_some() {
                        return Err(de::Error::duplicate_field("relationship"));
                    }
                    relationship = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, SYMBOL_RECORD_FIELDS)),
            }
        }
        Ok(SymbolRecord {
            symbol_id: symbol_id.ok_or_else(|| de::Error::missing_field("symbol_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            language: language.ok_or_else(|| de::Error::missing_field("language"))?,
            symbol_kind: symbol_kind.ok_or_else(|| de::Error::missing_field("symbol_kind"))?,
            symbol_kind_family: symbol_kind_family
                .ok_or_else(|| de::Error::missing_field("symbol_kind_family"))?,
            local_name: local_name
                .ok_or_else(|| de::Error::missing_field("local_name"))?
                .into_boxed_str(),
            qualified_name: qualified_name
                .ok_or_else(|| de::Error::missing_field("qualified_name"))?
                .into_boxed_str(),
            signature: signature
                .ok_or_else(|| de::Error::missing_field("signature"))?
                .map(String::into_boxed_str),
            visibility: visibility.ok_or_else(|| de::Error::missing_field("visibility"))?,
            definition_span: definition_span
                .ok_or_else(|| de::Error::missing_field("definition_span"))?,
            container_qualified_name: container_qualified_name
                .ok_or_else(|| de::Error::missing_field("container_qualified_name"))?
                .map(String::into_boxed_str),
            relationship: relationship.ok_or_else(|| de::Error::missing_field("relationship"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SymbolRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("SymbolRecord", SYMBOL_RECORD_FIELDS, SymbolRecordVisitor)
    }
}

impl fmt::Display for SymbolRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} @ {} ({}/{})",
            self.symbol_kind,
            self.qualified_name,
            self.definition_span,
            self.language,
            self.relationship,
        )
    }
}
