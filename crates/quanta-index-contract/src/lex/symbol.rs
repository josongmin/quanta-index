//! `SymbolKind` / `SymbolRelationship` / `SymbolSpan` / `SymbolRecord`.
//!
//! Wire shape: [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
//! §3.4 — the CBOR decoded form of `UpsertSymbol.payload` is a `SymbolRecord`.
//!
//! `SymbolKind` ships the v1 12-variant set per producer-handoff §3.4.2 +
//! LEX-05 §3.3. The longer 20-variant set listed in LEX-05 for v2 is **not**
//! landed here; growing the set requires a coordinated `wire_version` bump per
//! producer-handoff §5.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use super::lang::LangId;

/// Producer-supplied symbol category.
///
/// Closed v1 set per [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
/// §3.4.2. Pinned at 12 variants; v2 expansion is gated on the §8 handshake.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Variable,
    Constant,
    Module,
    Macro,
    TypeAlias,
}

impl SymbolKind {
    pub const ALL: &'static [Self] = &[
        Self::Function,
        Self::Method,
        Self::Class,
        Self::Struct,
        Self::Enum,
        Self::Trait,
        Self::Interface,
        Self::Variable,
        Self::Constant,
        Self::Module,
        Self::Macro,
        Self::TypeAlias,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Function => "Function",
            Self::Method => "Method",
            Self::Class => "Class",
            Self::Struct => "Struct",
            Self::Enum => "Enum",
            Self::Trait => "Trait",
            Self::Interface => "Interface",
            Self::Variable => "Variable",
            Self::Constant => "Constant",
            Self::Module => "Module",
            Self::Macro => "Macro",
            Self::TypeAlias => "TypeAlias",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "Function" => Some(Self::Function),
            "Method" => Some(Self::Method),
            "Class" => Some(Self::Class),
            "Struct" => Some(Self::Struct),
            "Enum" => Some(Self::Enum),
            "Trait" => Some(Self::Trait),
            "Interface" => Some(Self::Interface),
            "Variable" => Some(Self::Variable),
            "Constant" => Some(Self::Constant),
            "Module" => Some(Self::Module),
            "Macro" => Some(Self::Macro),
            "TypeAlias" => Some(Self::TypeAlias),
            _ => None,
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

impl Serialize for SymbolKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct SymbolKindVisitor;

impl Visitor<'_> for SymbolKindVisitor {
    type Value = SymbolKind;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolKind code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SymbolKind::from_code_str(value).ok_or_else(|| {
            de::Error::unknown_variant(
                value,
                &[
                    "Function",
                    "Method",
                    "Class",
                    "Struct",
                    "Enum",
                    "Trait",
                    "Interface",
                    "Variable",
                    "Constant",
                    "Module",
                    "Macro",
                    "TypeAlias",
                ],
            )
        })
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for SymbolKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SymbolKindVisitor)
    }
}

/// Definition vs reference. Per LEX-05 §3.5 the producer pre-classifies every
/// symbol record into one of these two roles.
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

/// Symbol span coordinates per producer-handoff §3.4.1.
///
/// All offsets are producer-authoritative; the search side does not recompute
/// them. `byte_end >= byte_start` and `line_end >= line_start` are wire-level
/// invariants and the producer is the source of truth; this scaffold does
/// **not** enforce them at deserialize time so that callers can route
/// invalid records through the typed `SYMBOL_RECORD_INVALID` path explicitly.
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
        let path = path.ok_or_else(|| de::Error::missing_field("path"))?;
        let byte_start = byte_start.ok_or_else(|| de::Error::missing_field("byte_start"))?;
        let byte_end = byte_end.ok_or_else(|| de::Error::missing_field("byte_end"))?;
        let line_start = line_start.ok_or_else(|| de::Error::missing_field("line_start"))?;
        let line_end = line_end.ok_or_else(|| de::Error::missing_field("line_end"))?;
        Ok(SymbolSpan {
            path: path.into_boxed_str(),
            byte_start,
            byte_end,
            line_start,
            line_end,
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

/// Producer-authored symbol record per producer-handoff §3.4.1.
///
/// `wire_version` is locked at 1 for the v1 cutover; bumps require a §5
/// coordinated cutover. Out-of-range values surface as `SYMBOL_RECORD_INVALID`
/// at the decode site (LEX-05); this scaffold does not enforce the range here.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolRecord {
    pub wire_version: u32,
    pub name: Box<str>,
    pub kind: SymbolKind,
    pub span: SymbolSpan,
    pub lang: LangId,
    pub parent: Option<Box<str>>,
    pub container_name: Option<Box<str>>,
    pub relationship: SymbolRelationship,
}

const SYMBOL_RECORD_FIELDS: &[&str] = &[
    "wire_version",
    "name",
    "kind",
    "span",
    "lang",
    "parent",
    "container_name",
    "relationship",
];

impl Serialize for SymbolRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolRecord", 8)?;
        state.serialize_field("wire_version", &self.wire_version)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("span", &self.span)?;
        state.serialize_field("lang", &self.lang)?;
        state.serialize_field("parent", &self.parent.as_deref())?;
        state.serialize_field("container_name", &self.container_name.as_deref())?;
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
        let mut wire_version: Option<u32> = None;
        let mut name: Option<String> = None;
        let mut kind: Option<SymbolKind> = None;
        let mut span: Option<SymbolSpan> = None;
        let mut lang: Option<LangId> = None;
        let mut parent: Option<Option<String>> = None;
        let mut container_name: Option<Option<String>> = None;
        let mut relationship: Option<SymbolRelationship> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "wire_version" => {
                    if wire_version.is_some() {
                        return Err(de::Error::duplicate_field("wire_version"));
                    }
                    wire_version = Some(map.next_value()?);
                }
                "name" => {
                    if name.is_some() {
                        return Err(de::Error::duplicate_field("name"));
                    }
                    name = Some(map.next_value()?);
                }
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "span" => {
                    if span.is_some() {
                        return Err(de::Error::duplicate_field("span"));
                    }
                    span = Some(map.next_value()?);
                }
                "lang" => {
                    if lang.is_some() {
                        return Err(de::Error::duplicate_field("lang"));
                    }
                    lang = Some(map.next_value()?);
                }
                "parent" => {
                    if parent.is_some() {
                        return Err(de::Error::duplicate_field("parent"));
                    }
                    parent = Some(map.next_value()?);
                }
                "container_name" => {
                    if container_name.is_some() {
                        return Err(de::Error::duplicate_field("container_name"));
                    }
                    container_name = Some(map.next_value()?);
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
        let wire_version =
            wire_version.ok_or_else(|| de::Error::missing_field("wire_version"))?;
        let name = name.ok_or_else(|| de::Error::missing_field("name"))?;
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        let span = span.ok_or_else(|| de::Error::missing_field("span"))?;
        let lang = lang.ok_or_else(|| de::Error::missing_field("lang"))?;
        let parent = parent.ok_or_else(|| de::Error::missing_field("parent"))?;
        let container_name =
            container_name.ok_or_else(|| de::Error::missing_field("container_name"))?;
        let relationship =
            relationship.ok_or_else(|| de::Error::missing_field("relationship"))?;
        Ok(SymbolRecord {
            wire_version,
            name: name.into_boxed_str(),
            kind,
            span,
            lang,
            parent: parent.map(String::into_boxed_str),
            container_name: container_name.map(String::into_boxed_str),
            relationship,
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
            self.kind, self.name, self.span, self.lang, self.relationship,
        )
    }
}
