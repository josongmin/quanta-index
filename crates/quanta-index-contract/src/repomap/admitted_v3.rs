//! Admitted decode of the existing `RepoMap` publish DTOs.
//!
//! Every retained String and Vec is born through the caller's decode owner.
//! This is an allocation policy for the current wire, not a second wire IR.

use core::{fmt, marker::PhantomData};
use std::num::NonZeroU64;

use quanta_index_contract_base::{
    AdmittedOptionalSeedV1, AdmittedSequenceSeedV1, AdmittedTextSeedV1,
    SourceIdentityDecodeAdmissionV1,
};
use serde::Deserialize;
use serde::de::{
    self, DeserializeSeed, Deserializer, EnumAccess, MapAccess, VariantAccess, Visitor,
};

use super::{
    CandidateCommitmentV1, ChunkId, FileId, LanguageCode, ManifestGeneration,
    REPOMAP_CALL_EDGE_FIELDS, REPOMAP_CHUNK_NODE_FIELDS, REPOMAP_CONTAINS_EDGE_FIELDS,
    REPOMAP_DEPENDS_ON_EDGE_FIELDS, REPOMAP_EDGE_VARIANTS, REPOMAP_FILE_NODE_FIELDS,
    REPOMAP_GRAPH_COVERAGE_FIELDS, REPOMAP_IMPORT_EDGE_FIELDS, REPOMAP_MODULE_NODE_FIELDS,
    REPOMAP_MUTATION_ACK_V1_FIELDS, REPOMAP_NODE_REF_VARIANTS, REPOMAP_NODE_VARIANTS,
    REPOMAP_OWNS_CHUNK_EDGE_FIELDS, REPOMAP_SOURCE_BUNDLE_V1_FIELDS, REPOMAP_SYMBOL_NODE_FIELDS,
    RepoId, RepoMapActivateGenerationRequestV2, RepoMapCallEdge, RepoMapChunkExactness,
    RepoMapChunkNode, RepoMapContainsEdge, RepoMapDependsOnEdge, RepoMapEdge,
    RepoMapExactnessSummary, RepoMapExpectedActiveV2, RepoMapFileNode, RepoMapGraphCoverage,
    RepoMapGraphCoverageClass, RepoMapImportEdge, RepoMapItemIndexAvailability, RepoMapModuleId,
    RepoMapModuleNode, RepoMapMutationAck, RepoMapMutationAckDecodePolicyV3,
    RepoMapMutationAckV1Visitor, RepoMapNode, RepoMapNodeRef, RepoMapOwnsChunkEdge,
    RepoMapRedactionState, RepoMapSourceBundle, RepoMapSymbolNode, RepoRelativePath, RevisionId,
    SymbolId, SymbolKindCode,
};

/// A caller-selected finite policy, independent of the incoming CBOR header.
/// The parser owner must separately bound its physical buffers and retain its
/// grants for at least as long as the returned DTO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RepoMapDecodeLimitsV3 {
    pub text_bytes: usize,
    pub collection_items: usize,
}

struct Seed<'a, A: ?Sized, T> {
    admission: &'a A,
    limits: RepoMapDecodeLimitsV3,
    marker: PhantomData<fn() -> T>,
}

impl<'a, A: ?Sized, T> Seed<'a, A, T> {
    fn new(admission: &'a A, limits: RepoMapDecodeLimitsV3) -> Self {
        Self {
            admission,
            limits,
            marker: PhantomData,
        }
    }
}

struct ScalarSeed<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for ScalarSeed<T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<T, D::Error> {
        T::deserialize(decoder)
    }
}

macro_rules! admitted_scalar {
    ($($type:ty),+ $(,)?) => {
        $(impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
            for Seed<'_, A, $type>
        {
            type Value = $type;

            fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<$type, D::Error> {
                <$type>::deserialize(decoder)
            }
        })+
    };
}

