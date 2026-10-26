//! Admission policy for dirty and structural ingest wire DTOs.

use core::fmt;

use quanta_index_contract_base::{
    AdmittedSequenceSeedV1, AdmittedTextSeedV1, SourceIdentityDecodeAdmissionV1,
};
use serde::{
    Deserializer,
    de::{self, DeserializeSeed, EnumAccess, MapAccess, VariantAccess, Visitor},
};

use super::{
    DIRTY_DELETE_FIELDS, DIRTY_INGEST_BATCH_FIELDS, DIRTY_MUTATION_VARIANTS, DirtyDelete,
    DirtyDeleteVisitor, DirtyIngestBatch, DirtyIngestBatchVisitor, DirtyMutation,
    STRUCTURAL_INGEST_BATCH_FIELDS, STRUCTURAL_REPLACE_SCOPE_FIELDS,
    STRUCTURAL_TOMBSTONE_SCOPE_FIELDS, STRUCTURAL_TREE_RECORD_FIELDS, StructuralIngestBatch,
    StructuralIngestBatchVisitor, StructuralReplaceScope, StructuralReplaceScopeVisitor,
    StructuralTombstoneScope, StructuralTombstoneScopeVisitor, StructuralTreeRecord,
    StructuralTreeRecordVisitor,
};
use crate::ipc::ingest::{CorpusDecodeLimitsV1, SearchScopeKey};
use crate::lex::{DirtyRecord, ParseTreeRecord};
use crate::{ChunkId, RepoId, RevisionId};

pub(super) trait RuntimeDecodePolicy<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error>;
    fn next_chunk<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ChunkId, M::Error>;
    fn next_repo<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error>;
    fn next_revision<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error>;
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error>;
    fn next_dirty_entries<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<DirtyMutation>, M::Error>;
    fn next_parse_tree<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ParseTreeRecord, M::Error>;
    fn next_scope<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SearchScopeKey, M::Error>;
    fn next_trees<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<StructuralTreeRecord>, M::Error>;
    fn next_replace_scopes<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<StructuralReplaceScope>, M::Error>;
    fn next_tombstone_scopes<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<StructuralTombstoneScope>, M::Error>;
}

pub(super) struct OrdinaryRuntime;

impl<'de> RuntimeDecodePolicy<'de> for OrdinaryRuntime {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error> {
        map.next_key::<String>()?
            .map(|key| {
                fields
                    .iter()
                    .copied()
                    .find(|field| *field == key)
                    .ok_or_else(|| de::Error::unknown_field(key.as_str(), fields))
            })
            .transpose()
    }
    fn next_chunk<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ChunkId, M::Error> {
        map.next_value()
    }
    fn next_repo<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error> {
        map.next_value()
    }
    fn next_revision<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error> {
        map.next_value()
    }
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value()
    }
    fn next_dirty_entries<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<DirtyMutation>, M::Error> {
        map.next_value()
    }
    fn next_parse_tree<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ParseTreeRecord, M::Error> {
        map.next_value()
    }
    fn next_scope<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SearchScopeKey, M::Error> {
        map.next_value()
    }
    fn next_trees<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<StructuralTreeRecord>, M::Error> {
        map.next_value()
    }
    fn next_replace_scopes<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<StructuralReplaceScope>, M::Error> {
        map.next_value()
    }
    fn next_tombstone_scopes<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<StructuralTombstoneScope>, M::Error> {
        map.next_value()
    }
}

struct RuntimeFieldSeed {
    fields: &'static [&'static str],
}

impl<'de> DeserializeSeed<'de> for RuntimeFieldSeed {
    type Value = &'static str;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for RuntimeFieldSeed {
    type Value = &'static str;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed runtime ingest field")
    }
    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        self.fields
            .iter()
            .copied()
            .find(|field| *field == key)
            .ok_or_else(|| E::unknown_field(key, self.fields))
    }
    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted runtime ingest field must be borrowed"))
    }
}

struct AdmittedRuntime<'a, A: ?Sized> {
    admission: &'a A,
    limits: CorpusDecodeLimitsV1,
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> RuntimeDecodePolicy<'de>
    for AdmittedRuntime<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(RuntimeFieldSeed { fields })
    }
    fn next_chunk<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ChunkId, M::Error> {
        map.next_value_seed(ChunkId::deserialize_seed_with_admission_v1(
            self.admission,
            self.limits.text_bytes,
        ))
    }
    fn next_repo<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error> {
        map.next_value_seed(RepoSeed(self.admission))
    }
    fn next_revision<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error> {
        map.next_value_seed(RevisionSeed(self.admission))
    }
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value_seed(AdmittedTextSeedV1::new(
            self.admission,
            self.limits.text_bytes,
        ))
    }
    fn next_dirty_entries<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<DirtyMutation>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || DirtyMutation::deserialize_seed_with_admission_v1(self.admission, self.limits),
        ))
    }
    fn next_parse_tree<M: MapAccess<'de>>(&self, map: &mut M) -> Result<ParseTreeRecord, M::Error> {
        map.next_value_seed(ParseTreeRecord::deserialize_seed_with_admission_v1(
            self.admission,
            self.limits.text_bytes,
            self.limits.collection_items,
        ))
    }
    fn next_scope<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SearchScopeKey, M::Error> {
        map.next_value_seed(SearchScopeKey::deserialize_seed_with_admission_v1(
            self.admission,
            self.limits,
        ))
    }
    fn next_trees<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<StructuralTreeRecord>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                StructuralTreeRecord::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits,
                )
            },
        ))
    }
    fn next_replace_scopes<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<StructuralReplaceScope>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                StructuralReplaceScope::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits,
                )
            },
        ))
    }
    fn next_tombstone_scopes<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<StructuralTombstoneScope>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                StructuralTombstoneScope::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits,
                )
            },
        ))
    }
}

