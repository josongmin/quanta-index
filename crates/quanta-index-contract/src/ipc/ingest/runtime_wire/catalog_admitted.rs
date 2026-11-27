//! Retained, admission-backed decode for the complete RuntimeMetadata header.

use core::{fmt, marker::PhantomData};

use quanta_index_contract_base::{
    AdmittedOptionalSeedV1, AdmittedSequenceSeedV1, AdmittedTextSeedV1,
    SourceIdentityDecodeAdmissionV1,
};
use serde::{Deserializer, de::{self, DeserializeSeed, MapAccess, Visitor}};

use super::{
    RUNTIME_CATALOG_INGEST_BATCH_FIELDS, RUNTIME_CHANGED_RECORD_FIELDS,
    RUNTIME_DOC_FACET_RECORD_FIELDS, RUNTIME_EDGE_AUTHORITY_RECORD_FIELDS,
    RUNTIME_SNAPSHOT_RECORD_FIELDS, RuntimeCatalogIngestBatch, RuntimeChangedRecord,
    RuntimeDocFacetRecord, RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord,
};
use crate::{ChunkId, ManifestGeneration, RepoId, RevisionId};
use crate::ipc::ingest::CorpusDecodeLimitsV1;

struct FieldSeed(&'static [&'static str]);

impl<'de> DeserializeSeed<'de> for FieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for FieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed runtime catalog field")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        self.0.iter().copied().find(|field| *field == key)
            .ok_or_else(|| E::unknown_field(key, self.0))
    }

    fn visit_bytes<E: de::Error>(self, key: &[u8]) -> Result<Self::Value, E> {
        let key = core::str::from_utf8(key)
            .map_err(|_| E::invalid_value(de::Unexpected::Bytes(key), &self))?;
        self.visit_str(key)
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted runtime catalog field must be borrowed"))
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

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de> for RevisionSeed<'_, A> {
    type Value = RevisionId;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        self.0.decode_revision_id_v1(decoder)
    }
}

struct CatalogSeed<'a, A: ?Sized, T> {
    admission: &'a A,
    limits: CorpusDecodeLimitsV1,
    value: PhantomData<T>,
}

impl<'a, A: ?Sized, T> CatalogSeed<'a, A, T> {
    fn new(admission: &'a A, limits: CorpusDecodeLimitsV1) -> Self {
        Self { admission, limits, value: PhantomData }
    }
}

trait CatalogDecode: Sized {
    const NAME: &'static str;
    const FIELDS: &'static [&'static str];

    fn visit<'de, M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
        map: M,
        admission: &A,
        limits: CorpusDecodeLimitsV1,
    ) -> Result<Self, M::Error>;
}

struct CatalogVisitor<'a, A: ?Sized, T> {
    admission: &'a A,
    limits: CorpusDecodeLimitsV1,
    value: PhantomData<T>,
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized, T: CatalogDecode> Visitor<'de>
    for CatalogVisitor<'_, A, T>
{
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(T::NAME)
    }

    fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
        T::visit(map, self.admission, self.limits)
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized, T: CatalogDecode> DeserializeSeed<'de>
    for CatalogSeed<'_, A, T>
{
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<T, D::Error> {
        let result = decoder.deserialize_struct(
            T::NAME, T::FIELDS,
            CatalogVisitor { admission: self.admission, limits: self.limits, value: PhantomData },
        );
        if result.is_err() {
            self.admission.abort_decode();
        }
        result
    }
}

macro_rules! set_once {
    ($slot:expr, $value:expr, $field:literal) => {
        if $slot.replace($value).is_some() {
            return Err(de::Error::duplicate_field($field));
        }
    };
}

macro_rules! required {
    ($slot:expr, $field:literal) => {
        $slot.ok_or_else(|| de::Error::missing_field($field))?
    };
}

fn next_chunk<'de, M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
    map: &mut M, admission: &A, limits: CorpusDecodeLimitsV1,
) -> Result<ChunkId, M::Error> {
    map.next_value_seed(ChunkId::deserialize_seed_with_admission_v1(admission, limits.text_bytes))
}

fn next_chunks<'de, M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
    map: &mut M, admission: &A, limits: CorpusDecodeLimitsV1,
) -> Result<Vec<ChunkId>, M::Error> {
    map.next_value_seed(AdmittedSequenceSeedV1::new(
        admission, limits.collection_items,
        || ChunkId::deserialize_seed_with_admission_v1(admission, limits.text_bytes),
    ))
}

fn next_text<'de, M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
    map: &mut M, admission: &A, limits: CorpusDecodeLimitsV1,
) -> Result<String, M::Error> {
    map.next_value_seed(AdmittedTextSeedV1::new(admission, limits.text_bytes))
}

