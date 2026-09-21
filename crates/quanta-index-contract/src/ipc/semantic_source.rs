use core::fmt;
use core::fmt::Write as _;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};
use sha2::{Digest as _, Sha256};

use crate::bounded_cluster_members::BoundedClusterMembersV1;
use crate::canonical_order::{CanonicalOrderBreakV1, first_canonical_order_break_v1};
use crate::semantic_kinds::{CapabilityStatusV1, SemanticCorpusKindV1, SourceRoleV1};
use crate::{MAX_CLUSTER_MEMBERSHIP_READ_V1, OwnerDocKind, RepoRelativePath, SymbolId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RawFallbackReasonV1 {
    MacroHeavy,
    DslConfigOrSchema,
    AlgorithmBodyDependent,
    DynamicDispatchUnmodeled,
    SimilarImplementationSearch,
    IntentNotRecoverableFromStructure,
}

impl RawFallbackReasonV1 {
    pub const ALL: &'static [Self] = &[
        Self::MacroHeavy,
        Self::DslConfigOrSchema,
        Self::AlgorithmBodyDependent,
        Self::DynamicDispatchUnmodeled,
        Self::SimilarImplementationSearch,
        Self::IntentNotRecoverableFromStructure,
    ];

    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::MacroHeavy => "MacroHeavy",
            Self::DslConfigOrSchema => "DslConfigOrSchema",
            Self::AlgorithmBodyDependent => "AlgorithmBodyDependent",
            Self::DynamicDispatchUnmodeled => "DynamicDispatchUnmodeled",
            Self::SimilarImplementationSearch => "SimilarImplementationSearch",
            Self::IntentNotRecoverableFromStructure => "IntentNotRecoverableFromStructure",
        }
    }

    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "MacroHeavy" => Some(Self::MacroHeavy),
            "DslConfigOrSchema" => Some(Self::DslConfigOrSchema),
            "AlgorithmBodyDependent" => Some(Self::AlgorithmBodyDependent),
            "DynamicDispatchUnmodeled" => Some(Self::DynamicDispatchUnmodeled),
            "SimilarImplementationSearch" => Some(Self::SimilarImplementationSearch),
            "IntentNotRecoverableFromStructure" => Some(Self::IntentNotRecoverableFromStructure),
            _ => None,
        }
    }
}

const RAW_FALLBACK_REASON_V1_VARIANTS: &[&str] = &[
    "MacroHeavy",
    "DslConfigOrSchema",
    "AlgorithmBodyDependent",
    "DynamicDispatchUnmodeled",
    "SimilarImplementationSearch",
    "IntentNotRecoverableFromStructure",
];

impl Serialize for RawFallbackReasonV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct RawFallbackReasonV1Visitor;

impl Visitor<'_> for RawFallbackReasonV1Visitor {
    type Value = RawFallbackReasonV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RawFallbackReasonV1 code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        RawFallbackReasonV1::from_code_str(value)
            .ok_or_else(|| de::Error::unknown_variant(value, RAW_FALLBACK_REASON_V1_VARIANTS))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for RawFallbackReasonV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(RawFallbackReasonV1Visitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SemanticSourceScopeKeyV1 {
    pub corpus_kind: SemanticCorpusKindV1,
    pub owner_kind: OwnerDocKind,
    pub owner_id: String,
}

const SEMANTIC_SOURCE_SCOPE_KEY_V1_FIELDS: &[&str] = &["corpus_kind", "owner_kind", "owner_id"];

impl Serialize for SemanticSourceScopeKeyV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticSourceScopeKeyV1", 3)?;
        state.serialize_field("corpus_kind", &self.corpus_kind)?;
        state.serialize_field("owner_kind", &self.owner_kind)?;
        state.serialize_field("owner_id", &self.owner_id)?;
        state.end()
    }
}

struct SemanticSourceScopeKeyV1Visitor;

