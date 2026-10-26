use core::fmt;
use core::fmt::Write as _;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, MapAccess, Visitor},
    ser::SerializeStruct,
};
use sha2::{Digest as _, Sha256};

use quanta_index_contract_base::{
    AdmittedOptionalSeedV1, AdmittedSequenceSeedV1, AdmittedTextSeedV1, DecodeBackingAdmissionV1,
};

use crate::bounded_cluster_members::{BoundedClusterMembersV1, admitted_cluster_members_seed_v1};
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

impl SemanticSourceScopeKeyV1 {
    /// Decode this wire scope under the caller's retained text admission.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        AdmittedSemanticScopeKeyPolicyV1 {
            admission,
            max_text_bytes,
        }
    }
}

const SEMANTIC_SOURCE_SCOPE_KEY_V1_FIELDS: &[&str] = &["corpus_kind", "owner_kind", "owner_id"];

#[derive(Clone, Copy)]
enum SemanticScopeKeyFieldV1 {
    CorpusKind,
    OwnerKind,
    OwnerId,
}

fn semantic_scope_key_field_v1<E: de::Error>(key: &str) -> Result<SemanticScopeKeyFieldV1, E> {
    match key {
        "corpus_kind" => Ok(SemanticScopeKeyFieldV1::CorpusKind),
        "owner_kind" => Ok(SemanticScopeKeyFieldV1::OwnerKind),
        "owner_id" => Ok(SemanticScopeKeyFieldV1::OwnerId),
        other => Err(E::unknown_field(other, SEMANTIC_SOURCE_SCOPE_KEY_V1_FIELDS)),
    }
}

struct SemanticScopeKeyFieldSeedV1;

impl<'de> DeserializeSeed<'de> for SemanticScopeKeyFieldSeedV1 {
    type Value = SemanticScopeKeyFieldV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for SemanticScopeKeyFieldSeedV1 {
    type Value = SemanticScopeKeyFieldV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed SemanticSourceScopeKeyV1 field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        semantic_scope_key_field_v1(key)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted semantic scope key must be borrowed"))
    }
}

trait SemanticScopeKeyDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SemanticScopeKeyFieldV1>, M::Error>;
    fn next_owner_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error>;
}

struct OrdinarySemanticScopeKeyPolicyV1;

impl<'de> SemanticScopeKeyDecodePolicyV1<'de> for OrdinarySemanticScopeKeyPolicyV1 {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SemanticScopeKeyFieldV1>, M::Error> {
        map.next_key::<String>()?
            .map(|key| semantic_scope_key_field_v1(key.as_str()))
            .transpose()
    }

    fn next_owner_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value()
    }
}

struct AdmittedSemanticScopeKeyPolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> SemanticScopeKeyDecodePolicyV1<'de>
    for AdmittedSemanticScopeKeyPolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SemanticScopeKeyFieldV1>, M::Error> {
        map.next_key_seed(SemanticScopeKeyFieldSeedV1)
    }

    fn next_owner_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value_seed(AdmittedTextSeedV1::new(self.admission, self.max_text_bytes))
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedSemanticScopeKeyPolicyV1<'_, A>
{
    type Value = SemanticSourceScopeKeyV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "SemanticSourceScopeKeyV1",
            SEMANTIC_SOURCE_SCOPE_KEY_V1_FIELDS,
            SemanticSourceScopeKeyV1Visitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

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

struct SemanticSourceScopeKeyV1Visitor<P>(P);

impl<'de, P: SemanticScopeKeyDecodePolicyV1<'de>> Visitor<'de>
    for SemanticSourceScopeKeyV1Visitor<P>
{
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
        while let Some(key) = self.0.next_field(&mut map)? {
            match key {
                SemanticScopeKeyFieldV1::CorpusKind => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
                }
                SemanticScopeKeyFieldV1::OwnerKind => {
                    if owner_kind.is_some() {
                        return Err(de::Error::duplicate_field("owner_kind"));
                    }
                    owner_kind = Some(map.next_value()?);
                }
                SemanticScopeKeyFieldV1::OwnerId => {
                    if owner_id.is_some() {
                        return Err(de::Error::duplicate_field("owner_id"));
                    }
                    owner_id = Some(self.0.next_owner_id(&mut map)?);
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
            SemanticSourceScopeKeyV1Visitor(OrdinarySemanticScopeKeyPolicyV1),
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

impl SemanticSourceRecordV1 {
    /// Decode this source row under one retained owner. The large `text` field
    /// has its own byte ceiling; all other text uses `max_text_bytes`.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
        max_record_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        AdmittedSemanticRecordPolicyV1 {
            admission,
            max_text_bytes,
            max_record_bytes,
        }
    }
}

#[derive(Clone, Copy)]
enum SemanticRecordFieldV1 {
    RecordId,
    CorpusKind,
    OwnerKind,
    OwnerId,
    SourceDocId,
    ParentOwnerId,
    RepoRelativePath,
    Language,
    Package,
    SymbolKind,
    Visibility,
    SourceRole,
    Generated,
    CapabilityStatus,
    RawFallbackReason,
    AuthorityDigest,
    RenderPolicyDigest,
    CardSchemaVersion,
    Text,
}

fn semantic_record_field_v1<E: de::Error>(key: &str) -> Result<SemanticRecordFieldV1, E> {
    match key {
        "record_id" => Ok(SemanticRecordFieldV1::RecordId),
        "corpus_kind" => Ok(SemanticRecordFieldV1::CorpusKind),
        "owner_kind" => Ok(SemanticRecordFieldV1::OwnerKind),
        "owner_id" => Ok(SemanticRecordFieldV1::OwnerId),
        "source_doc_id" => Ok(SemanticRecordFieldV1::SourceDocId),
        "parent_owner_id" => Ok(SemanticRecordFieldV1::ParentOwnerId),
        "repo_relative_path" => Ok(SemanticRecordFieldV1::RepoRelativePath),
        "language" => Ok(SemanticRecordFieldV1::Language),
        "package" => Ok(SemanticRecordFieldV1::Package),
        "symbol_kind" => Ok(SemanticRecordFieldV1::SymbolKind),
        "visibility" => Ok(SemanticRecordFieldV1::Visibility),
        "source_role" => Ok(SemanticRecordFieldV1::SourceRole),
        "generated" => Ok(SemanticRecordFieldV1::Generated),
        "capability_status" => Ok(SemanticRecordFieldV1::CapabilityStatus),
        "raw_fallback_reason" => Ok(SemanticRecordFieldV1::RawFallbackReason),
        "authority_digest" => Ok(SemanticRecordFieldV1::AuthorityDigest),
        "render_policy_digest" => Ok(SemanticRecordFieldV1::RenderPolicyDigest),
        "card_schema_version" => Ok(SemanticRecordFieldV1::CardSchemaVersion),
        "text" => Ok(SemanticRecordFieldV1::Text),
        other => Err(E::unknown_field(other, SEMANTIC_SOURCE_RECORD_V1_FIELDS)),
    }
}

struct SemanticRecordFieldSeedV1;

impl<'de> DeserializeSeed<'de> for SemanticRecordFieldSeedV1 {
    type Value = SemanticRecordFieldV1;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for SemanticRecordFieldSeedV1 {
    type Value = SemanticRecordFieldV1;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed SemanticSourceRecordV1 field name")
    }
    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        semantic_record_field_v1(key)
    }
    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted semantic record key must be borrowed"))
    }
}

trait SemanticRecordDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SemanticRecordFieldV1>, M::Error>;
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error>;
    fn next_record_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error>;
    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<String>, M::Error>;
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error>;
}

struct OrdinarySemanticRecordPolicyV1;

impl<'de> SemanticRecordDecodePolicyV1<'de> for OrdinarySemanticRecordPolicyV1 {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SemanticRecordFieldV1>, M::Error> {
        map.next_key::<String>()?
            .map(|key| semantic_record_field_v1(key.as_str()))
            .transpose()
    }
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value()
    }
    fn next_record_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value()
    }
    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<String>, M::Error> {
        map.next_value()
    }
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value()
    }
}

struct AdmittedSemanticRecordPolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
    max_record_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> SemanticRecordDecodePolicyV1<'de>
    for AdmittedSemanticRecordPolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SemanticRecordFieldV1>, M::Error> {
        map.next_key_seed(SemanticRecordFieldSeedV1)
    }
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value_seed(AdmittedTextSeedV1::new(self.admission, self.max_text_bytes))
    }
    fn next_record_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value_seed(AdmittedTextSeedV1::new(
            self.admission,
            self.max_record_bytes,
        ))
    }
    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<String>, M::Error> {
        map.next_value_seed(AdmittedOptionalSeedV1::new(
            self.admission,
            AdmittedTextSeedV1::new(self.admission, self.max_text_bytes),
        ))
    }
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value_seed(RepoRelativePath::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedSemanticRecordPolicyV1<'_, A>
{
    type Value = SemanticSourceRecordV1;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "SemanticSourceRecordV1",
            SEMANTIC_SOURCE_RECORD_V1_FIELDS,
            SemanticSourceRecordV1Visitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

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

struct SemanticSourceRecordV1Visitor<P>(P);

impl<'de, P: SemanticRecordDecodePolicyV1<'de>> Visitor<'de> for SemanticSourceRecordV1Visitor<P> {
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
        while let Some(key) = self.0.next_field(&mut map)? {
            match key {
                SemanticRecordFieldV1::RecordId => {
                    if record_id.is_some() {
                        return Err(de::Error::duplicate_field("record_id"));
                    }
                    record_id = Some(self.0.next_text(&mut map)?);
                }
                SemanticRecordFieldV1::CorpusKind => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
                }
                SemanticRecordFieldV1::OwnerKind => {
                    if owner_kind.is_some() {
                        return Err(de::Error::duplicate_field("owner_kind"));
                    }
                    owner_kind = Some(map.next_value()?);
                }
                SemanticRecordFieldV1::OwnerId => {
                    if owner_id.is_some() {
                        return Err(de::Error::duplicate_field("owner_id"));
                    }
                    owner_id = Some(self.0.next_text(&mut map)?);
                }
                SemanticRecordFieldV1::SourceDocId => {
                    if source_doc_id.is_some() {
                        return Err(de::Error::duplicate_field("source_doc_id"));
                    }
                    source_doc_id = Some(self.0.next_text(&mut map)?);
                }
                SemanticRecordFieldV1::ParentOwnerId => {
                    if parent_owner_id.is_some() {
                        return Err(de::Error::duplicate_field("parent_owner_id"));
                    }
                    parent_owner_id = Some(self.0.next_optional_text(&mut map)?);
                }
                SemanticRecordFieldV1::RepoRelativePath => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(self.0.next_path(&mut map)?);
                }
                SemanticRecordFieldV1::Language => {
                    if language.is_some() {
                        return Err(de::Error::duplicate_field("language"));
                    }
                    language = Some(self.0.next_optional_text(&mut map)?);
                }
                SemanticRecordFieldV1::Package => {
                    if package.is_some() {
                        return Err(de::Error::duplicate_field("package"));
                    }
                    package = Some(self.0.next_optional_text(&mut map)?);
                }
                SemanticRecordFieldV1::SymbolKind => {
                    if symbol_kind.is_some() {
                        return Err(de::Error::duplicate_field("symbol_kind"));
                    }
                    symbol_kind = Some(self.0.next_optional_text(&mut map)?);
                }
                SemanticRecordFieldV1::Visibility => {
                    if visibility.is_some() {
                        return Err(de::Error::duplicate_field("visibility"));
                    }
                    visibility = Some(self.0.next_optional_text(&mut map)?);
                }
                SemanticRecordFieldV1::SourceRole => {
                    if source_role.is_some() {
                        return Err(de::Error::duplicate_field("source_role"));
                    }
                    source_role = Some(map.next_value()?);
                }
                SemanticRecordFieldV1::Generated => {
                    if generated.is_some() {
                        return Err(de::Error::duplicate_field("generated"));
                    }
                    generated = Some(map.next_value()?);
                }
                SemanticRecordFieldV1::CapabilityStatus => {
                    if capability_status.is_some() {
                        return Err(de::Error::duplicate_field("capability_status"));
                    }
                    capability_status = Some(map.next_value()?);
                }
                SemanticRecordFieldV1::RawFallbackReason => {
                    if raw_fallback_reason.is_some() {
                        return Err(de::Error::duplicate_field("raw_fallback_reason"));
                    }
                    raw_fallback_reason = Some(map.next_value()?);
                }
                SemanticRecordFieldV1::AuthorityDigest => {
                    if authority_digest.is_some() {
                        return Err(de::Error::duplicate_field("authority_digest"));
                    }
                    authority_digest = Some(self.0.next_text(&mut map)?);
                }
                SemanticRecordFieldV1::RenderPolicyDigest => {
                    if render_policy_digest.is_some() {
                        return Err(de::Error::duplicate_field("render_policy_digest"));
                    }
                    render_policy_digest = Some(self.0.next_text(&mut map)?);
                }
                SemanticRecordFieldV1::CardSchemaVersion => {
                    if card_schema_version.is_some() {
                        return Err(de::Error::duplicate_field("card_schema_version"));
                    }
                    card_schema_version = Some(map.next_value()?);
                }
                SemanticRecordFieldV1::Text => {
                    if text.is_some() {
                        return Err(de::Error::duplicate_field("text"));
                    }
                    text = Some(self.0.next_record_text(&mut map)?);
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
            parent_owner_id: parent_owner_id
                .ok_or_else(|| de::Error::missing_field("parent_owner_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            language: language.ok_or_else(|| de::Error::missing_field("language"))?,
            package: package.ok_or_else(|| de::Error::missing_field("package"))?,
            symbol_kind: symbol_kind.ok_or_else(|| de::Error::missing_field("symbol_kind"))?,
            visibility: visibility.ok_or_else(|| de::Error::missing_field("visibility"))?,
            source_role: source_role.ok_or_else(|| de::Error::missing_field("source_role"))?,
            generated: generated.ok_or_else(|| de::Error::missing_field("generated"))?,
            capability_status: capability_status
                .ok_or_else(|| de::Error::missing_field("capability_status"))?,
            raw_fallback_reason: raw_fallback_reason
                .ok_or_else(|| de::Error::missing_field("raw_fallback_reason"))?,
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
            SemanticSourceRecordV1Visitor(OrdinarySemanticRecordPolicyV1),
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
    /// Decode this same wire record using retained text and member admission.
    /// The caller supplies a controlled parser and keeps its backing owner live
    /// through the decoded record. A failed decode ends that owner's attempt.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        AdmittedClusterMembershipPolicyV1 {
            admission,
            max_text_bytes,
        }
    }

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

#[derive(Clone, Copy)]
enum ClusterMembershipFieldV1 {
    ClusterRecordId,
    AuthorityDigest,
    Members,
}

struct ClusterMembershipFieldSeedV1;

impl<'de> DeserializeSeed<'de> for ClusterMembershipFieldSeedV1 {
    type Value = ClusterMembershipFieldV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for ClusterMembershipFieldSeedV1 {
    type Value = ClusterMembershipFieldV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ClusterMembershipReplaceV1 field name")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        match value {
            "cluster_record_id" => Ok(ClusterMembershipFieldV1::ClusterRecordId),
            "authority_digest" => Ok(ClusterMembershipFieldV1::AuthorityDigest),
            "members" => Ok(ClusterMembershipFieldV1::Members),
            other => Err(E::unknown_field(
                other,
                CLUSTER_MEMBERSHIP_REPLACE_V1_FIELDS,
            )),
        }
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        Err(E::invalid_type(de::Unexpected::Bytes(value), &self))
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        // A controlled CBOR identifier is borrowed from parser scratch. An
        // owned key would outlive neither its parser backing nor its grant.
        Err(E::custom(
            "admitted cluster membership key must be borrowed",
        ))
    }
}

trait ClusterMembershipDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ClusterMembershipFieldV1>, M::Error>;

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error>;

    fn next_members<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<SymbolId>, M::Error>;
}

struct OrdinaryClusterMembershipPolicyV1;

impl<'de> ClusterMembershipDecodePolicyV1<'de> for OrdinaryClusterMembershipPolicyV1 {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ClusterMembershipFieldV1>, M::Error> {
        let Some(key) = map.next_key::<String>()? else {
            return Ok(None);
        };
        match key.as_str() {
            "cluster_record_id" => Ok(Some(ClusterMembershipFieldV1::ClusterRecordId)),
            "authority_digest" => Ok(Some(ClusterMembershipFieldV1::AuthorityDigest)),
            "members" => Ok(Some(ClusterMembershipFieldV1::Members)),
            other => Err(de::Error::unknown_field(
                other,
                CLUSTER_MEMBERSHIP_REPLACE_V1_FIELDS,
            )),
        }
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value()
    }

    fn next_members<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<SymbolId>, M::Error> {
        let members: BoundedClusterMembersV1 = map.next_value()?;
        Ok(members.into_inner())
    }
}

struct AdmittedClusterMembershipPolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> ClusterMembershipDecodePolicyV1<'de>
    for AdmittedClusterMembershipPolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ClusterMembershipFieldV1>, M::Error> {
        map.next_key_seed(ClusterMembershipFieldSeedV1)
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value_seed(AdmittedTextSeedV1::new(self.admission, self.max_text_bytes))
    }

    fn next_members<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<SymbolId>, M::Error> {
        map.next_value_seed(admitted_cluster_members_seed_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedClusterMembershipPolicyV1<'_, A>
{
    type Value = ClusterMembershipReplaceV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "ClusterMembershipReplaceV1",
            CLUSTER_MEMBERSHIP_REPLACE_V1_FIELDS,
            ClusterMembershipReplaceV1Visitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

struct ClusterMembershipReplaceV1Visitor<P>(P);

impl<'de, P: ClusterMembershipDecodePolicyV1<'de>> Visitor<'de>
    for ClusterMembershipReplaceV1Visitor<P>
{
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
        let mut members: Option<Vec<SymbolId>> = None;
        while let Some(field) = self.0.next_field(&mut map)? {
            match field {
                ClusterMembershipFieldV1::ClusterRecordId => {
                    if cluster_record_id.is_some() {
                        return Err(de::Error::duplicate_field("cluster_record_id"));
                    }
                    cluster_record_id = Some(self.0.next_text(&mut map)?);
                }
                ClusterMembershipFieldV1::AuthorityDigest => {
                    if authority_digest.is_some() {
                        return Err(de::Error::duplicate_field("authority_digest"));
                    }
                    authority_digest = Some(self.0.next_text(&mut map)?);
                }
                ClusterMembershipFieldV1::Members => {
                    if members.is_some() {
                        return Err(de::Error::duplicate_field("members"));
                    }
                    members = Some(self.0.next_members(&mut map)?);
                }
            }
        }
        let replacement = ClusterMembershipReplaceV1 {
            cluster_record_id: cluster_record_id
                .ok_or_else(|| de::Error::missing_field("cluster_record_id"))?,
            authority_digest: authority_digest
                .ok_or_else(|| de::Error::missing_field("authority_digest"))?,
            members: members.ok_or_else(|| de::Error::missing_field("members"))?,
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
            ClusterMembershipReplaceV1Visitor(OrdinaryClusterMembershipPolicyV1),
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

impl SemanticSourceReplaceScopeV1 {
    /// Decode the existing scope wire with bounded source text and collection
    /// backings. Both source and cluster collections use `max_records`.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
        max_record_bytes: usize,
        max_records: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        AdmittedSemanticReplacePolicyV1 {
            admission,
            max_text_bytes,
            max_record_bytes,
            max_records,
        }
    }
}

const SEMANTIC_SOURCE_REPLACE_SCOPE_V1_FIELDS: &[&str] =
    &["scope", "scope_digest", "sources", "cluster_memberships"];

#[derive(Clone, Copy)]
enum SemanticReplaceFieldV1 {
    Scope,
    ScopeDigest,
    Sources,
    ClusterMemberships,
}

fn semantic_replace_field_v1<E: de::Error>(key: &str) -> Result<SemanticReplaceFieldV1, E> {
    match key {
        "scope" => Ok(SemanticReplaceFieldV1::Scope),
        "scope_digest" => Ok(SemanticReplaceFieldV1::ScopeDigest),
        "sources" => Ok(SemanticReplaceFieldV1::Sources),
        "cluster_memberships" => Ok(SemanticReplaceFieldV1::ClusterMemberships),
        other => Err(E::unknown_field(
            other,
            SEMANTIC_SOURCE_REPLACE_SCOPE_V1_FIELDS,
        )),
    }
}

struct SemanticReplaceFieldSeedV1;

impl<'de> DeserializeSeed<'de> for SemanticReplaceFieldSeedV1 {
    type Value = SemanticReplaceFieldV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for SemanticReplaceFieldSeedV1 {
    type Value = SemanticReplaceFieldV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed SemanticSourceReplaceScopeV1 field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        semantic_replace_field_v1(key)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted semantic replace key must be borrowed"))
    }
}

trait SemanticReplaceDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SemanticReplaceFieldV1>, M::Error>;
    fn next_scope<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<SemanticSourceScopeKeyV1, M::Error>;
    fn next_digest<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error>;
    fn next_sources<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SemanticSourceRecordV1>, M::Error>;
    fn next_memberships<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<ClusterMembershipReplaceV1>, M::Error>;
}

struct OrdinarySemanticReplacePolicyV1;

impl<'de> SemanticReplaceDecodePolicyV1<'de> for OrdinarySemanticReplacePolicyV1 {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SemanticReplaceFieldV1>, M::Error> {
        map.next_key::<String>()?
            .map(|key| semantic_replace_field_v1(key.as_str()))
            .transpose()
    }

    fn next_scope<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<SemanticSourceScopeKeyV1, M::Error> {
        map.next_value()
    }

    fn next_digest<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value()
    }

    fn next_sources<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SemanticSourceRecordV1>, M::Error> {
        map.next_value()
    }

    fn next_memberships<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<ClusterMembershipReplaceV1>, M::Error> {
        map.next_value()
    }
}

struct AdmittedSemanticReplacePolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
    max_record_bytes: usize,
    max_records: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> SemanticReplaceDecodePolicyV1<'de>
    for AdmittedSemanticReplacePolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<SemanticReplaceFieldV1>, M::Error> {
        map.next_key_seed(SemanticReplaceFieldSeedV1)
    }

    fn next_scope<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<SemanticSourceScopeKeyV1, M::Error> {
        map.next_value_seed(
            SemanticSourceScopeKeyV1::deserialize_seed_with_admission_v1(
                self.admission,
                self.max_text_bytes,
            ),
        )
    }

    fn next_digest<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value_seed(AdmittedTextSeedV1::new(self.admission, self.max_text_bytes))
    }

    fn next_sources<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SemanticSourceRecordV1>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.max_records,
            || {
                SemanticSourceRecordV1::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.max_text_bytes,
                    self.max_record_bytes,
                )
            },
        ))
    }

    fn next_memberships<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<ClusterMembershipReplaceV1>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.max_records,
            || {
                ClusterMembershipReplaceV1::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.max_text_bytes,
                )
            },
        ))
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedSemanticReplacePolicyV1<'_, A>
{
    type Value = SemanticSourceReplaceScopeV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "SemanticSourceReplaceScopeV1",
            SEMANTIC_SOURCE_REPLACE_SCOPE_V1_FIELDS,
            SemanticSourceReplaceScopeV1Visitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

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

struct SemanticSourceReplaceScopeV1Visitor<P>(P);

impl<'de, P: SemanticReplaceDecodePolicyV1<'de>> Visitor<'de>
    for SemanticSourceReplaceScopeV1Visitor<P>
{
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
        while let Some(key) = self.0.next_field(&mut map)? {
            match key {
                SemanticReplaceFieldV1::Scope => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(self.0.next_scope(&mut map)?);
                }
                SemanticReplaceFieldV1::ScopeDigest => {
                    if scope_digest.is_some() {
                        return Err(de::Error::duplicate_field("scope_digest"));
                    }
                    scope_digest = Some(self.0.next_digest(&mut map)?);
                }
                SemanticReplaceFieldV1::Sources => {
                    if sources.is_some() {
                        return Err(de::Error::duplicate_field("sources"));
                    }
                    sources = Some(self.0.next_sources(&mut map)?);
                }
                SemanticReplaceFieldV1::ClusterMemberships => {
                    if cluster_memberships.is_some() {
                        return Err(de::Error::duplicate_field("cluster_memberships"));
                    }
                    cluster_memberships = Some(self.0.next_memberships(&mut map)?);
                }
            }
        }
        Ok(SemanticSourceReplaceScopeV1 {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
            scope_digest: scope_digest.ok_or_else(|| de::Error::missing_field("scope_digest"))?,
            sources: sources.ok_or_else(|| de::Error::missing_field("sources"))?,
            cluster_memberships: cluster_memberships
                .ok_or_else(|| de::Error::missing_field("cluster_memberships"))?,
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
            SemanticSourceReplaceScopeV1Visitor(OrdinarySemanticReplacePolicyV1),
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

#[cfg(test)]
mod admitted_source_tests;

#[cfg(test)]
mod admitted_cluster_membership_tests {
    use quanta_index_contract_base::{DecodeBackingAdmissionV1, DecodeBackingRefusalV1};
    use serde::de::DeserializeSeed;

    use super::ClusterMembershipReplaceV1;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    // These fixtures check typed decode policy, not physical memory custody.
    struct TestAdmission {
        refuse: bool,
        attempt: std::cell::RefCell<quanta_index_contract_base::DecodeAttemptStateV1>,
    }

    impl DecodeBackingAdmissionV1 for TestAdmission {
        fn abort_decode(&self) {
            self.attempt.borrow_mut().abort_v1();
        }

        fn enter_decode_field_v1(
            &self,
            kind: quanta_index_contract_base::DecodeFieldKindV1,
            maximum: usize,
        ) -> Result<quanta_index_contract_base::DecodeFieldTokenV1, DecodeBackingRefusalV1>
        {
            let mut attempt = self.attempt.borrow_mut();
            if !attempt.is_reading_v1() {
                attempt.begin_decode_v1()?;
            }
            attempt.enter_field_v1(kind, maximum)
        }

        fn leave_decode_field_v1(
            &self,
            token: quanta_index_contract_base::DecodeFieldTokenV1,
        ) -> Result<(), DecodeBackingRefusalV1> {
            self.attempt.borrow_mut().leave_field_v1(token)
        }

        fn reserve_text(
            &self,
            destination: &mut String,
            additional: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            if self.refuse || destination.try_reserve_exact(additional).is_err() {
                return Err(DecodeBackingRefusalV1);
            }
            Ok(())
        }

        fn reserve_bytes(
            &self,
            _destination: &mut Vec<u8>,
            _additional: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            Err(DecodeBackingRefusalV1)
        }

        fn reserve_elements<T>(
            &self,
            destination: &mut Vec<T>,
            additional: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            if self.refuse || destination.try_reserve_exact(additional).is_err() {
                return Err(DecodeBackingRefusalV1);
            }
            Ok(())
        }

        fn copy_boxed_text(&self, _source: &str) -> Result<Box<str>, DecodeBackingRefusalV1> {
            Err(DecodeBackingRefusalV1)
        }

        fn adopt_parser_text<E>(
            &self,
            source: &mut Option<String>,
            validate: impl FnOnce(&str) -> Result<(), E>,
        ) -> Result<String, crate::ParserTextInputErrorV1<E>> {
            if self.refuse {
                return Err(DecodeBackingRefusalV1.into());
            }
            let value = source
                .as_deref()
                .ok_or(crate::ParserTextInputErrorV1::Admission(
                    DecodeBackingRefusalV1,
                ))?;
            validate(value).map_err(crate::ParserTextInputErrorV1::Validation)?;
            source
                .take()
                .ok_or(crate::ParserTextInputErrorV1::Admission(
                    DecodeBackingRefusalV1,
                ))
        }

        fn copy_boxed_parser_text<E>(
            &self,
            _source: &mut Option<String>,
            _validate: impl FnOnce(&str) -> Result<(), E>,
        ) -> Result<Box<str>, crate::ParserTextInputErrorV1<E>> {
            Err(DecodeBackingRefusalV1.into())
        }

        fn verify_parser_owned_bytes(
            &self,
            _source: &[u8],
            _capacity: usize,
        ) -> Result<(), DecodeBackingRefusalV1> {
            Err(DecodeBackingRefusalV1)
        }
    }

    fn admitted(
        wire: &str,
        owner: &TestAdmission,
    ) -> Result<ClusterMembershipReplaceV1, serde_json::Error> {
        let mut decoder = serde_json::Deserializer::from_str(wire);
        ClusterMembershipReplaceV1::deserialize_seed_with_admission_v1(owner, 64)
            .deserialize(&mut decoder)
    }

    #[test]
    fn admitted_membership_matches_fixed_ordinary_record() -> TestResult {
        let wire = r#"{"cluster_record_id":"cluster:one","authority_digest":"sha256:fixed","members":["symbol:a","symbol:b"]}"#;
        let owner = TestAdmission {
            refuse: false,
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        let admitted = admitted(wire, &owner)?;
        let ordinary: ClusterMembershipReplaceV1 = serde_json::from_str(wire)?;
        if admitted != ordinary || admitted.members.len() != 2 {
            return Err("admitted membership differs from the fixed ordinary wire".into());
        }
        Ok(())
    }

    #[test]
    fn admitted_membership_preserves_rejection_contract() -> TestResult {
        let owner = TestAdmission {
            refuse: false,
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        for wire in [
            r#"{"cluster_record_id":"cluster:one","authority_digest":"sha256:fixed","members":["symbol:b","symbol:a"]}"#,
            r#"{"cluster_record_id":"cluster:one","authority_digest":"sha256:fixed","members":["symbol:a","symbol:a"]}"#,
            r#"{"cluster_record_id":"cluster:one","authority_digest":"sha256:fixed","members":[]}"#,
            r#"{"cluster_record_id":"cluster:one","authority_digest":"sha256:fixed"}"#,
            r#"{"cluster_record_id":"cluster:one","cluster_record_id":"cluster:two","authority_digest":"sha256:fixed","members":["symbol:a"]}"#,
            r#"{"cluster_record_id":"cluster:one","authority_digest":"sha256:fixed","members":["symbol:a"],"unknown":1}"#,
        ] {
            if admitted(wire, &owner).is_ok()
                || serde_json::from_str::<ClusterMembershipReplaceV1>(wire).is_ok()
            {
                return Err(format!("membership policy accepted invalid wire {wire}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn admitted_membership_refuses_owner_backing() -> TestResult {
        let wire = r#"{"cluster_record_id":"cluster:one","authority_digest":"sha256:fixed","members":["symbol:a"]}"#;
        let owner = TestAdmission {
            refuse: true,
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        if admitted(wire, &owner).is_ok() {
            return Err("owner refusal produced a membership record".into());
        }
        Ok(())
    }

    #[test]
    fn admitted_membership_rejects_the_4097th_member() -> TestResult {
        let members: Vec<String> = (0..=4096)
            .map(|index| format!("symbol:{index:04}"))
            .collect();
        let wire = serde_json::json!({
            "cluster_record_id": "cluster:one",
            "authority_digest": "sha256:fixed",
            "members": members,
        })
        .to_string();
        let owner = TestAdmission {
            refuse: false,
            attempt: std::cell::RefCell::new(
                quanta_index_contract_base::DecodeAttemptStateV1::default(),
            ),
        };
        let Err(error) = admitted(&wire, &owner) else {
            return Err("4097 members escaped the admitted sequence bound".into());
        };
        if !error
            .to_string()
            .contains("admitted sequence exceeds its element limit")
        {
            return Err(format!("4097th member failed for another reason: {error}").into());
        }
        Ok(())
    }
}
