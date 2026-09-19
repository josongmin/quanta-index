//! The kinds that classify semantic records.
//!
//! Which document owns a record, which corpus it belongs to, what role its
//! source plays, and whether a capability is present. Both the channel
//! records and the semantic source DTOs are built from them.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

const OWNER_DOC_KIND_VARIANTS: &[&str] = &[
    "File",
    "Module",
    "Symbol",
    "Chunk",
    "Callsite",
    "GraphEdge",
    "Dataflow",
    "Risk",
    "Test",
    "RepoMap",
    "ServiceMap",
    "OwnerMap",
];

const SEMANTIC_CORPUS_KIND_V1_VARIANTS: &[&str] = &[
    "SymbolCard",
    "ModuleCard",
    "ClusterCard",
    "RawCodeFallback",
    "DocumentLeaf",
    "DocumentSection",
    "DocumentSummary",
    "TestBehavior",
    "RepositorySummary",
];

const SOURCE_ROLE_V1_VARIANTS: &[&str] =
    &["CardText", "RawFallbackText", "DocumentText", "SummaryText"];

const CAPABILITY_STATUS_V1_VARIANTS: &[&str] = &["Full", "Degraded", "Unsupported", "NotComputed"];

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OwnerDocKind {
    File,
    Module,
    Symbol,
    Chunk,
    Callsite,
    GraphEdge,
    Dataflow,
    Risk,
    Test,
    RepoMap,
    ServiceMap,
    OwnerMap,
}

impl OwnerDocKind {
    pub const ALL: &'static [Self] = &[
        Self::File,
        Self::Module,
        Self::Symbol,
        Self::Chunk,
        Self::Callsite,
        Self::GraphEdge,
        Self::Dataflow,
        Self::Risk,
        Self::Test,
        Self::RepoMap,
        Self::ServiceMap,
        Self::OwnerMap,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Module => "Module",
            Self::Symbol => "Symbol",
            Self::Chunk => "Chunk",
            Self::Callsite => "Callsite",
            Self::GraphEdge => "GraphEdge",
            Self::Dataflow => "Dataflow",
            Self::Risk => "Risk",
            Self::Test => "Test",
            Self::RepoMap => "RepoMap",
            Self::ServiceMap => "ServiceMap",
            Self::OwnerMap => "OwnerMap",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "File" => Some(Self::File),
            "Module" => Some(Self::Module),
            "Symbol" => Some(Self::Symbol),
            "Chunk" => Some(Self::Chunk),
            "Callsite" => Some(Self::Callsite),
            "GraphEdge" => Some(Self::GraphEdge),
            "Dataflow" => Some(Self::Dataflow),
            "Risk" => Some(Self::Risk),
            "Test" => Some(Self::Test),
            "RepoMap" => Some(Self::RepoMap),
            "ServiceMap" => Some(Self::ServiceMap),
            "OwnerMap" => Some(Self::OwnerMap),
            _ => None,
        }
    }
}

impl Serialize for OwnerDocKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for OwnerDocKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(OwnerDocKindVisitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SemanticCorpusKindV1 {
    SymbolCard,
    ModuleCard,
    ClusterCard,
    RawCodeFallback,
    DocumentLeaf,
    DocumentSection,
    DocumentSummary,
    TestBehavior,
    RepositorySummary,
}

impl SemanticCorpusKindV1 {
    pub const ALL: &'static [Self] = &[
        Self::SymbolCard,
        Self::ModuleCard,
        Self::ClusterCard,
        Self::RawCodeFallback,
        Self::DocumentLeaf,
        Self::DocumentSection,
        Self::DocumentSummary,
        Self::TestBehavior,
        Self::RepositorySummary,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::SymbolCard => "SymbolCard",
            Self::ModuleCard => "ModuleCard",
            Self::ClusterCard => "ClusterCard",
            Self::RawCodeFallback => "RawCodeFallback",
            Self::DocumentLeaf => "DocumentLeaf",
            Self::DocumentSection => "DocumentSection",
            Self::DocumentSummary => "DocumentSummary",
            Self::TestBehavior => "TestBehavior",
            Self::RepositorySummary => "RepositorySummary",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "SymbolCard" => Some(Self::SymbolCard),
            "ModuleCard" => Some(Self::ModuleCard),
            "ClusterCard" => Some(Self::ClusterCard),
            "RawCodeFallback" => Some(Self::RawCodeFallback),
            "DocumentLeaf" => Some(Self::DocumentLeaf),
            "DocumentSection" => Some(Self::DocumentSection),
            "DocumentSummary" => Some(Self::DocumentSummary),
            "TestBehavior" => Some(Self::TestBehavior),
            "RepositorySummary" => Some(Self::RepositorySummary),
            _ => None,
        }
    }
}

