use quanta_index_contract_base::{
    AdmittedBoxedTextSeedV1, AdmittedOptionalSeedV1, DecodeBackingAdmissionV1,
    SourceIdentityDecodeAdmissionV1,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::{LanguageCode, SymbolKindCode};
use crate::semantic_kinds::OwnerDocKind;
use crate::{
    CapabilityStatusV1, ChunkId, EmbeddingId, RepoId, RepoRelativePath, SemanticCorpusKindV1,
    SourceRoleV1,
};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkStructuralMetadata {
    pub variant_tag: Box<str>,
    pub structural_kind_tag: Box<str>,
    pub structural_pattern_kind: Box<str>,
    pub structural_name: Box<str>,
    pub structural_scope: Box<str>,
    pub structural_matched_node: Box<str>,
}

impl ChunkStructuralMetadata {
    /// Decode the existing wire shape while the caller retains the admission
    /// owner for all six boxed text backings.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: DecodeBackingAdmissionV1 + ?Sized,
    {
        AdmittedChunkStructuralMetadataPolicyV1 {
            admission,
            max_text_bytes,
        }
    }
}

const CHUNK_STRUCTURAL_METADATA_FIELDS: &[&str] = &[
    "variant_tag",
    "structural_kind_tag",
    "structural_pattern_kind",
    "structural_name",
    "structural_scope",
    "structural_matched_node",
];

impl Serialize for ChunkStructuralMetadata {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ChunkStructuralMetadata", 6)?;
        state.serialize_field("variant_tag", self.variant_tag.as_ref())?;
        state.serialize_field("structural_kind_tag", self.structural_kind_tag.as_ref())?;
        state.serialize_field(
            "structural_pattern_kind",
            self.structural_pattern_kind.as_ref(),
        )?;
        state.serialize_field("structural_name", self.structural_name.as_ref())?;
        state.serialize_field("structural_scope", self.structural_scope.as_ref())?;
        state.serialize_field(
            "structural_matched_node",
            self.structural_matched_node.as_ref(),
        )?;
        state.end()
    }
}

#[derive(Clone, Copy)]
enum ChunkStructuralFieldV1 {
    VariantTag,
    StructuralKindTag,
    StructuralPatternKind,
    StructuralName,
    StructuralScope,
    StructuralMatchedNode,
}

fn chunk_structural_field_v1<E: de::Error>(key: &str) -> Result<ChunkStructuralFieldV1, E> {
    match key {
        "variant_tag" => Ok(ChunkStructuralFieldV1::VariantTag),
        "structural_kind_tag" => Ok(ChunkStructuralFieldV1::StructuralKindTag),
        "structural_pattern_kind" => Ok(ChunkStructuralFieldV1::StructuralPatternKind),
        "structural_name" => Ok(ChunkStructuralFieldV1::StructuralName),
        "structural_scope" => Ok(ChunkStructuralFieldV1::StructuralScope),
        "structural_matched_node" => Ok(ChunkStructuralFieldV1::StructuralMatchedNode),
        other => Err(E::unknown_field(other, CHUNK_STRUCTURAL_METADATA_FIELDS)),
    }
}

struct ChunkStructuralFieldSeedV1;

impl<'de> DeserializeSeed<'de> for ChunkStructuralFieldSeedV1 {
    type Value = ChunkStructuralFieldV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for ChunkStructuralFieldSeedV1 {
    type Value = ChunkStructuralFieldV1;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("a borrowed ChunkStructuralMetadata field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        chunk_structural_field_v1(key)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted structural field key must be borrowed"))
    }
}

trait ChunkStructuralMetadataDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkStructuralFieldV1>, M::Error>;

    fn next_boxed_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error>;
}

struct OrdinaryChunkStructuralMetadataPolicyV1;

impl<'de> ChunkStructuralMetadataDecodePolicyV1<'de> for OrdinaryChunkStructuralMetadataPolicyV1 {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkStructuralFieldV1>, M::Error> {
        map.next_key::<String>()?
            .map(|key| chunk_structural_field_v1(key.as_str()))
            .transpose()
    }

    fn next_boxed_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        let value: String = map.next_value()?;
        Ok(value.into_boxed_str())
    }
}

struct AdmittedChunkStructuralMetadataPolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> ChunkStructuralMetadataDecodePolicyV1<'de>
    for AdmittedChunkStructuralMetadataPolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkStructuralFieldV1>, M::Error> {
        map.next_key_seed(ChunkStructuralFieldSeedV1)
    }

    fn next_boxed_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        map.next_value_seed(AdmittedBoxedTextSeedV1::new(
            self.admission,
            self.max_text_bytes,
        ))
    }
}