struct RepoSeed<'a, A: ?Sized>(&'a A);
struct RevisionSeed<'a, A: ?Sized>(&'a A);

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de> for RepoSeed<'_, A> {
    type Value = RepoId;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        self.0.decode_repo_id_v1(decoder)
    }
}
impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for RevisionSeed<'_, A>
{
    type Value = RevisionId;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        self.0.decode_revision_id_v1(decoder)
    }
}

macro_rules! admitted_seed {
    ($dto:ident, $seed:ident, $visitor:ident, $fields:ident) => {
        impl $dto {
            /// The caller retains the admission owner and parser backing with this DTO.
            pub fn deserialize_seed_with_admission_v1<'a, A>(
                admission: &'a A,
                limits: CorpusDecodeLimitsV1,
            ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
            where
                A: SourceIdentityDecodeAdmissionV1 + ?Sized,
            {
                $seed(AdmittedRuntime { admission, limits })
            }
        }

        struct $seed<P>(P);

        impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
            for $seed<AdmittedRuntime<'_, A>>
        {
            type Value = $dto;
            fn deserialize<D: Deserializer<'de>>(
                self,
                decoder: D,
            ) -> Result<Self::Value, D::Error> {
                let admission = self.0.admission;
                let result =
                    decoder.deserialize_struct(stringify!($dto), $fields, $visitor(self.0));
                if result.is_err() {
                    admission.abort_decode();
                }
                result
            }
        }
    };
}

admitted_seed!(
    DirtyDelete,
    DirtyDeleteSeed,
    DirtyDeleteVisitor,
    DIRTY_DELETE_FIELDS
);
admitted_seed!(
    DirtyIngestBatch,
    DirtyIngestBatchSeed,
    DirtyIngestBatchVisitor,
    DIRTY_INGEST_BATCH_FIELDS
);
admitted_seed!(
    StructuralTreeRecord,
    StructuralTreeRecordSeed,
    StructuralTreeRecordVisitor,
    STRUCTURAL_TREE_RECORD_FIELDS
);
admitted_seed!(
    StructuralReplaceScope,
    StructuralReplaceScopeSeed,
    StructuralReplaceScopeVisitor,
    STRUCTURAL_REPLACE_SCOPE_FIELDS
);
admitted_seed!(
    StructuralTombstoneScope,
    StructuralTombstoneScopeSeed,
    StructuralTombstoneScopeVisitor,
    STRUCTURAL_TOMBSTONE_SCOPE_FIELDS
);
admitted_seed!(
    StructuralIngestBatch,
    StructuralIngestBatchSeed,
    StructuralIngestBatchVisitor,
    STRUCTURAL_INGEST_BATCH_FIELDS
);

impl DirtyMutation {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        limits: CorpusDecodeLimitsV1,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        DirtyMutationSeed(AdmittedRuntime { admission, limits })
    }
}

struct DirtyMutationSeed<P>(P);

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for DirtyMutationSeed<AdmittedRuntime<'_, A>>
{
    type Value = DirtyMutation;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.0.admission;
        let result = decoder.deserialize_enum("DirtyMutation", DIRTY_MUTATION_VARIANTS, self);
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> Visitor<'de>
    for DirtyMutationSeed<AdmittedRuntime<'_, A>>
{
    type Value = DirtyMutation;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DirtyMutation enum")
    }
    fn visit_enum<E: EnumAccess<'de>>(self, data: E) -> Result<Self::Value, E::Error> {
        let (tag, variant) = data.variant_seed(RuntimeFieldSeed {
            fields: DIRTY_MUTATION_VARIANTS,
        })?;
        match tag {
            "Upsert" => variant
                .newtype_variant_seed(DirtyRecord::deserialize_seed_with_admission_v1(
                    self.0.admission,
                    self.0.limits.text_bytes,
                ))
                .map(DirtyMutation::Upsert),
            "Delete" => variant
                .newtype_variant_seed(DirtyDelete::deserialize_seed_with_admission_v1(
                    self.0.admission,
                    self.0.limits,
                ))
                .map(DirtyMutation::Delete),
            other => Err(de::Error::unknown_variant(other, DIRTY_MUTATION_VARIANTS)),
        }
    }
}

#[cfg(test)]
mod tests;