impl<'de> Visitor<'de> for SemanticSourceScopeKeyV1Visitor {
    type Value = SemanticSourceScopeKeyV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticSourceScopeKeyV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut corpus_kind: Option<SemanticCorpusKindV1> = None;
        let mut owner_kind: Option<OwnerDocKind> = None;
        let mut owner_id: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "corpus_kind" => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
                }
                "owner_kind" => {
                    if owner_kind.is_some() {
                        return Err(de::Error::duplicate_field("owner_kind"));
                    }
                    owner_kind = Some(map.next_value()?);
                }
                "owner_id" => {
                    if owner_id.is_some() {
                        return Err(de::Error::duplicate_field("owner_id"));
                    }
                    owner_id = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_SOURCE_SCOPE_KEY_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticSourceScopeKeyV1 {
            corpus_kind: corpus_kind.ok_or_else(|| de::Error::missing_field("corpus_kind"))?,
            owner_kind: owner_kind.ok_or_else(|| de::Error::missing_field("owner_kind"))?,
            owner_id: owner_id.ok_or_else(|| de::Error::missing_field("owner_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticSourceScopeKeyV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticSourceScopeKeyV1",
            SEMANTIC_SOURCE_SCOPE_KEY_V1_FIELDS,
            SemanticSourceScopeKeyV1Visitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticSourceRecordV1 {
    pub record_id: String,
    pub corpus_kind: SemanticCorpusKindV1,
    pub owner_kind: OwnerDocKind,
    pub owner_id: String,
    pub source_doc_id: String,
    pub parent_owner_id: Option<String>,
    pub repo_relative_path: RepoRelativePath,
    pub language: Option<String>,
    pub package: Option<String>,
    pub symbol_kind: Option<String>,
    pub visibility: Option<String>,
    pub source_role: SourceRoleV1,
    pub generated: bool,
    pub capability_status: CapabilityStatusV1,
    pub raw_fallback_reason: Option<RawFallbackReasonV1>,
    pub authority_digest: String,
    pub render_policy_digest: String,
    pub card_schema_version: u32,
    pub text: String,
}

const SEMANTIC_SOURCE_RECORD_V1_FIELDS: &[&str] = &[
    "record_id",
    "corpus_kind",
    "owner_kind",
    "owner_id",
    "source_doc_id",
    "parent_owner_id",
    "repo_relative_path",
    "language",
    "package",
    "symbol_kind",
    "visibility",
    "source_role",
    "generated",
    "capability_status",
    "raw_fallback_reason",
    "authority_digest",
    "render_policy_digest",
    "card_schema_version",
    "text",
];

impl Serialize for SemanticSourceRecordV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Err(message) = validate_semantic_source_record_v1(self) {
            return Err(serde::ser::Error::custom(message));
        }
        let mut state = serializer.serialize_struct("SemanticSourceRecordV1", 19)?;
        state.serialize_field("record_id", &self.record_id)?;
        state.serialize_field("corpus_kind", &self.corpus_kind)?;
        state.serialize_field("owner_kind", &self.owner_kind)?;
        state.serialize_field("owner_id", &self.owner_id)?;
        state.serialize_field("source_doc_id", &self.source_doc_id)?;
        state.serialize_field("parent_owner_id", &self.parent_owner_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("language", &self.language)?;
        state.serialize_field("package", &self.package)?;
        state.serialize_field("symbol_kind", &self.symbol_kind)?;
        state.serialize_field("visibility", &self.visibility)?;
        state.serialize_field("source_role", &self.source_role)?;
        state.serialize_field("generated", &self.generated)?;
        state.serialize_field("capability_status", &self.capability_status)?;
        state.serialize_field("raw_fallback_reason", &self.raw_fallback_reason)?;
        state.serialize_field("authority_digest", &self.authority_digest)?;
        state.serialize_field("render_policy_digest", &self.render_policy_digest)?;
        state.serialize_field("card_schema_version", &self.card_schema_version)?;
        state.serialize_field("text", &self.text)?;
        state.end()
    }
}

struct SemanticSourceRecordV1Visitor;

impl<'de> Visitor<'de> for SemanticSourceRecordV1Visitor {
    type Value = SemanticSourceRecordV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticSourceRecordV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut record_id: Option<String> = None;
        let mut corpus_kind: Option<SemanticCorpusKindV1> = None;
        let mut owner_kind: Option<OwnerDocKind> = None;
        let mut owner_id: Option<String> = None;
        let mut source_doc_id: Option<String> = None;
        let mut parent_owner_id: Option<Option<String>> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut language: Option<Option<String>> = None;
        let mut package: Option<Option<String>> = None;
        let mut symbol_kind: Option<Option<String>> = None;
        let mut visibility: Option<Option<String>> = None;
        let mut source_role: Option<SourceRoleV1> = None;
        let mut generated: Option<bool> = None;
        let mut capability_status: Option<CapabilityStatusV1> = None;
        let mut raw_fallback_reason: Option<Option<RawFallbackReasonV1>> = None;
        let mut authority_digest: Option<String> = None;
        let mut render_policy_digest: Option<String> = None;
        let mut card_schema_version: Option<u32> = None;
        let mut text: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "record_id" => {
                    if record_id.is_some() {
                        return Err(de::Error::duplicate_field("record_id"));
                    }
                    record_id = Some(map.next_value()?);
                }
                "corpus_kind" => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
                }
                "owner_kind" => {
                    if owner_kind.is_some() {
                        return Err(de::Error::duplicate_field("owner_kind"));
                    }
                    owner_kind = Some(map.next_value()?);
                }
                "owner_id" => {
                    if owner_id.is_some() {
                        return Err(de::Error::duplicate_field("owner_id"));
                    }
                    owner_id = Some(map.next_value()?);
                }
                "source_doc_id" => {
                    if source_doc_id.is_some() {
                        return Err(de::Error::duplicate_field("source_doc_id"));
                    }
                    source_doc_id = Some(map.next_value()?);
                }
                "parent_owner_id" => {
                    if parent_owner_id.is_some() {
                        return Err(de::Error::duplicate_field("parent_owner_id"));
                    }
                    parent_owner_id = Some(map.next_value()?);
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
                "package" => {
                    if package.is_some() {
                        return Err(de::Error::duplicate_field("package"));
                    }
                    package = Some(map.next_value()?);
                }
                "symbol_kind" => {
                    if symbol_kind.is_some() {
                        return Err(de::Error::duplicate_field("symbol_kind"));
                    }
                    symbol_kind = Some(map.next_value()?);
                }
                "visibility" => {
                    if visibility.is_some() {
                        return Err(de::Error::duplicate_field("visibility"));
                    }
                    visibility = Some(map.next_value()?);
                }
                "source_role" => {
                    if source_role.is_some() {
                        return Err(de::Error::duplicate_field("source_role"));
                    }
                    source_role = Some(map.next_value()?);
                }
                "generated" => {
                    if generated.is_some() {
                        return Err(de::Error::duplicate_field("generated"));
                    }
                    generated = Some(map.next_value()?);
                }
                "capability_status" => {
                    if capability_status.is_some() {
                        return Err(de::Error::duplicate_field("capability_status"));
                    }
                    capability_status = Some(map.next_value()?);
                }
                "raw_fallback_reason" => {
                    if raw_fallback_reason.is_some() {
                        return Err(de::Error::duplicate_field("raw_fallback_reason"));
                    }
                    raw_fallback_reason = Some(map.next_value()?);
                }
                "authority_digest" => {
                    if authority_digest.is_some() {
                        return Err(de::Error::duplicate_field("authority_digest"));
                    }
                    authority_digest = Some(map.next_value()?);
                }
                "render_policy_digest" => {
                    if render_policy_digest.is_some() {
                        return Err(de::Error::duplicate_field("render_policy_digest"));
                    }
                    render_policy_digest = Some(map.next_value()?);
                }
                "card_schema_version" => {
                    if card_schema_version.is_some() {
                        return Err(de::Error::duplicate_field("card_schema_version"));
                    }
                    card_schema_version = Some(map.next_value()?);
                }
                "text" => {
                    if text.is_some() {
                        return Err(de::Error::duplicate_field("text"));
                    }
                    text = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, SEMANTIC_SOURCE_RECORD_V1_FIELDS));
                }
            }
        }
        let record = SemanticSourceRecordV1 {
            record_id: record_id.ok_or_else(|| de::Error::missing_field("record_id"))?,
            corpus_kind: corpus_kind.ok_or_else(|| de::Error::missing_field("corpus_kind"))?,
            owner_kind: owner_kind.ok_or_else(|| de::Error::missing_field("owner_kind"))?,
            owner_id: owner_id.ok_or_else(|| de::Error::missing_field("owner_id"))?,
            source_doc_id: source_doc_id
                .ok_or_else(|| de::Error::missing_field("source_doc_id"))?,
            parent_owner_id: parent_owner_id.unwrap_or(None),
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            language: language.unwrap_or(None),
            package: package.unwrap_or(None),
            symbol_kind: symbol_kind.unwrap_or(None),
            visibility: visibility.unwrap_or(None),
            source_role: source_role.ok_or_else(|| de::Error::missing_field("source_role"))?,
            generated: generated.ok_or_else(|| de::Error::missing_field("generated"))?,
            capability_status: capability_status
                .ok_or_else(|| de::Error::missing_field("capability_status"))?,
            raw_fallback_reason: raw_fallback_reason.unwrap_or(None),
            authority_digest: authority_digest
                .ok_or_else(|| de::Error::missing_field("authority_digest"))?,
            render_policy_digest: render_policy_digest
                .ok_or_else(|| de::Error::missing_field("render_policy_digest"))?,
            card_schema_version: card_schema_version
                .ok_or_else(|| de::Error::missing_field("card_schema_version"))?,
            text: text.ok_or_else(|| de::Error::missing_field("text"))?,
        };
        validate_semantic_source_record_v1(&record).map_err(de::Error::custom)?;
        Ok(record)
    }
}