admitted_scalar!(
    u32,
    ManifestGeneration,
    RepoMapChunkExactness,
    RepoMapItemIndexAvailability,
    RepoMapGraphCoverageClass,
    RepoMapExactnessSummary,
    RepoMapRedactionState,
);

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for Seed<'_, A, RepoId>
{
    type Value = RepoId;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<RepoId, D::Error> {
        self.admission.decode_repo_id_v1(decoder)
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for Seed<'_, A, RevisionId>
{
    type Value = RevisionId;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<RevisionId, D::Error> {
        self.admission.decode_revision_id_v1(decoder)
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for Seed<'_, A, String>
{
    type Value = String;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<String, D::Error> {
        AdmittedTextSeedV1::new(self.admission, self.limits.text_bytes).deserialize(decoder)
    }
}

/// CBOR keys and enum tags are known constants. Refuse parser-owned key
/// Strings instead of constructing another unretained visitor allocation.
pub(super) struct KnownKeySeed(pub(super) &'static [&'static str]);

struct AdmittedMutationAckPolicyV3<'a, A: ?Sized> {
    admission: &'a A,
    max_text_bytes: usize,
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> RepoMapMutationAckDecodePolicyV3<'de>
    for AdmittedMutationAckPolicyV3<'_, A>
{
    fn next_field<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(KnownKeySeed(REPOMAP_MUTATION_ACK_V1_FIELDS))
    }

    fn next_repo_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error> {
        map.next_value_seed(Seed::<A, RepoId>::new(
            self.admission,
            RepoMapDecodeLimitsV3 {
                text_bytes: self.max_text_bytes,
                collection_items: 0,
            },
        ))
    }

    fn next_revision_id<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error> {
        map.next_value_seed(Seed::<A, RevisionId>::new(
            self.admission,
            RepoMapDecodeLimitsV3 {
                text_bytes: self.max_text_bytes,
                collection_items: 0,
            },
        ))
    }

    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value_seed(AdmittedTextSeedV1::new(self.admission, self.max_text_bytes))
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
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for Seed<'_, A, RepoMapMutationAck>
{
    type Value = RepoMapMutationAck;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_struct(
            "RepoMapMutationAck",
            REPOMAP_MUTATION_ACK_V1_FIELDS,
            RepoMapMutationAckV1Visitor(AdmittedMutationAckPolicyV3 {
                admission: self.admission,
                max_text_bytes: self.limits.text_bytes,
            }),
        )
    }
}

impl RepoMapMutationAck {
    pub fn deserialize_seed_with_admission_v3<'a, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
        admission: &'a A,
        max_text_bytes: usize,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a {
        Seed::<A, Self>::new(
            admission,
            RepoMapDecodeLimitsV3 {
                text_bytes: max_text_bytes,
                collection_items: 0,
            },
        )
    }
}

impl<'de> DeserializeSeed<'de> for KnownKeySeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for KnownKeySeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed RepoMap schema key")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        self.0
            .iter()
            .copied()
            .find(|candidate| *candidate == key)
            .ok_or_else(|| de::Error::unknown_field(key, self.0))
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted RepoMap key must be borrowed"))
    }
}

macro_rules! admitted_struct {
    ($type:ident, $fields:ident, { $($name:ident : $field_type:ty),+ $(,)? }) => {
        impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
            for Seed<'_, A, $type>
        {
            type Value = $type;

            fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<$type, D::Error> {
                struct StructVisitor<'a, A: ?Sized> {
                    admission: &'a A,
                    limits: RepoMapDecodeLimitsV3,
                }

                impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> Visitor<'de>
                    for StructVisitor<'_, A>
                {
                    type Value = $type;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("an admitted ", stringify!($type)))
                    }

                    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<$type, M::Error> {
                        $(let mut $name: Option<$field_type> = None;)+
                        while let Some(key) = map.next_key_seed(KnownKeySeed($fields))? {
                            match key {
                                $(stringify!($name) => {
                                    if $name.is_some() {
                                        return Err(de::Error::duplicate_field(stringify!($name)));
                                    }
                                    $name = Some(map.next_value_seed(
                                        Seed::<_, $field_type>::new(self.admission, self.limits),
                                    )?);
                                },)+
                                _ => return Err(de::Error::unknown_field(key, $fields)),
                            }
                        }
                        Ok($type {
                            $($name: $name.ok_or_else(|| de::Error::missing_field(stringify!($name)))?,)+
                        })
                    }
                }

                let result = decoder.deserialize_struct(
                    stringify!($type),
                    $fields,
                    StructVisitor { admission: self.admission, limits: self.limits },
                );
                if result.is_err() {
                    self.admission.abort_decode();
                }
                result
            }
        }
    };
}

macro_rules! admitted_enum {
    ($type:ident, $variants:ident, { $($variant:ident : $inner:ty),+ $(,)? }) => {
        impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
            for Seed<'_, A, $type>
        {
            type Value = $type;

            fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<$type, D::Error> {
                struct EnumVisitor<'a, A: ?Sized> {
                    admission: &'a A,
                    limits: RepoMapDecodeLimitsV3,
                }

                impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> Visitor<'de>
                    for EnumVisitor<'_, A>
                {
                    type Value = $type;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("an admitted ", stringify!($type)))
                    }

                    fn visit_enum<E: EnumAccess<'de>>(self, data: E) -> Result<$type, E::Error> {
                        let (variant, access) = data.variant_seed(KnownKeySeed($variants))?;
                        match variant {
                            $(stringify!($variant) => Ok($type::$variant(
                                access.newtype_variant_seed(Seed::<_, $inner>::new(
                                    self.admission, self.limits,
                                ))?,
                            )),)+
                            _ => Err(de::Error::unknown_variant(variant, $variants)),
                        }
                    }
                }

                let result = decoder.deserialize_enum(
                    stringify!($type),
                    $variants,
                    EnumVisitor { admission: self.admission, limits: self.limits },
                );
                if result.is_err() {
                    self.admission.abort_decode();
                }
                result
            }
        }
    };
}

