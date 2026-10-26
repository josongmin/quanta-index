//! One decode policy for the existing corpus visitors and wire DTOs.

use core::marker::PhantomData;

use quanta_index_contract_base::{
    AdmittedByteStringSeedV1, AdmittedOptionalSeedV1, AdmittedSequenceSeedV1, AdmittedTextSeedV1,
    SourceIdentityDecodeAdmissionV1,
};
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::{
    CorpusFieldKeySeed, SEARCH_CORPUS_INGEST_BATCH_FIELDS, SEARCH_CORPUS_REPLACE_SCOPE_FIELDS,
    SEARCH_CORPUS_TOMBSTONE_SCOPE_FIELDS, SEARCH_SCOPE_KEY_FIELDS, SearchCorpusIngestBatch,
    SearchCorpusIngestBatchVisitor, SearchCorpusReplaceScope, SearchCorpusReplaceScopeVisitor,
    SearchCorpusTombstoneScope, SearchCorpusTombstoneScopeVisitor, SearchScopeKey,
    SearchScopeKeyVisitor, SearchScopeSurface, SourceBytesBuf,
};
use crate::ipc::semantic_source::{SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1};
use crate::lex::SymbolRecord;
use crate::{
    ChunkRecord, RepoId, RepoRelativePath, RevisionId, SourceFileCoverage, SourceFileKey,
    SourcePublicationEvent,
};

/// Per-call finite limits; no limit is inferred from encoded frame length.
///
/// These bound accepted fields and collection counts. A deserializer may
/// create temporary backing before entering a field visitor; its allocation
/// admission and peak capacity must be bounded separately by the parser owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CorpusDecodeLimitsV1 {
    pub text_bytes: usize,
    pub source_bytes: usize,
    pub bundle_bytes: usize,
    pub collection_items: usize,
}

pub(super) struct Ordinary;

pub(super) struct Admitted<'a, A: ?Sized> {
    admission: &'a A,
    limits: CorpusDecodeLimitsV1,
}

/// Only the finite known schema keys are accepted. The admitted form refuses
/// parser-owned key Strings rather than silently retaining unadmitted backing.
struct AdmittedFieldKeySeed(CorpusFieldKeySeed);

impl<'de> DeserializeSeed<'de> for AdmittedFieldKeySeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for AdmittedFieldKeySeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("a borrowed known corpus field name")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        self.0.classify(key)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted corpus field key must be borrowed"))
    }
}

struct ScalarSeed<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for ScalarSeed<T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<T, D::Error> {
        T::deserialize(decoder)
    }
}

struct RepoSeed<'a, A: ?Sized>(&'a A);
struct RevisionSeed<'a, A: ?Sized>(&'a A);

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de> for RepoSeed<'_, A> {
    type Value = RepoId;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<RepoId, D::Error> {
        self.0.decode_repo_id_v1(decoder)
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for RevisionSeed<'_, A>
{
    type Value = RevisionId;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<RevisionId, D::Error> {
        self.0.decode_revision_id_v1(decoder)
    }
}

pub(super) trait CorpusDecodePolicy<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error>;
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error>;
    fn next_coverage<M: MapAccess<'de>>(&self, map: &mut M)
    -> Result<SourceFileCoverage, M::Error>;
    fn next_source_bytes<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<u8>, M::Error>;
    fn next_chunks<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<ChunkRecord>, M::Error>;
    fn next_symbols<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<SymbolRecord>, M::Error>;
    fn next_file<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SourceFileKey, M::Error>;
    fn next_event<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<SourcePublicationEvent, M::Error>;
    fn next_repo<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error>;
    fn next_revision<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error>;
    fn next_digest<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error>;
    fn next_bundle<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<Vec<u8>>, M::Error>;
    fn next_clear<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SearchScopeSurface>, M::Error>;
    fn next_replace<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SearchCorpusReplaceScope>, M::Error>;
    fn next_tombstone<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SearchCorpusTombstoneScope>, M::Error>;
    fn next_semantic_replace<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SemanticSourceReplaceScopeV1>, M::Error>;
    fn next_semantic_tombstone<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SemanticSourceScopeKeyV1>, M::Error>;
}