impl<'de> Deserialize<'de> for SemanticSourceRecordV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticSourceRecordV1",
            SEMANTIC_SOURCE_RECORD_V1_FIELDS,
            SemanticSourceRecordV1Visitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterMembershipReplaceV1 {
    pub cluster_record_id: String,
    pub authority_digest: String,
    pub members: Vec<SymbolId>,
}

impl ClusterMembershipReplaceV1 {
    pub fn validate_v1(&self) -> Result<(), &'static str> {
        if self.cluster_record_id.is_empty() {
            return Err("cluster membership cluster_record_id must not be empty");
        }
        if self.authority_digest.is_empty() {
            return Err("cluster membership authority_digest must not be empty");
        }
        if self.members.is_empty() {
            return Err("cluster membership members must not be empty");
        }
        let Ok(member_count) = u32::try_from(self.members.len()) else {
            return Err("cluster membership members exceed the bounded cardinality");
        };
        if member_count > MAX_CLUSTER_MEMBERSHIP_READ_V1 {
            return Err("cluster membership members exceed the bounded cardinality");
        }
        if self.members.iter().any(|member| member.as_str().is_empty()) {
            return Err("cluster membership member identity must not be empty");
        }
        match first_canonical_order_break_v1(&self.members, |member| member.as_str()) {
            Some(CanonicalOrderBreakV1::Duplicate) => {
                Err("cluster membership member identities must be duplicate-free")
            }
            Some(CanonicalOrderBreakV1::OutOfOrder) => {
                Err("cluster membership member identities must use canonical order")
            }
            None => Ok(()),
        }
    }
}

