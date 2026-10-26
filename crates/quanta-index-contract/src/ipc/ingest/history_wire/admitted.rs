//! One admitted decode policy for the existing History3 wire visitors.

use core::fmt;

use quanta_index_contract_base::{
    AdmittedBoxedTextSeedV1, AdmittedOptionalSeedV1, AdmittedSequenceSeedV1, AdmittedTextSeedV1,
    SourceIdentityDecodeAdmissionV1,
};
use serde::Deserializer;
use serde::de::{self, DeserializeSeed, EnumAccess, MapAccess, VariantAccess, Visitor};

use super::{
    FILE_CONTRIBUTOR_ENTRY_FIELDS, FILE_CONTRIBUTOR_IDENTITY_ENTRY_FIELDS,
    FILE_CONTRIBUTOR_INGEST_BATCH_FIELDS, FileContributorEntry, FileContributorEntryVisitor,
    FileContributorIdentityEntry, FileContributorIdentityEntryVisitor, FileContributorIngestBatch,
    FileContributorIngestBatchVisitor, HISTORY_DIFF_HUNK_UPSERT_FIELDS,
    HISTORY_INGEST_BATCH_FIELDS, HISTORY_REF_DELETE_FIELDS, HISTORY_REF_MUTATION_VARIANTS,
    HISTORY_REF_UPSERT_FIELDS, HistoryDiffHunkUpsert, HistoryDiffHunkUpsertVisitor,
    HistoryIngestBatch, HistoryIngestBatchVisitor, HistoryRefDelete, HistoryRefDeleteVisitor,
    HistoryRefMutation, HistoryRefMutationVisitor, HistoryRefUpsert, HistoryRefUpsertVisitor,
    REPO_COMMIT_RECENCY_ENTRY_FIELDS, REPO_COMMIT_RECENCY_INGEST_BATCH_FIELDS,
    RepoCommitRecencyEntry, RepoCommitRecencyEntryVisitor, RepoCommitRecencyIngestBatch,
    RepoCommitRecencyIngestBatchVisitor,
};
use crate::lex::{CommitRecord, CommitSha, DiffHunkRecord};
use crate::{RepoId, RepoRelativePath, RevisionId};

/// Finite per-field and per-sequence bounds. The caller separately owns the
/// parser's transient backing and must retain the admission owner with the DTO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryDecodeLimitsV1 {
    pub text_bytes: usize,
    pub collection_items: usize,
}

pub(super) struct Ordinary;

struct Admitted<'a, A: ?Sized> {
    admission: &'a A,
    limits: HistoryDecodeLimitsV1,
}

impl<A: ?Sized> Copy for Admitted<'_, A> {}
impl<A: ?Sized> Clone for Admitted<'_, A> {
    fn clone(&self) -> Self {
        *self
    }
}

struct FieldSeed(&'static [&'static str]);

struct VariantSeed;

impl<'de> DeserializeSeed<'de> for FieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for FieldSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed known History3 field or variant")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Self::Value, E> {
        self.0
            .iter()
            .copied()
            .find(|field| *field == key)
            .ok_or_else(|| E::unknown_field(key, self.0))
    }

    fn visit_string<E: de::Error>(self, _key: String) -> Result<Self::Value, E> {
        Err(E::custom("admitted History3 field must be borrowed"))
    }
}

impl<'de> DeserializeSeed<'de> for VariantSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_identifier(self)
    }
}

impl Visitor<'_> for VariantSeed {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a borrowed HistoryRefMutation variant")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        HISTORY_REF_MUTATION_VARIANTS
            .iter()
            .copied()
            .find(|variant| *variant == value)
            .ok_or_else(|| E::unknown_variant(value, HISTORY_REF_MUTATION_VARIANTS))
    }

    fn visit_string<E: de::Error>(self, _value: String) -> Result<Self::Value, E> {
        Err(E::custom(
            "admitted HistoryRefMutation variant must be borrowed",
        ))
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

pub(super) trait HistoryDecodePolicy<'de> {
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error>;
    fn next_boxed<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error>;
    fn next_sha<M: MapAccess<'de>>(&self, map: &mut M) -> Result<CommitSha, M::Error>;
    fn next_diff_record<M: MapAccess<'de>>(&self, map: &mut M) -> Result<DiffHunkRecord, M::Error>;
    fn next_repo<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoId, M::Error>;
    fn next_revision<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RevisionId, M::Error>;
    fn next_text<M: MapAccess<'de>>(&self, map: &mut M) -> Result<String, M::Error>;
    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<String>, M::Error>;
    fn next_commits<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<CommitRecord>, M::Error>;
    fn next_refs<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<HistoryRefMutation>, M::Error>;
    fn next_hunks<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<HistoryDiffHunkUpsert>, M::Error>;
    fn next_ref_variant<E: EnumAccess<'de>>(&self, data: E)
    -> Result<HistoryRefMutation, E::Error>;
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error>;
    fn next_recency_entries<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<RepoCommitRecencyEntry>, M::Error>;
    fn next_contributor_identities<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<FileContributorIdentityEntry>, M::Error>;
    fn next_contributor_entries<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<FileContributorEntry>, M::Error>;
}