impl<'de> CorpusDecodePolicy<'de> for Ordinary {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(CorpusFieldKeySeed { fields })
    }
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value()
    }
    fn next_coverage<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<SourceFileCoverage, M::Error> {
        map.next_value()
    }
    fn next_source_bytes<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<u8>, M::Error> {
        Ok(map.next_value::<SourceBytesBuf>()?.0)
    }
    fn next_chunks<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<ChunkRecord>, M::Error> {
        map.next_value()
    }
    fn next_symbols<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<SymbolRecord>, M::Error> {
        map.next_value()
    }
    fn next_file<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SourceFileKey, M::Error> {
        map.next_value()
    }
    fn next_event<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<SourcePublicationEvent, M::Error> {
        map.next_value()
    }
    fn next_repo<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error> {
        map.next_value()
    }
    fn next_revision<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error> {
        map.next_value()
    }
    fn next_digest<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value()
    }
    fn next_bundle<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<Vec<u8>>, M::Error> {
        map.next_value()
    }
    fn next_clear<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SearchScopeSurface>, M::Error> {
        map.next_value()
    }
    fn next_replace<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SearchCorpusReplaceScope>, M::Error> {
        map.next_value()
    }
    fn next_tombstone<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SearchCorpusTombstoneScope>, M::Error> {
        map.next_value()
    }
    fn next_semantic_replace<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SemanticSourceReplaceScopeV1>, M::Error> {
        map.next_value()
    }
    fn next_semantic_tombstone<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SemanticSourceScopeKeyV1>, M::Error> {
        map.next_value()
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> CorpusDecodePolicy<'de> for Admitted<'_, A> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(AdmittedFieldKeySeed(CorpusFieldKeySeed { fields }))
    }
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value_seed(RepoRelativePath::deserialize_seed_with_admission_v1(
            self.admission,
            self.limits.text_bytes,
        ))
    }
    fn next_coverage<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<SourceFileCoverage, M::Error> {
        map.next_value_seed(SourceFileCoverage::deserialize_seed_with_admission_v1(
            self.admission,
            self.limits.text_bytes,
        ))
    }
    fn next_source_bytes<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<u8>, M::Error> {
        map.next_value_seed(AdmittedByteStringSeedV1::new(
            self.admission,
            self.limits.source_bytes,
        ))
    }
    fn next_chunks<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<ChunkRecord>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                ChunkRecord::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits.text_bytes,
                )
            },
        ))
    }
    fn next_symbols<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<SymbolRecord>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                SymbolRecord::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits.text_bytes,
                )
            },
        ))
    }
    fn next_file<M: MapAccess<'de>>(&self, map: &mut M) -> Result<SourceFileKey, M::Error> {
        map.next_value_seed(SourceFileKey::deserialize_seed_with_admission_v1(
            self.admission,
            self.limits.text_bytes,
        ))
    }
    fn next_event<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<SourcePublicationEvent, M::Error> {
        map.next_value_seed(SourcePublicationEvent::deserialize_seed_with_admission_v1(
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
    fn next_digest<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error> {
        map.next_value_seed(AdmittedTextSeedV1::new(
            self.admission,
            self.limits.text_bytes,
        ))
    }
    fn next_bundle<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Option<Vec<u8>>, M::Error> {
        map.next_value_seed(AdmittedOptionalSeedV1::new(
            self.admission,
            AdmittedSequenceSeedV1::new(self.admission, self.limits.bundle_bytes, || {
                ScalarSeed::<u8>(PhantomData)
            }),
        ))
    }
    fn next_clear<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SearchScopeSurface>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || ScalarSeed::<SearchScopeSurface>(PhantomData),
        ))
    }
    fn next_replace<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SearchCorpusReplaceScope>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                SearchCorpusReplaceScope::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits,
                )
            },
        ))
    }
    fn next_tombstone<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SearchCorpusTombstoneScope>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                SearchCorpusTombstoneScope::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits,
                )
            },
        ))
    }
    fn next_semantic_replace<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SemanticSourceReplaceScopeV1>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                SemanticSourceReplaceScopeV1::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits.text_bytes,
                    self.limits.source_bytes,
                    self.limits.collection_items,
                )
            },
        ))
    }
    fn next_semantic_tombstone<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<SemanticSourceScopeKeyV1>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                SemanticSourceScopeKeyV1::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits.text_bytes,
                )
            },
        ))
    }
}

macro_rules! admitted_seed {
    ($seed:ident, $value:ty, $visitor:ident, $fields:expr) => {
        struct $seed<'a, A: ?Sized>(Admitted<'a, A>);

        impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
            for $seed<'_, A>
        {
            type Value = $value;

            fn deserialize<D: Deserializer<'de>>(
                self,
                decoder: D,
            ) -> Result<Self::Value, D::Error> {
                let admission = self.0.admission;
                let result =
                    decoder.deserialize_struct(stringify!($value), $fields, $visitor(self.0));
                if result.is_err() {
                    admission.abort_decode();
                }
                result
            }
        }
    };
}

admitted_seed!(
    SearchScopeKeySeed,
    SearchScopeKey,
    SearchScopeKeyVisitor,
    SEARCH_SCOPE_KEY_FIELDS
);
admitted_seed!(
    SearchCorpusReplaceScopeSeed,
    SearchCorpusReplaceScope,
    SearchCorpusReplaceScopeVisitor,
    SEARCH_CORPUS_REPLACE_SCOPE_FIELDS
);
admitted_seed!(
    SearchCorpusTombstoneScopeSeed,
    SearchCorpusTombstoneScope,
    SearchCorpusTombstoneScopeVisitor,
    SEARCH_CORPUS_TOMBSTONE_SCOPE_FIELDS
);
admitted_seed!(
    SearchCorpusIngestBatchSeed,
    SearchCorpusIngestBatch,
    SearchCorpusIngestBatchVisitor,
    SEARCH_CORPUS_INGEST_BATCH_FIELDS
);

impl SearchScopeKey {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        limits: CorpusDecodeLimitsV1,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        SearchScopeKeySeed(Admitted { admission, limits })
    }
}
impl SearchCorpusReplaceScope {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        limits: CorpusDecodeLimitsV1,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        SearchCorpusReplaceScopeSeed(Admitted { admission, limits })
    }
}
impl SearchCorpusTombstoneScope {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        limits: CorpusDecodeLimitsV1,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        SearchCorpusTombstoneScopeSeed(Admitted { admission, limits })
    }
}
impl SearchCorpusIngestBatch {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        limits: CorpusDecodeLimitsV1,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        SearchCorpusIngestBatchSeed(Admitted { admission, limits })
    }
}

#[cfg(test)]
mod tests;