impl CatalogDecode for RuntimeChangedRecord {
    const NAME: &'static str = "RuntimeChangedRecord";
    const FIELDS: &'static [&'static str] = RUNTIME_CHANGED_RECORD_FIELDS;

    fn visit<'de, M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
        mut map: M, admission: &A, limits: CorpusDecodeLimitsV1,
    ) -> Result<Self, M::Error> {
        let (mut doc_id, mut applied_at_ms, mut payload_hash) = (None, None, None);
        while let Some(key) = map.next_key_seed(FieldSeed(Self::FIELDS))? {
            match key {
                "doc_id" => set_once!(doc_id, next_chunk(&mut map, admission, limits)?, "doc_id"),
                "applied_at_ms" => set_once!(applied_at_ms, map.next_value()?, "applied_at_ms"),
                "payload_hash" => set_once!(payload_hash, map.next_value()?, "payload_hash"),
                _ => return Err(de::Error::unknown_field(key, Self::FIELDS)),
            }
        }
        Ok(Self {
            doc_id: required!(doc_id, "doc_id"),
            applied_at_ms: required!(applied_at_ms, "applied_at_ms"),
            payload_hash: required!(payload_hash, "payload_hash"),
        })
    }
}

impl CatalogDecode for RuntimeDocFacetRecord {
    const NAME: &'static str = "RuntimeDocFacetRecord";
    const FIELDS: &'static [&'static str] = RUNTIME_DOC_FACET_RECORD_FIELDS;

    fn visit<'de, M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
        mut map: M, admission: &A, limits: CorpusDecodeLimitsV1,
    ) -> Result<Self, M::Error> {
        let (mut doc_id, mut owner, mut service, mut layer, mut surface) =
            (None, None, None, None, None);
        while let Some(key) = map.next_key_seed(FieldSeed(Self::FIELDS))? {
            match key {
                "doc_id" => set_once!(doc_id, next_chunk(&mut map, admission, limits)?, "doc_id"),
                "owner" => set_once!(owner, map.next_value_seed(AdmittedOptionalSeedV1::new(
                    admission, AdmittedTextSeedV1::new(admission, limits.text_bytes),
                ))?, "owner"),
                "service" => set_once!(service, map.next_value_seed(AdmittedOptionalSeedV1::new(
                    admission, AdmittedTextSeedV1::new(admission, limits.text_bytes),
                ))?, "service"),
                "layer" => set_once!(layer, map.next_value_seed(AdmittedOptionalSeedV1::new(
                    admission, AdmittedTextSeedV1::new(admission, limits.text_bytes),
                ))?, "layer"),
                "surface" => set_once!(surface, map.next_value_seed(AdmittedOptionalSeedV1::new(
                    admission, AdmittedTextSeedV1::new(admission, limits.text_bytes),
                ))?, "surface"),
                _ => return Err(de::Error::unknown_field(key, Self::FIELDS)),
            }
        }
        Ok(Self {
            doc_id: required!(doc_id, "doc_id"),
            owner: required!(owner, "owner"),
            service: required!(service, "service"),
            layer: required!(layer, "layer"),
            surface: required!(surface, "surface"),
        })
    }
}

impl CatalogDecode for RuntimeSnapshotRecord {
    const NAME: &'static str = "RuntimeSnapshotRecord";
    const FIELDS: &'static [&'static str] = RUNTIME_SNAPSHOT_RECORD_FIELDS;

    fn visit<'de, M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
        mut map: M, admission: &A, limits: CorpusDecodeLimitsV1,
    ) -> Result<Self, M::Error> {
        let (mut name, mut doc_ids) = (None, None);
        while let Some(key) = map.next_key_seed(FieldSeed(Self::FIELDS))? {
            match key {
                "name" => set_once!(name, next_text(&mut map, admission, limits)?, "name"),
                "doc_ids" => set_once!(doc_ids, next_chunks(&mut map, admission, limits)?, "doc_ids"),
                _ => return Err(de::Error::unknown_field(key, Self::FIELDS)),
            }
        }
        Ok(Self { name: required!(name, "name"), doc_ids: required!(doc_ids, "doc_ids") })
    }
}

impl CatalogDecode for RuntimeEdgeAuthorityRecord {
    const NAME: &'static str = "RuntimeEdgeAuthorityRecord";
    const FIELDS: &'static [&'static str] = RUNTIME_EDGE_AUTHORITY_RECORD_FIELDS;

    fn visit<'de, M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
        mut map: M, admission: &A, limits: CorpusDecodeLimitsV1,
    ) -> Result<Self, M::Error> {
        let (mut key, mut doc_ids) = (None, None);
        while let Some(field) = map.next_key_seed(FieldSeed(Self::FIELDS))? {
            match field {
                "key" => set_once!(key, next_text(&mut map, admission, limits)?, "key"),
                "doc_ids" => set_once!(doc_ids, next_chunks(&mut map, admission, limits)?, "doc_ids"),
                _ => return Err(de::Error::unknown_field(field, Self::FIELDS)),
            }
        }
        Ok(Self { key: required!(key, "key"), doc_ids: required!(doc_ids, "doc_ids") })
    }
}