admitted_enum!(RepoMapNodeRef, REPOMAP_NODE_REF_VARIANTS, {
    File: FileId,
    Module: RepoMapModuleId,
    Symbol: SymbolId,
    Chunk: ChunkId,
});

// Scalar ID wrappers use the same admitted text seed as their existing wire
// deserializers. RepoId and RevisionId use the distinct native identity port.
macro_rules! admitted_string_id {
    ($type:ty) => {
        impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
            for Seed<'_, A, $type>
        {
            type Value = $type;

            fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<$type, D::Error> {
                <$type>::deserialize_seed_with_admission_v1(self.admission, self.limits.text_bytes)
                    .deserialize(decoder)
            }
        }
    };
}

admitted_string_id!(FileId);
admitted_string_id!(RepoMapModuleId);
admitted_string_id!(SymbolId);
admitted_string_id!(ChunkId);
admitted_string_id!(RepoRelativePath);
admitted_string_id!(LanguageCode);
admitted_string_id!(SymbolKindCode);

admitted_struct!(RepoMapFileNode, REPOMAP_FILE_NODE_FIELDS, {
    file_id: FileId,
    repo_relative_path: RepoRelativePath,
    line_count: u32,
});

admitted_struct!(RepoMapModuleNode, REPOMAP_MODULE_NODE_FIELDS, {
    module_id: RepoMapModuleId,
    repo_relative_path: RepoRelativePath,
    qualified_name: String,
});

admitted_struct!(RepoMapSymbolNode, REPOMAP_SYMBOL_NODE_FIELDS, {
    symbol_id: SymbolId,
    owner_path: RepoRelativePath,
    local_name: String,
    qualified_name: String,
    symbol_kind: SymbolKindCode,
});

admitted_struct!(RepoMapChunkNode, REPOMAP_CHUNK_NODE_FIELDS, {
    chunk_id: ChunkId,
    owner_path: RepoRelativePath,
    language: LanguageCode,
    start_byte: u32,
    end_byte: u32,
    start_line: u32,
    end_line: u32,
    token_count: u32,
    preview_text: String,
    exactness: RepoMapChunkExactness,
});

admitted_enum!(RepoMapNode, REPOMAP_NODE_VARIANTS, {
    File: RepoMapFileNode,
    Module: RepoMapModuleNode,
    Symbol: RepoMapSymbolNode,
    Chunk: RepoMapChunkNode,
});

admitted_struct!(RepoMapContainsEdge, REPOMAP_CONTAINS_EDGE_FIELDS, {
    container: RepoMapNodeRef,
    contained: RepoMapNodeRef,
});
admitted_struct!(RepoMapCallEdge, REPOMAP_CALL_EDGE_FIELDS, {
    caller: RepoMapNodeRef,
    callee: RepoMapNodeRef,
});
admitted_struct!(RepoMapImportEdge, REPOMAP_IMPORT_EDGE_FIELDS, {
    importer: RepoMapNodeRef,
    imported: RepoMapNodeRef,
});
admitted_struct!(RepoMapOwnsChunkEdge, REPOMAP_OWNS_CHUNK_EDGE_FIELDS, {
    owner: RepoMapNodeRef,
    chunk: RepoMapNodeRef,
});
admitted_struct!(RepoMapDependsOnEdge, REPOMAP_DEPENDS_ON_EDGE_FIELDS, {
    dependent: RepoMapNodeRef,
    dependency: RepoMapNodeRef,
});

admitted_enum!(RepoMapEdge, REPOMAP_EDGE_VARIANTS, {
    Contains: RepoMapContainsEdge,
    Call: RepoMapCallEdge,
    Import: RepoMapImportEdge,
    OwnsChunk: RepoMapOwnsChunkEdge,
    DependsOn: RepoMapDependsOnEdge,
});

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for Seed<'_, A, Vec<RepoMapNode>>
{
    type Value = Vec<RepoMapNode>;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        AdmittedSequenceSeedV1::new(self.admission, self.limits.collection_items, || {
            Seed::<_, RepoMapNode>::new(self.admission, self.limits)
        })
        .deserialize(decoder)
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for Seed<'_, A, Vec<RepoMapEdge>>
{
    type Value = Vec<RepoMapEdge>;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        AdmittedSequenceSeedV1::new(self.admission, self.limits.collection_items, || {
            Seed::<_, RepoMapEdge>::new(self.admission, self.limits)
        })
        .deserialize(decoder)
    }
}