impl<'de, A: DecodeBackingAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedChunkStructuralMetadataPolicyV1<'_, A>
{
    type Value = ChunkStructuralMetadata;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "ChunkStructuralMetadata",
            CHUNK_STRUCTURAL_METADATA_FIELDS,
            ChunkStructuralMetadataVisitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

struct ChunkStructuralMetadataVisitor<P>(P);

impl<'de, P: ChunkStructuralMetadataDecodePolicyV1<'de>> Visitor<'de>
    for ChunkStructuralMetadataVisitor<P>
{
    type Value = ChunkStructuralMetadata;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("a ChunkStructuralMetadata map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut variant_tag = None;
        let mut structural_kind_tag = None;
        let mut structural_pattern_kind = None;
        let mut structural_name = None;
        let mut structural_scope = None;
        let mut structural_matched_node = None;
        while let Some(field) = self.0.next_field(&mut map)? {
            match field {
                ChunkStructuralFieldV1::VariantTag => {
                    if variant_tag.is_some() {
                        return Err(de::Error::duplicate_field("variant_tag"));
                    }
                    variant_tag = Some(self.0.next_boxed_text(&mut map)?);
                }
                ChunkStructuralFieldV1::StructuralKindTag => {
                    if structural_kind_tag.is_some() {
                        return Err(de::Error::duplicate_field("structural_kind_tag"));
                    }
                    structural_kind_tag = Some(self.0.next_boxed_text(&mut map)?);
                }
                ChunkStructuralFieldV1::StructuralPatternKind => {
                    if structural_pattern_kind.is_some() {
                        return Err(de::Error::duplicate_field("structural_pattern_kind"));
                    }
                    structural_pattern_kind = Some(self.0.next_boxed_text(&mut map)?);
                }
                ChunkStructuralFieldV1::StructuralName => {
                    if structural_name.is_some() {
                        return Err(de::Error::duplicate_field("structural_name"));
                    }
                    structural_name = Some(self.0.next_boxed_text(&mut map)?);
                }
                ChunkStructuralFieldV1::StructuralScope => {
                    if structural_scope.is_some() {
                        return Err(de::Error::duplicate_field("structural_scope"));
                    }
                    structural_scope = Some(self.0.next_boxed_text(&mut map)?);
                }
                ChunkStructuralFieldV1::StructuralMatchedNode => {
                    if structural_matched_node.is_some() {
                        return Err(de::Error::duplicate_field("structural_matched_node"));
                    }
                    structural_matched_node = Some(self.0.next_boxed_text(&mut map)?);
                }
            }
        }
        Ok(ChunkStructuralMetadata {
            variant_tag: variant_tag.ok_or_else(|| de::Error::missing_field("variant_tag"))?,
            structural_kind_tag: structural_kind_tag
                .ok_or_else(|| de::Error::missing_field("structural_kind_tag"))?,
            structural_pattern_kind: structural_pattern_kind
                .ok_or_else(|| de::Error::missing_field("structural_pattern_kind"))?,
            structural_name: structural_name
                .ok_or_else(|| de::Error::missing_field("structural_name"))?,
            structural_scope: structural_scope
                .ok_or_else(|| de::Error::missing_field("structural_scope"))?,
            structural_matched_node: structural_matched_node
                .ok_or_else(|| de::Error::missing_field("structural_matched_node"))?,
        })
    }
}

impl<'de> Deserialize<'de> for ChunkStructuralMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ChunkStructuralMetadata",
            CHUNK_STRUCTURAL_METADATA_FIELDS,
            ChunkStructuralMetadataVisitor(OrdinaryChunkStructuralMetadataPolicyV1),
        )
    }
}

#[cfg(test)]
#[path = "records/admitted_structural_tests.rs"]
mod admitted_structural_tests;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkRecord {
    pub chunk_id: ChunkId,
    pub repo_relative_path: RepoRelativePath,
    pub language: LanguageCode,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub text: Box<str>,
    pub structural: Option<ChunkStructuralMetadata>,
    pub parent_chunk_id: Option<ChunkId>,
    /// Producer-carried searchable repo facet for federated chunks in one
    /// generation pin. When absent, the batch `repo_id` is indexed.
    pub source_repo_id: Option<RepoId>,
}

impl ChunkRecord {
    /// Decode the existing wire with every owned text backing admitted by
    /// `admission`. The caller must retain that owner alongside the result.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        AdmittedChunkRecordPolicyV1 {
            admission,
            max_text_bytes,
        }
    }
}