impl<'de> HistoryDecodePolicy<'de> for Ordinary {
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
                    .ok_or_else(|| de::Error::unknown_field(&key, fields))
            })
            .transpose()
    }
    fn next_boxed<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        Ok(map.next_value::<String>()?.into_boxed_str())
    }
    fn next_sha<M: MapAccess<'de>>(&self, map: &mut M) -> Result<CommitSha, M::Error> {
        map.next_value()
    }
    fn next_diff_record<M: MapAccess<'de>>(&self, map: &mut M) -> Result<DiffHunkRecord, M::Error> {
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
    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<String>, M::Error> {
        map.next_value()
    }
    fn next_commits<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<CommitRecord>, M::Error> {
        map.next_value()
    }
    fn next_refs<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<HistoryRefMutation>, M::Error> {
        map.next_value()
    }
    fn next_hunks<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<HistoryDiffHunkUpsert>, M::Error> {
        map.next_value()
    }
    fn next_ref_variant<E: EnumAccess<'de>>(
        &self,
        data: E,
    ) -> Result<HistoryRefMutation, E::Error> {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "Upsert" => Ok(HistoryRefMutation::Upsert(variant.newtype_variant()?)),
            "Delete" => Ok(HistoryRefMutation::Delete(variant.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(
                other,
                HISTORY_REF_MUTATION_VARIANTS,
            )),
        }
    }
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value()
    }
    fn next_recency_entries<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<RepoCommitRecencyEntry>, M::Error> {
        map.next_value()
    }
    fn next_contributor_identities<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<FileContributorIdentityEntry>, M::Error> {
        map.next_value()
    }
    fn next_contributor_entries<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<FileContributorEntry>, M::Error> {
        map.next_value()
    }
}

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> HistoryDecodePolicy<'de>
    for Admitted<'_, A>
{
    fn next_field<M: MapAccess<'de>>(
        &self,
        map: &mut M,
        fields: &'static [&'static str],
    ) -> Result<Option<&'static str>, M::Error> {
        map.next_key_seed(FieldSeed(fields))
    }
    fn next_boxed<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Box<str>, M::Error> {
        map.next_value_seed(AdmittedBoxedTextSeedV1::new(
            self.admission,
            self.limits.text_bytes,
        ))
    }
    fn next_sha<M: MapAccess<'de>>(&self, map: &mut M) -> Result<CommitSha, M::Error> {
        map.next_value_seed(CommitSha::deserialize_seed_with_admission_v1(
            self.admission,
        ))
    }
    fn next_diff_record<M: MapAccess<'de>>(&self, map: &mut M) -> Result<DiffHunkRecord, M::Error> {
        map.next_value_seed(DiffHunkRecord::deserialize_seed_with_admission_v1(
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
    fn next_optional_text<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Option<String>, M::Error> {
        map.next_value_seed(AdmittedOptionalSeedV1::new(
            self.admission,
            AdmittedTextSeedV1::new(self.admission, self.limits.text_bytes),
        ))
    }
    fn next_commits<M: MapAccess<'de>>(&self, map: &mut M) -> Result<Vec<CommitRecord>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || {
                CommitRecord::deserialize_seed_with_admission_v1(
                    self.admission,
                    self.limits.text_bytes,
                    self.limits.collection_items,
                )
            },
        ))
    }
    fn next_refs<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<HistoryRefMutation>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || RefMutationSeed(*self),
        ))
    }
    fn next_hunks<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<HistoryDiffHunkUpsert>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || DiffUpsertSeed(*self),
        ))
    }
    fn next_ref_variant<E: EnumAccess<'de>>(
        &self,
        data: E,
    ) -> Result<HistoryRefMutation, E::Error> {
        let (tag, variant) = data.variant_seed(VariantSeed)?;
        match tag {
            "Upsert" => Ok(HistoryRefMutation::Upsert(
                variant.newtype_variant_seed(RefUpsertSeed(*self))?,
            )),
            "Delete" => Ok(HistoryRefMutation::Delete(
                variant.newtype_variant_seed(RefDeleteSeed(*self))?,
            )),
            other => Err(de::Error::unknown_variant(
                other,
                HISTORY_REF_MUTATION_VARIANTS,
            )),
        }
    }
    fn next_path<M: MapAccess<'de>>(&self, map: &mut M) -> Result<RepoRelativePath, M::Error> {
        map.next_value_seed(RepoRelativePath::deserialize_seed_with_admission_v1(
            self.admission,
            self.limits.text_bytes,
        ))
    }
    fn next_recency_entries<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<RepoCommitRecencyEntry>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || RecencyEntrySeed(*self),
        ))
    }
    fn next_contributor_identities<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<FileContributorIdentityEntry>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || ContributorIdentitySeed(*self),
        ))
    }
    fn next_contributor_entries<M: MapAccess<'de>>(
        &self,
        map: &mut M,
    ) -> Result<Vec<FileContributorEntry>, M::Error> {
        map.next_value_seed(AdmittedSequenceSeedV1::new(
            self.admission,
            self.limits.collection_items,
            || ContributorEntrySeed(*self),
        ))
    }
}