impl Serialize for SemanticCorpusKindV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for SemanticCorpusKindV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SemanticCorpusKindV1Visitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
// These suffixes are stable serialized vocabulary in the semantic-source IPC.
#[expect(
    clippy::enum_variant_names,
    reason = "wire-compatible semantic-source role names intentionally share the Text suffix"
)]
pub enum SourceRoleV1 {
    CardText,
    RawFallbackText,
    DocumentText,
    SummaryText,
}

impl SourceRoleV1 {
    pub const ALL: &'static [Self] = &[
        Self::CardText,
        Self::RawFallbackText,
        Self::DocumentText,
        Self::SummaryText,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::CardText => "CardText",
            Self::RawFallbackText => "RawFallbackText",
            Self::DocumentText => "DocumentText",
            Self::SummaryText => "SummaryText",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "CardText" => Some(Self::CardText),
            "RawFallbackText" => Some(Self::RawFallbackText),
            "DocumentText" => Some(Self::DocumentText),
            "SummaryText" => Some(Self::SummaryText),
            _ => None,
        }
    }
}

impl Serialize for SourceRoleV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for SourceRoleV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(SourceRoleV1Visitor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CapabilityStatusV1 {
    Full,
    Degraded,
    Unsupported,
    NotComputed,
}

impl CapabilityStatusV1 {
    pub const ALL: &'static [Self] = &[
        Self::Full,
        Self::Degraded,
        Self::Unsupported,
        Self::NotComputed,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::Degraded => "Degraded",
            Self::Unsupported => "Unsupported",
            Self::NotComputed => "NotComputed",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "Full" => Some(Self::Full),
            "Degraded" => Some(Self::Degraded),
            "Unsupported" => Some(Self::Unsupported),
            "NotComputed" => Some(Self::NotComputed),
            _ => None,
        }
    }
}

impl Serialize for CapabilityStatusV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

impl<'de> Deserialize<'de> for CapabilityStatusV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(CapabilityStatusV1Visitor)
    }
}

struct OwnerDocKindVisitor;

impl Visitor<'_> for OwnerDocKindVisitor {
    type Value = OwnerDocKind;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an OwnerDocKind code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        OwnerDocKind::from_code_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, OWNER_DOC_KIND_VARIANTS))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

struct SemanticCorpusKindV1Visitor;

impl Visitor<'_> for SemanticCorpusKindV1Visitor {
    type Value = SemanticCorpusKindV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticCorpusKindV1 code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SemanticCorpusKindV1::from_code_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, SEMANTIC_CORPUS_KIND_V1_VARIANTS))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

struct SourceRoleV1Visitor;

impl Visitor<'_> for SourceRoleV1Visitor {
    type Value = SourceRoleV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SourceRoleV1 code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        SourceRoleV1::from_code_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, SOURCE_ROLE_V1_VARIANTS))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

struct CapabilityStatusV1Visitor;

impl Visitor<'_> for CapabilityStatusV1Visitor {
    type Value = CapabilityStatusV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a CapabilityStatusV1 code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        CapabilityStatusV1::from_code_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, CAPABILITY_STATUS_V1_VARIANTS))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}