#[must_use]
pub fn cluster_membership_content_digest_v1(members: &[SymbolId]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index:cluster-membership-content:v1\0");
    for member in members {
        let bytes = member.as_str().as_bytes();
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(
        "sha256:"
            .len()
            .saturating_add(digest.len().saturating_mul(2)),
    );
    encoded.push_str("sha256:");
    for byte in digest {
        let _written = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

const CLUSTER_MEMBERSHIP_REPLACE_V1_FIELDS: &[&str] =
    &["cluster_record_id", "authority_digest", "members"];

impl Serialize for ClusterMembershipReplaceV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("ClusterMembershipReplaceV1", 3)?;
        state.serialize_field("cluster_record_id", &self.cluster_record_id)?;
        state.serialize_field("authority_digest", &self.authority_digest)?;
        state.serialize_field("members", &self.members)?;
        state.end()
    }
}

struct ClusterMembershipReplaceV1Visitor;

impl<'de> Visitor<'de> for ClusterMembershipReplaceV1Visitor {
    type Value = ClusterMembershipReplaceV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ClusterMembershipReplaceV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut cluster_record_id = None;
        let mut authority_digest = None;
        let mut members: Option<BoundedClusterMembersV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "cluster_record_id" => {
                    if cluster_record_id.is_some() {
                        return Err(de::Error::duplicate_field("cluster_record_id"));
                    }
                    cluster_record_id = Some(map.next_value()?);
                }
                "authority_digest" => {
                    if authority_digest.is_some() {
                        return Err(de::Error::duplicate_field("authority_digest"));
                    }
                    authority_digest = Some(map.next_value()?);
                }
                "members" => {
                    if members.is_some() {
                        return Err(de::Error::duplicate_field("members"));
                    }
                    members = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        CLUSTER_MEMBERSHIP_REPLACE_V1_FIELDS,
                    ));
                }
            }
        }
        let replacement = ClusterMembershipReplaceV1 {
            cluster_record_id: cluster_record_id
                .ok_or_else(|| de::Error::missing_field("cluster_record_id"))?,
            authority_digest: authority_digest
                .ok_or_else(|| de::Error::missing_field("authority_digest"))?,
            members: members
                .ok_or_else(|| de::Error::missing_field("members"))?
                .into_inner(),
        };
        replacement.validate_v1().map_err(de::Error::custom)?;
        Ok(replacement)
    }
}