struct RefUpsertSeed<'a, A: ?Sized>(Admitted<'a, A>);
struct RefDeleteSeed<'a, A: ?Sized>(Admitted<'a, A>);
struct RefMutationSeed<'a, A: ?Sized>(Admitted<'a, A>);
struct DiffUpsertSeed<'a, A: ?Sized>(Admitted<'a, A>);
struct BatchSeed<'a, A: ?Sized>(Admitted<'a, A>);
struct RecencyEntrySeed<'a, A: ?Sized>(Admitted<'a, A>);
struct RecencyBatchSeed<'a, A: ?Sized>(Admitted<'a, A>);
struct ContributorIdentitySeed<'a, A: ?Sized>(Admitted<'a, A>);
struct ContributorEntrySeed<'a, A: ?Sized>(Admitted<'a, A>);
struct ContributorBatchSeed<'a, A: ?Sized>(Admitted<'a, A>);

macro_rules! admitted_record_seed {
    ($seed:ident, $value:ty, $name:literal, $fields:ident, $visitor:ident) => {
        impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
            for $seed<'_, A>
        {
            type Value = $value;
            fn deserialize<D: Deserializer<'de>>(
                self,
                decoder: D,
            ) -> Result<Self::Value, D::Error> {
                let admission = self.0.admission;
                let result = decoder.deserialize_struct($name, $fields, $visitor(self.0));
                if result.is_err() {
                    admission.abort_decode();
                }
                result
            }
        }
    };
}

admitted_record_seed!(
    RefUpsertSeed,
    HistoryRefUpsert,
    "HistoryRefUpsert",
    HISTORY_REF_UPSERT_FIELDS,
    HistoryRefUpsertVisitor
);
admitted_record_seed!(
    RefDeleteSeed,
    HistoryRefDelete,
    "HistoryRefDelete",
    HISTORY_REF_DELETE_FIELDS,
    HistoryRefDeleteVisitor
);
admitted_record_seed!(
    DiffUpsertSeed,
    HistoryDiffHunkUpsert,
    "HistoryDiffHunkUpsert",
    HISTORY_DIFF_HUNK_UPSERT_FIELDS,
    HistoryDiffHunkUpsertVisitor
);
admitted_record_seed!(
    BatchSeed,
    HistoryIngestBatch,
    "HistoryIngestBatch",
    HISTORY_INGEST_BATCH_FIELDS,
    HistoryIngestBatchVisitor
);
admitted_record_seed!(
    RecencyEntrySeed,
    RepoCommitRecencyEntry,
    "RepoCommitRecencyEntry",
    REPO_COMMIT_RECENCY_ENTRY_FIELDS,
    RepoCommitRecencyEntryVisitor
);
admitted_record_seed!(
    RecencyBatchSeed,
    RepoCommitRecencyIngestBatch,
    "RepoCommitRecencyIngestBatch",
    REPO_COMMIT_RECENCY_INGEST_BATCH_FIELDS,
    RepoCommitRecencyIngestBatchVisitor
);
admitted_record_seed!(
    ContributorIdentitySeed,
    FileContributorIdentityEntry,
    "FileContributorIdentityEntry",
    FILE_CONTRIBUTOR_IDENTITY_ENTRY_FIELDS,
    FileContributorIdentityEntryVisitor
);
admitted_record_seed!(
    ContributorEntrySeed,
    FileContributorEntry,
    "FileContributorEntry",
    FILE_CONTRIBUTOR_ENTRY_FIELDS,
    FileContributorEntryVisitor
);
admitted_record_seed!(
    ContributorBatchSeed,
    FileContributorIngestBatch,
    "FileContributorIngestBatch",
    FILE_CONTRIBUTOR_INGEST_BATCH_FIELDS,
    FileContributorIngestBatchVisitor
);

impl<'de, A: SourceIdentityDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for RefMutationSeed<'_, A>
{
    type Value = HistoryRefMutation;
    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        let admission = self.0.admission;
        let result = decoder.deserialize_enum(
            "HistoryRefMutation",
            HISTORY_REF_MUTATION_VARIANTS,
            HistoryRefMutationVisitor(self.0),
        );
        if result.is_err() {
            admission.abort_decode();
        }
        result
    }
}

impl HistoryIngestBatch {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        limits: HistoryDecodeLimitsV1,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        BatchSeed(Admitted { admission, limits })
    }
}

impl RepoCommitRecencyIngestBatch {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        limits: HistoryDecodeLimitsV1,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        RecencyBatchSeed(Admitted { admission, limits })
    }
}

impl FileContributorIngestBatch {
    pub fn deserialize_seed_with_admission_v1<'a, A>(
        admission: &'a A,
        limits: HistoryDecodeLimitsV1,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    {
        ContributorBatchSeed(Admitted { admission, limits })
    }
}

#[cfg(test)]
mod tests;