admitted_struct!(RepoMapGraphCoverage, REPOMAP_GRAPH_COVERAGE_FIELDS, {
    item_index_availability: RepoMapItemIndexAvailability,
    graph_coverage_class: RepoMapGraphCoverageClass,
});

admitted_struct!(RepoMapSourceBundle, REPOMAP_SOURCE_BUNDLE_V1_FIELDS, {
    repo_id: RepoId,
    revision_id: RevisionId,
    manifest_generation: ManifestGeneration,
    manifest_digest: String,
    snapshot_id: String,
    projection_version: u32,
    authority_digest: String,
    graph_coverage: RepoMapGraphCoverage,
    exactness_summary: RepoMapExactnessSummary,
    redaction_state: RepoMapRedactionState,
    nodes: Vec<RepoMapNode>,
    edges: Vec<RepoMapEdge>,
});

impl RepoMapSourceBundle {
    /// Decode the existing bundle with all nested retained backing admitted.
    pub fn deserialize_seed_with_admission_v3<A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
        admission: &A,
        limits: RepoMapDecodeLimitsV3,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + '_ {
        Seed::<_, Self>::new(admission, limits)
    }
}

struct ExpectedActiveSeed<'a, A: ?Sized> {
    admission: &'a A,
    limits: RepoMapDecodeLimitsV3,
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for ExpectedActiveSeed<'_, A>
{
    type Value = RepoMapExpectedActiveV2;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        struct ExpectedVisitor<'a, A: ?Sized> {
            admission: &'a A,
            limits: RepoMapDecodeLimitsV3,
        }

        impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> Visitor<'de> for ExpectedVisitor<'_, A> {
            type Value = RepoMapExpectedActiveV2;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an admitted RepoMapExpectedActiveV2")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                const FIELDS: &[&str] = &["epoch", "candidate_commitment"];
                let mut epoch = None;
                let mut candidate_commitment = None;
                while let Some(key) = map.next_key_seed(KnownKeySeed(FIELDS))? {
                    match key {
                        "epoch" if epoch.is_none() => {
                            epoch = Some(map.next_value_seed(ScalarSeed::<u64>(PhantomData))?);
                        }
                        "epoch" => return Err(de::Error::duplicate_field("epoch")),
                        "candidate_commitment" if candidate_commitment.is_none() => {
                            candidate_commitment = Some(map.next_value_seed(
                                AdmittedTextSeedV1::new(self.admission, self.limits.text_bytes),
                            )?);
                        }
                        "candidate_commitment" => {
                            return Err(de::Error::duplicate_field("candidate_commitment"));
                        }
                        _ => return Err(de::Error::unknown_field(key, FIELDS)),
                    }
                }
                let epoch =
                    NonZeroU64::new(epoch.ok_or_else(|| de::Error::missing_field("epoch"))?)
                        .ok_or_else(|| de::Error::custom("activation epoch must be positive"))?;
                let commitment = candidate_commitment
                    .ok_or_else(|| de::Error::missing_field("candidate_commitment"))?;
                let candidate_commitment =
                    CandidateCommitmentV1::from_wire_str(&commitment).map_err(de::Error::custom)?;
                Ok(RepoMapExpectedActiveV2::new(epoch, candidate_commitment))
            }
        }

        let result = decoder.deserialize_struct(
            "RepoMapExpectedActiveV2",
            &["epoch", "candidate_commitment"],
            ExpectedVisitor {
                admission: self.admission,
                limits: self.limits,
            },
        );
        if result.is_err() {
            self.admission.abort_decode();
        }
        result
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for Seed<'_, A, Option<RepoMapExpectedActiveV2>>
{
    type Value = Option<RepoMapExpectedActiveV2>;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        AdmittedOptionalSeedV1::new(
            self.admission,
            ExpectedActiveSeed {
                admission: self.admission,
                limits: self.limits,
            },
        )
        .deserialize(decoder)
    }
}

const ACTIVATE_REQUEST_ADMITTED_FIELDS_V3: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "manifest_digest",
    "snapshot_id",
    "projection_version",
    "authority_digest",
    "source_bundle_digest",
    "expected_active",
];

admitted_struct!(RepoMapActivateGenerationRequestV2, ACTIVATE_REQUEST_ADMITTED_FIELDS_V3, {
    repo_id: RepoId,
    revision_id: RevisionId,
    manifest_generation: ManifestGeneration,
    manifest_digest: String,
    snapshot_id: String,
    projection_version: u32,
    authority_digest: String,
    source_bundle_digest: String,
    expected_active: Option<RepoMapExpectedActiveV2>,
});

impl RepoMapActivateGenerationRequestV2 {
    /// Decode the existing activation request with nested retained backing.
    pub fn deserialize_seed_with_admission_v3<A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
        admission: &A,
        limits: RepoMapDecodeLimitsV3,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + '_ {
        Seed::<_, Self>::new(admission, limits)
    }
}