impl<'de> Deserialize<'de> for ClusterMembershipReplaceV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ClusterMembershipReplaceV1",
            CLUSTER_MEMBERSHIP_REPLACE_V1_FIELDS,
            ClusterMembershipReplaceV1Visitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticSourceReplaceScopeV1 {
    pub scope: SemanticSourceScopeKeyV1,
    pub scope_digest: String,
    pub sources: Vec<SemanticSourceRecordV1>,
    /// Structured `ClusterCard` membership sealed with this semantic scope.
    /// This is authority data, never a rendered-card parser input.
    pub cluster_memberships: Vec<ClusterMembershipReplaceV1>,
}

const SEMANTIC_SOURCE_REPLACE_SCOPE_V1_FIELDS: &[&str] =
    &["scope", "scope_digest", "sources", "cluster_memberships"];

impl Serialize for SemanticSourceReplaceScopeV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticSourceReplaceScopeV1", 4)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("scope_digest", &self.scope_digest)?;
        state.serialize_field("sources", &self.sources)?;
        state.serialize_field("cluster_memberships", &self.cluster_memberships)?;
        state.end()
    }
}

struct SemanticSourceReplaceScopeV1Visitor;

impl<'de> Visitor<'de> for SemanticSourceReplaceScopeV1Visitor {
    type Value = SemanticSourceReplaceScopeV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticSourceReplaceScopeV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SemanticSourceScopeKeyV1> = None;
        let mut scope_digest: Option<String> = None;
        let mut sources: Option<Vec<SemanticSourceRecordV1>> = None;
        let mut cluster_memberships: Option<Vec<ClusterMembershipReplaceV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                "scope_digest" => {
                    if scope_digest.is_some() {
                        return Err(de::Error::duplicate_field("scope_digest"));
                    }
                    scope_digest = Some(map.next_value()?);
                }
                "sources" => {
                    if sources.is_some() {
                        return Err(de::Error::duplicate_field("sources"));
                    }
                    sources = Some(map.next_value()?);
                }
                "cluster_memberships" => {
                    if cluster_memberships.is_some() {
                        return Err(de::Error::duplicate_field("cluster_memberships"));
                    }
                    cluster_memberships = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_SOURCE_REPLACE_SCOPE_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticSourceReplaceScopeV1 {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
            scope_digest: scope_digest.ok_or_else(|| de::Error::missing_field("scope_digest"))?,
            sources: sources.ok_or_else(|| de::Error::missing_field("sources"))?,
            // Bounded legacy wire compatibility: prior semantic-source batches
            // did not carry structured membership. Current-format builders
            // still reject a ClusterCard replacement without this authority.
            cluster_memberships: cluster_memberships.unwrap_or_default(),
        })
    }
}

impl<'de> Deserialize<'de> for SemanticSourceReplaceScopeV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticSourceReplaceScopeV1",
            SEMANTIC_SOURCE_REPLACE_SCOPE_V1_FIELDS,
            SemanticSourceReplaceScopeV1Visitor,
        )
    }
}

pub fn validate_semantic_source_record_v1(
    record: &SemanticSourceRecordV1,
) -> Result<(), &'static str> {
    if record.record_id.is_empty() {
        return Err("semantic source record_id must not be empty");
    }
    if record.owner_id.is_empty() {
        return Err("semantic source owner_id must not be empty");
    }
    if record.text.is_empty() {
        return Err("semantic source text must not be empty");
    }
    if record.authority_digest.is_empty() {
        return Err("semantic source authority_digest must not be empty");
    }
    if record.render_policy_digest.is_empty() {
        return Err("semantic source render_policy_digest must not be empty");
    }
    if record.corpus_kind == SemanticCorpusKindV1::SymbolCard
        && record.source_role == SourceRoleV1::RawFallbackText
    {
        return Err("SymbolCard records must not use RawFallbackText");
    }
    if record.corpus_kind == SemanticCorpusKindV1::SymbolCard
        && record.source_role != SourceRoleV1::CardText
    {
        return Err("SymbolCard records require CardText source_role");
    }
    if record.corpus_kind == SemanticCorpusKindV1::RawCodeFallback {
        if record.raw_fallback_reason.is_none() {
            return Err("RawCodeFallback records require raw_fallback_reason");
        }
        let has_parent_owner_id = record
            .parent_owner_id
            .as_deref()
            .is_some_and(|value| !value.is_empty());
        if !has_parent_owner_id {
            return Err("RawCodeFallback records require non-empty parent_owner_id");
        }
    }
    Ok(())
}