const CHUNK_RECORD_FIELDS: &[&str] = &[
    "chunk_id",
    "repo_relative_path",
    "language",
    "start_byte",
    "end_byte",
    "start_line",
    "end_line",
    "text",
    "structural",
    "parent_chunk_id",
    "source_repo_id",
];

impl Serialize for ChunkRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ChunkRecord", 11)?;
        state.serialize_field("chunk_id", &self.chunk_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("language", &self.language)?;
        state.serialize_field("start_byte", &self.start_byte)?;
        state.serialize_field("end_byte", &self.end_byte)?;
        state.serialize_field("start_line", &self.start_line)?;
        state.serialize_field("end_line", &self.end_line)?;
        state.serialize_field("text", self.text.as_ref())?;
        state.serialize_field("structural", &self.structural)?;
        state.serialize_field("parent_chunk_id", &self.parent_chunk_id)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.end()
    }
}

#[derive(Clone, Copy)]
enum ChunkRecordFieldV1 {
    ChunkId,
    RepoRelativePath,
    Language,
    StartByte,
    EndByte,
    StartLine,
    EndLine,
    Text,
    Structural,
    ParentChunkId,
    SourceRepoId,
}

fn chunk_record_field_v1<E: de::Error>(key: &str) -> Result<ChunkRecordFieldV1, E> {
    match key {
        "chunk_id" => Ok(ChunkRecordFieldV1::ChunkId),
        "repo_relative_path" => Ok(ChunkRecordFieldV1::RepoRelativePath),
        "language" => Ok(ChunkRecordFieldV1::Language),
        "start_byte" => Ok(ChunkRecordFieldV1::StartByte),
        "end_byte" => Ok(ChunkRecordFieldV1::EndByte),
        "start_line" => Ok(ChunkRecordFieldV1::StartLine),
        "end_line" => Ok(ChunkRecordFieldV1::EndLine),
        "text" => Ok(ChunkRecordFieldV1::Text),
        "structural" => Ok(ChunkRecordFieldV1::Structural),
        "parent_chunk_id" => Ok(ChunkRecordFieldV1::ParentChunkId),
        "source_repo_id" => Ok(ChunkRecordFieldV1::SourceRepoId),
        other => Err(E::unknown_field(other, CHUNK_RECORD_FIELDS)),
    }
}

struct ChunkRecordFieldSeedV1;

impl<'de> DeserializeSeed<'de> for ChunkRecordFieldSeedV1 {
    type Value = ChunkRecordFieldV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for ChunkRecordFieldSeedV1 {
    type Value = ChunkRecordFieldV1;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("a borrowed ChunkRecord field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        chunk_record_field_v1(key)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted chunk field key must be borrowed"))
    }
}

trait ChunkRecordDecodePolicyV1<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkRecordFieldV1>, M::Error>;
    fn next_chunk_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ChunkId, M::Error>;
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error>;
    fn next_language<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error>;
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error>;
    fn next_structural<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkStructuralMetadata>, M::Error>;
    fn next_parent_chunk_id<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkId>, M::Error>;
    fn next_source_repo_id<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<RepoId>, M::Error>;
}

struct OrdinaryChunkRecordPolicyV1;

impl<'de> ChunkRecordDecodePolicyV1<'de> for OrdinaryChunkRecordPolicyV1 {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkRecordFieldV1>, M::Error> {
        map.next_key::<String>()?
            .map(|key| chunk_record_field_v1(key.as_str()))
            .transpose()
    }

    fn next_chunk_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ChunkId, M::Error> {
        map.next_value()
    }

    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value()
    }

    fn next_language<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error> {
        map.next_value()
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        let text: String = map.next_value()?;
        Ok(text.into_boxed_str())
    }

    fn next_structural<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkStructuralMetadata>, M::Error> {
        map.next_value()
    }

    fn next_parent_chunk_id<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkId>, M::Error> {
        map.next_value()
    }

    fn next_source_repo_id<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<RepoId>, M::Error> {
        map.next_value()
    }
}

struct AdmittedRepoIdSeedV1<'a, A: ?Sized>(&'a A);

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedRepoIdSeedV1<'_, A>
{
    type Value = RepoId;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        self.0.decode_repo_id_v1(decoder)
    }
}