impl CatalogDecode for RuntimeCatalogIngestBatch {
    const NAME: &'static str = "RuntimeCatalogIngestBatch";
    const FIELDS: &'static [&'static str] = RUNTIME_CATALOG_INGEST_BATCH_FIELDS;

    fn visit<'de, M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized>(
        mut map: M, admission: &A, limits: CorpusDecodeLimitsV1,
    ) -> Result<Self, M::Error> {
        let (mut repo_id, mut revision_id, mut generation, mut overlay_epoch_ms,
            mut batch_digest, mut producer_head_applied_at_ms, mut generation_materialized_at_ms) =
            (None, None, None, None, None, None, None);
        let (mut changed_entries, mut facet_entries, mut snapshot_entries,
            mut affected_entries, mut invalidated_by_entries) = (None, None, None, None, None);
        while let Some(field) = map.next_key_seed(FieldSeed(Self::FIELDS))? {
            match field {
                "repo_id" => set_once!(repo_id, map.next_value_seed(RepoSeed(admission))?, "repo_id"),
                "revision_id" => set_once!(revision_id, map.next_value_seed(RevisionSeed(admission))?, "revision_id"),
                "generation" => set_once!(generation, map.next_value::<ManifestGeneration>()?, "generation"),
                "overlay_epoch_ms" => set_once!(overlay_epoch_ms, map.next_value()?, "overlay_epoch_ms"),
                "batch_digest" => set_once!(batch_digest, next_text(&mut map, admission, limits)?, "batch_digest"),
                "producer_head_applied_at_ms" => set_once!(producer_head_applied_at_ms, map.next_value()?, "producer_head_applied_at_ms"),
                "generation_materialized_at_ms" => set_once!(generation_materialized_at_ms, map.next_value()?, "generation_materialized_at_ms"),
                "changed_entries" => set_once!(changed_entries, next_records::<_, _, RuntimeChangedRecord>(&mut map, admission, limits)?, "changed_entries"),
                "facet_entries" => set_once!(facet_entries, next_records::<_, _, RuntimeDocFacetRecord>(&mut map, admission, limits)?, "facet_entries"),
                "snapshot_entries" => set_once!(snapshot_entries, next_records::<_, _, RuntimeSnapshotRecord>(&mut map, admission, limits)?, "snapshot_entries"),
                "affected_entries" => set_once!(affected_entries, next_records::<_, _, RuntimeEdgeAuthorityRecord>(&mut map, admission, limits)?, "affected_entries"),
                "invalidated_by_entries" => set_once!(invalidated_by_entries, next_records::<_, _, RuntimeEdgeAuthorityRecord>(&mut map, admission, limits)?, "invalidated_by_entries"),
                _ => return Err(de::Error::unknown_field(field, Self::FIELDS)),
            }
        }
        Ok(Self {
            repo_id: required!(repo_id, "repo_id"),
            revision_id: required!(revision_id, "revision_id"),
            generation: required!(generation, "generation"),
            overlay_epoch_ms: required!(overlay_epoch_ms, "overlay_epoch_ms"),
            batch_digest: required!(batch_digest, "batch_digest"),
            producer_head_applied_at_ms: required!(producer_head_applied_at_ms, "producer_head_applied_at_ms"),
            generation_materialized_at_ms: required!(generation_materialized_at_ms, "generation_materialized_at_ms"),
            changed_entries: required!(changed_entries, "changed_entries"),
            facet_entries: required!(facet_entries, "facet_entries"),
            snapshot_entries: required!(snapshot_entries, "snapshot_entries"),
            affected_entries: required!(affected_entries, "affected_entries"),
            invalidated_by_entries: required!(invalidated_by_entries, "invalidated_by_entries"),
        })
    }
}

fn next_records<'de, M, A, T>(
    map: &mut M, admission: &A, limits: CorpusDecodeLimitsV1,
) -> Result<Vec<T>, M::Error>
where
    M: MapAccess<'de>, A: SourceIdentityDecodeAdmissionV1 + ?Sized, T: CatalogDecode,
{
    map.next_value_seed(AdmittedSequenceSeedV1::new(
        admission, limits.collection_items, || CatalogSeed::<A, T>::new(admission, limits),
    ))
}

impl RuntimeCatalogIngestBatch {
    /// Decode the complete retained catalog with admitted nested text and vectors.
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A, limits: CorpusDecodeLimitsV1,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where A: SourceIdentityDecodeAdmissionV1 + ?Sized {
        CatalogSeed::<A, Self>::new(admission, limits)
    }
}