struct AdmittedChunkRecordPolicyV1<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> ChunkRecordDecodePolicyV1<'de>
    for AdmittedChunkRecordPolicyV1<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkRecordFieldV1>, M::Error> {
        map.next_key_seed(ChunkRecordFieldSeedV1)
    }

    fn next_chunk_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ChunkId, M::Error> {
        map.next_value_seed(ChunkId::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value_seed(RepoRelativePath::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_language<M: MapAccess<'de>>(&self, map: &mut M) -> Result<LanguageCode, M::Error> {
        map.next_value_seed(LanguageCode::deserialize_seed_with_admission_v1(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        map.next_value_seed(AdmittedBoxedTextSeedV1::new(
            self.admission,
            self.max_text_bytes,
        ))
    }

    fn next_structural<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkStructuralMetadata>, M::Error> {
        map.next_value_seed(AdmittedOptionalSeedV1::new(
            self.admission,
            ChunkStructuralMetadata::deserialize_seed_with_admission_v1(
                self.admission,
                self.max_text_bytes,
            ),
        ))
    }

    fn next_parent_chunk_id<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<ChunkId>, M::Error> {
        map.next_value_seed(AdmittedOptionalSeedV1::new(
            self.admission,
            ChunkId::deserialize_seed_with_admission_v1(self.admission, self.max_text_bytes),
        ))
    }

    fn next_source_repo_id<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<RepoId>, M::Error> {
        map.next_value_seed(AdmittedOptionalSeedV1::new(
            self.admission,
            AdmittedRepoIdSeedV1(self.admission),
        ))
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for AdmittedChunkRecordPolicyV1<'_, A>
{
    type Value = ChunkRecord;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.admission;
        let result = decoder.deserialize_struct(
            "ChunkRecord",
            CHUNK_RECORD_FIELDS,
            ChunkRecordVisitor(self),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

struct ChunkRecordVisitor<P>(P);

impl<'de, P: ChunkRecordDecodePolicyV1<'de>> Visitor<'de> for ChunkRecordVisitor<P> {
    type Value = ChunkRecord;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("a ChunkRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut chunk_id: Option<ChunkId> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut language: Option<LanguageCode> = None;
        let mut start_byte: Option<u32> = None;
        let mut end_byte: Option<u32> = None;
        let mut start_line: Option<u32> = None;
        let mut end_line: Option<u32> = None;
        let mut text: Option<Box<str>> = None;
        let mut structural: Option<Option<ChunkStructuralMetadata>> = None;
        let mut parent_chunk_id: Option<Option<ChunkId>> = None;
        let mut source_repo_id: Option<Option<RepoId>> = None;
        while let Some(field) = self.0.next_field(&mut map)? {
            match field {
                ChunkRecordFieldV1::ChunkId => {
                    if chunk_id.is_some() {
                        return Err(de::Error::duplicate_field("chunk_id"));
                    }
                    chunk_id = Some(self.0.next_chunk_id(&mut map)?);
                }
                ChunkRecordFieldV1::RepoRelativePath => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(self.0.next_path(&mut map)?);
                }
                ChunkRecordFieldV1::Language => {
                    if language.is_some() {
                        return Err(de::Error::duplicate_field("language"));
                    }
                    language = Some(self.0.next_language(&mut map)?);
                }
                ChunkRecordFieldV1::StartByte => {
                    if start_byte.is_some() {
                        return Err(de::Error::duplicate_field("start_byte"));
                    }
                    start_byte = Some(map.next_value()?);
                }
                ChunkRecordFieldV1::EndByte => {
                    if end_byte.is_some() {
                        return Err(de::Error::duplicate_field("end_byte"));
                    }
                    end_byte = Some(map.next_value()?);
                }
                ChunkRecordFieldV1::StartLine => {
                    if start_line.is_some() {
                        return Err(de::Error::duplicate_field("start_line"));
                    }
                    start_line = Some(map.next_value()?);
                }
                ChunkRecordFieldV1::EndLine => {
                    if end_line.is_some() {
                        return Err(de::Error::duplicate_field("end_line"));
                    }
                    end_line = Some(map.next_value()?);
                }
                ChunkRecordFieldV1::Text => {
                    if text.is_some() {
                        return Err(de::Error::duplicate_field("text"));
                    }
                    text = Some(self.0.next_text(&mut map)?);
                }
                ChunkRecordFieldV1::Structural => {
                    if structural.is_some() {
                        return Err(de::Error::duplicate_field("structural"));
                    }
                    structural = Some(self.0.next_structural(&mut map)?);
                }
                ChunkRecordFieldV1::ParentChunkId => {
                    if parent_chunk_id.is_some() {
                        return Err(de::Error::duplicate_field("parent_chunk_id"));
                    }
                    parent_chunk_id = Some(self.0.next_parent_chunk_id(&mut map)?);
                }
                ChunkRecordFieldV1::SourceRepoId => {
                    if source_repo_id.is_some() {
                        return Err(de::Error::duplicate_field("source_repo_id"));
                    }
                    source_repo_id = Some(self.0.next_source_repo_id(&mut map)?);
                }
            }
        }
        Ok(ChunkRecord {
            chunk_id: chunk_id.ok_or_else(|| de::Error::missing_field("chunk_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            language: language.ok_or_else(|| de::Error::missing_field("language"))?,
            start_byte: start_byte.ok_or_else(|| de::Error::missing_field("start_byte"))?,
            end_byte: end_byte.ok_or_else(|| de::Error::missing_field("end_byte"))?,
            start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
            end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
            text: text.ok_or_else(|| de::Error::missing_field("text"))?,
            structural: structural.ok_or_else(|| de::Error::missing_field("structural"))?,
            parent_chunk_id: parent_chunk_id
                .ok_or_else(|| de::Error::missing_field("parent_chunk_id"))?,
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for ChunkRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ChunkRecord",
            CHUNK_RECORD_FIELDS,
            ChunkRecordVisitor(OrdinaryChunkRecordPolicyV1),
        )
    }
}

#[cfg(test)]
#[path = "records/admitted_chunk_tests.rs"]
mod admitted_chunk_tests;

impl ChunkRecord {
    #[must_use]
    pub fn derived_snippet(&self) -> &str {
        self.text.as_ref()
    }

    #[must_use]
    pub fn searchable_repo_id<'a>(&'a self, batch_repo_id: &'a RepoId) -> &'a RepoId {
        self.source_repo_id.as_ref().unwrap_or(batch_repo_id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingRecord {
    pub embedding_id: EmbeddingId,
    pub record_id: Box<str>,
    pub owner_kind: OwnerDocKind,
    pub owner_id: Box<str>,
    pub corpus_kind: SemanticCorpusKindV1,
    pub parent_owner_id: Option<Box<str>>,
    pub source_doc_id: Box<str>,
    pub repo_relative_path: RepoRelativePath,
    pub language: LanguageCode,
    pub package: Option<Box<str>>,
    pub symbol_kind: Option<SymbolKindCode>,
    pub visibility: Option<Box<str>>,
    pub source_role: SourceRoleV1,
    pub generated: bool,
    pub capability_status: CapabilityStatusV1,
    pub authority_digest: Box<str>,
    pub render_policy_digest: Box<str>,
    pub card_schema_version: u32,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub snippet: Box<str>,
    pub embedding_input_digest: Box<str>,
    pub vector_digest: Box<str>,
    pub view_kind: Box<str>,
    pub vector: Vec<f32>,
}

const EMBEDDING_RECORD_FIELDS: &[&str] = &[
    "embedding_id",
    "record_id",
    "owner_kind",
    "owner_id",
    "corpus_kind",
    "parent_owner_id",
    "source_doc_id",
    "repo_relative_path",
    "language",
    "package",
    "symbol_kind",
    "visibility",
    "source_role",
    "generated",
    "capability_status",
    "authority_digest",
    "render_policy_digest",
    "card_schema_version",
    "start_byte",
    "end_byte",
    "start_line",
    "end_line",
    "snippet",
    "embedding_input_digest",
    "vector_digest",
    "view_kind",
    "vector",
];

impl Serialize for EmbeddingRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("EmbeddingRecord", 27)?;
        state.serialize_field("embedding_id", &self.embedding_id)?;
        state.serialize_field("record_id", self.record_id.as_ref())?;
        state.serialize_field("owner_kind", &self.owner_kind)?;
        state.serialize_field("owner_id", self.owner_id.as_ref())?;
        state.serialize_field("corpus_kind", &self.corpus_kind)?;
        state.serialize_field("parent_owner_id", &self.parent_owner_id)?;
        state.serialize_field("source_doc_id", self.source_doc_id.as_ref())?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("language", &self.language)?;
        state.serialize_field("package", &self.package)?;
        state.serialize_field("symbol_kind", &self.symbol_kind)?;
        state.serialize_field("visibility", &self.visibility)?;
        state.serialize_field("source_role", &self.source_role)?;
        state.serialize_field("generated", &self.generated)?;
        state.serialize_field("capability_status", &self.capability_status)?;
        state.serialize_field("authority_digest", self.authority_digest.as_ref())?;
        state.serialize_field("render_policy_digest", self.render_policy_digest.as_ref())?;
        state.serialize_field("card_schema_version", &self.card_schema_version)?;
        state.serialize_field("start_byte", &self.start_byte)?;
        state.serialize_field("end_byte", &self.end_byte)?;
        state.serialize_field("start_line", &self.start_line)?;
        state.serialize_field("end_line", &self.end_line)?;
        state.serialize_field("snippet", self.snippet.as_ref())?;
        state.serialize_field(
            "embedding_input_digest",
            self.embedding_input_digest.as_ref(),
        )?;
        state.serialize_field("vector_digest", self.vector_digest.as_ref())?;
        state.serialize_field("view_kind", self.view_kind.as_ref())?;
        state.serialize_field("vector", &self.vector)?;
        state.end()
    }
}

struct EmbeddingRecordVisitor;

impl<'de> Visitor<'de> for EmbeddingRecordVisitor {
    type Value = EmbeddingRecord;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("an EmbeddingRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut embedding_id: Option<EmbeddingId> = None;
        let mut record_id: Option<String> = None;
        let mut owner_kind: Option<OwnerDocKind> = None;
        let mut owner_id: Option<String> = None;
        let mut corpus_kind: Option<SemanticCorpusKindV1> = None;
        let mut parent_owner_id: Option<Option<String>> = None;
        let mut source_doc_id: Option<String> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut language: Option<LanguageCode> = None;
        let mut package: Option<Option<String>> = None;
        let mut symbol_kind: Option<Option<SymbolKindCode>> = None;
        let mut visibility: Option<Option<String>> = None;
        let mut source_role: Option<SourceRoleV1> = None;
        let mut generated: Option<bool> = None;
        let mut capability_status: Option<CapabilityStatusV1> = None;
        let mut authority_digest: Option<String> = None;
        let mut render_policy_digest: Option<String> = None;
        let mut card_schema_version: Option<u32> = None;
        let mut start_byte: Option<u32> = None;
        let mut end_byte: Option<u32> = None;
        let mut start_line: Option<u32> = None;
        let mut end_line: Option<u32> = None;
        let mut snippet: Option<String> = None;
        let mut embedding_input_digest: Option<String> = None;
        let mut vector_digest: Option<String> = None;
        let mut view_kind: Option<String> = None;
        let mut vector: Option<Vec<f32>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "embedding_id" => {
                    if embedding_id.is_some() {
                        return Err(de::Error::duplicate_field("embedding_id"));
                    }
                    embedding_id = Some(map.next_value()?);
                }
                "record_id" => {
                    if record_id.is_some() {
                        return Err(de::Error::duplicate_field("record_id"));
                    }
                    record_id = Some(map.next_value()?);
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
                "corpus_kind" => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
                }
                "parent_owner_id" => {
                    if parent_owner_id.is_some() {
                        return Err(de::Error::duplicate_field("parent_owner_id"));
                    }
                    parent_owner_id = Some(map.next_value()?);
                }
                "source_doc_id" => {
                    if source_doc_id.is_some() {
                        return Err(de::Error::duplicate_field("source_doc_id"));
                    }
                    source_doc_id = Some(map.next_value()?);
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
                "start_byte" => {
                    if start_byte.is_some() {
                        return Err(de::Error::duplicate_field("start_byte"));
                    }
                    start_byte = Some(map.next_value()?);
                }
                "end_byte" => {
                    if end_byte.is_some() {
                        return Err(de::Error::duplicate_field("end_byte"));
                    }
                    end_byte = Some(map.next_value()?);
                }
                "start_line" => {
                    if start_line.is_some() {
                        return Err(de::Error::duplicate_field("start_line"));
                    }
                    start_line = Some(map.next_value()?);
                }
                "end_line" => {
                    if end_line.is_some() {
                        return Err(de::Error::duplicate_field("end_line"));
                    }
                    end_line = Some(map.next_value()?);
                }
                "snippet" => {
                    if snippet.is_some() {
                        return Err(de::Error::duplicate_field("snippet"));
                    }
                    snippet = Some(map.next_value()?);
                }
                "embedding_input_digest" => {
                    if embedding_input_digest.is_some() {
                        return Err(de::Error::duplicate_field("embedding_input_digest"));
                    }
                    embedding_input_digest = Some(map.next_value()?);
                }
                "vector_digest" => {
                    if vector_digest.is_some() {
                        return Err(de::Error::duplicate_field("vector_digest"));
                    }
                    vector_digest = Some(map.next_value()?);
                }
                "view_kind" => {
                    if view_kind.is_some() {
                        return Err(de::Error::duplicate_field("view_kind"));
                    }
                    view_kind = Some(map.next_value()?);
                }
                "vector" => {
                    if vector.is_some() {
                        return Err(de::Error::duplicate_field("vector"));
                    }
                    vector = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, EMBEDDING_RECORD_FIELDS)),
            }
        }
        let vector = vector.ok_or_else(|| de::Error::missing_field("vector"))?;
        if vector.is_empty() {
            return Err(de::Error::invalid_length(
                0,
                &"a non-empty embedding vector",
            ));
        }
        Ok(EmbeddingRecord {
            embedding_id: embedding_id.ok_or_else(|| de::Error::missing_field("embedding_id"))?,
            record_id: record_id
                .ok_or_else(|| de::Error::missing_field("record_id"))?
                .into_boxed_str(),
            owner_kind: owner_kind.ok_or_else(|| de::Error::missing_field("owner_kind"))?,
            owner_id: owner_id
                .ok_or_else(|| de::Error::missing_field("owner_id"))?
                .into_boxed_str(),
            corpus_kind: corpus_kind.ok_or_else(|| de::Error::missing_field("corpus_kind"))?,
            parent_owner_id: parent_owner_id
                .ok_or_else(|| de::Error::missing_field("parent_owner_id"))?
                .map(String::into_boxed_str),
            source_doc_id: source_doc_id
                .ok_or_else(|| de::Error::missing_field("source_doc_id"))?
                .into_boxed_str(),
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            language: language.ok_or_else(|| de::Error::missing_field("language"))?,
            package: package
                .ok_or_else(|| de::Error::missing_field("package"))?
                .map(String::into_boxed_str),
            symbol_kind: symbol_kind.ok_or_else(|| de::Error::missing_field("symbol_kind"))?,
            visibility: visibility
                .ok_or_else(|| de::Error::missing_field("visibility"))?
                .map(String::into_boxed_str),
            source_role: source_role.ok_or_else(|| de::Error::missing_field("source_role"))?,
            generated: generated.ok_or_else(|| de::Error::missing_field("generated"))?,
            capability_status: capability_status
                .ok_or_else(|| de::Error::missing_field("capability_status"))?,
            authority_digest: authority_digest
                .ok_or_else(|| de::Error::missing_field("authority_digest"))?
                .into_boxed_str(),
            render_policy_digest: render_policy_digest
                .ok_or_else(|| de::Error::missing_field("render_policy_digest"))?
                .into_boxed_str(),
            card_schema_version: card_schema_version
                .ok_or_else(|| de::Error::missing_field("card_schema_version"))?,
            start_byte: start_byte.ok_or_else(|| de::Error::missing_field("start_byte"))?,
            end_byte: end_byte.ok_or_else(|| de::Error::missing_field("end_byte"))?,
            start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
            end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
            snippet: snippet
                .ok_or_else(|| de::Error::missing_field("snippet"))?
                .into_boxed_str(),
            embedding_input_digest: embedding_input_digest
                .ok_or_else(|| de::Error::missing_field("embedding_input_digest"))?
                .into_boxed_str(),
            vector_digest: vector_digest
                .ok_or_else(|| de::Error::missing_field("vector_digest"))?
                .into_boxed_str(),
            view_kind: view_kind
                .ok_or_else(|| de::Error::missing_field("view_kind"))?
                .into_boxed_str(),
            vector,
        })
    }
}

impl<'de> Deserialize<'de> for EmbeddingRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "EmbeddingRecord",
            EMBEDDING_RECORD_FIELDS,
            EmbeddingRecordVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use ciborium::Value;
    use serde::de::DeserializeOwned;

    use super::{ChunkRecord, ChunkStructuralMetadata, EmbeddingRecord};
    use crate::lex::LanguageCode;
    use crate::semantic_kinds::OwnerDocKind;
    use crate::{
        CapabilityStatusV1, ChunkId, EmbeddingId, RepoRelativePath, SemanticCorpusKindV1,
        SourceRoleV1,
    };

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn rust_language() -> Result<LanguageCode, Box<dyn std::error::Error>> {
        LanguageCode::new("rust")
            .map_err(|err| -> Box<dyn std::error::Error> { err.to_string().into() })
    }

    fn missing_cbor_fields_are_refused<T: DeserializeOwned>(
        bytes: &[u8],
        fields: &[&str],
    ) -> TestRes {
        let original: Value = ciborium::from_reader(bytes)?;
        for field in fields {
            let mut wire = original.clone();
            let Value::Map(entries) = &mut wire else {
                return Err("record wire shape must be a map".into());
            };
            let before = entries.len();
            entries.retain(|(key, _)| key != &Value::Text((*field).to_owned()));
            if before.checked_sub(entries.len()) != Some(1) {
                return Err(format!("fixture does not carry exactly one {field}").into());
            }
            let mut old_bytes = Vec::new();
            ciborium::into_writer(&wire, &mut old_bytes)?;
            if ciborium::from_reader::<T, _>(old_bytes.as_slice()).is_ok() {
                return Err(format!("missing {field} was accepted").into());
            }
        }
        Ok(())
    }

    #[test]
    fn chunk_record_round_trip() -> TestRes {
        let record = ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language()?,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 2,
            text: "fn main() {}".into(),
            structural: Some(ChunkStructuralMetadata {
                variant_tag: "node".into(),
                structural_kind_tag: "function".into(),
                structural_pattern_kind: "function_def".into(),
                structural_name: "main".into(),
                structural_scope: "crate".into(),
                structural_matched_node: "fn main() {}".into(),
            }),
            parent_chunk_id: None,
            source_repo_id: None,
        };
        let mut bytes = Vec::new();
        ciborium::into_writer(&record, &mut bytes)?;
        let decoded: ChunkRecord = ciborium::from_reader(bytes.as_slice())?;
        if decoded != record {
            return Err(format!("decoded chunk record mismatch: {decoded:?} != {record:?}").into());
        }
        missing_cbor_fields_are_refused::<ChunkRecord>(&bytes, &["source_repo_id"])?;
        Ok(())
    }

    #[test]
    fn chunk_record_source_repo_id_round_trip() -> TestRes {
        use crate::RepoId;

        let record = ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language()?,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 2,
            text: "fn main() {}".into(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(
                RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            ),
        };
        let mut bytes = Vec::new();
        ciborium::into_writer(&record, &mut bytes)?;
        let decoded: ChunkRecord = ciborium::from_reader(bytes.as_slice())?;
        if decoded != record {
            return Err(format!("decoded chunk record mismatch: {decoded:?} != {record:?}").into());
        }
        Ok(())
    }

    #[test]
    fn chunk_record_rejects_legacy_preview_and_digest_fields() -> TestRes {
        let record = ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language()?,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 2,
            text: "fn main() {}".into(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        };
        for (field, value) in [
            ("snippet", "legacy snippet"),
            ("indexed_text", "legacy indexed text"),
            ("text_digest", "text:legacy"),
            ("shape_digest", "shape:legacy"),
        ] {
            let mut bytes = Vec::new();
            ciborium::into_writer(&record, &mut bytes)?;
            let mut wire: Value = ciborium::from_reader(bytes.as_slice())?;
            let Value::Map(entries) = &mut wire else {
                return Err("chunk record wire shape must remain a map".into());
            };
            entries.push((
                Value::Text(field.to_string()),
                Value::Text(value.to_string()),
            ));
            bytes.clear();
            ciborium::into_writer(&wire, &mut bytes)?;
            let err = match ciborium::from_reader::<ChunkRecord, _>(bytes.as_slice()) {
                Ok(decoded) => {
                    return Err(format!(
                        "expected legacy field `{field}` to be rejected, got {decoded:?}"
                    )
                    .into());
                }
                Err(err) => err.to_string(),
            };
            if !err.contains(field) {
                return Err(format!(
                    "expected legacy field `{field}` rejection to mention field name, got `{err}`"
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ciborium f16 path uses aarch64 inline asm that Miri cannot execute; embedding f32 vec serde is exercised in stable tests + fuzz"
    )]
    fn embedding_record_round_trip() -> TestRes {
        let record = EmbeddingRecord {
            embedding_id: EmbeddingId::new("emb-1"),
            record_id: "record-1".into(),
            owner_kind: OwnerDocKind::Chunk,
            owner_id: "chunk-1".into(),
            corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
            parent_owner_id: Some("parent-1".into()),
            source_doc_id: "doc-1".into(),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language()?,
            package: Some("crate".into()),
            symbol_kind: None,
            visibility: Some("pub".into()),
            source_role: SourceRoleV1::RawFallbackText,
            generated: true,
            capability_status: CapabilityStatusV1::Degraded,
            authority_digest: "auth:abc".into(),
            render_policy_digest: "render:def".into(),
            card_schema_version: 0,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 2,
            snippet: "fn main() {}".into(),
            embedding_input_digest: "input:abc".into(),
            vector_digest: "vec:def".into(),
            view_kind: "raw_chunk".into(),
            vector: vec![0.1, 0.2],
        };
        let mut bytes = Vec::new();
        ciborium::into_writer(&record, &mut bytes)?;
        let decoded: EmbeddingRecord = ciborium::from_reader(bytes.as_slice())?;
        if decoded != record {
            return Err(
                format!("decoded embedding record mismatch: {decoded:?} != {record:?}").into(),
            );
        }
        missing_cbor_fields_are_refused::<EmbeddingRecord>(
            &bytes,
            &["parent_owner_id", "package", "visibility"],
        )?;
        Ok(())
    }
}
