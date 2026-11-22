//! Shared admission primitives for RepoMap requests and receipts.

use core::{fmt, marker::PhantomData};

use quanta_index_contract_base::{
    AdmittedOptionalSeedV1, AdmittedTextSeedV1, SourceIdentityDecodeAdmissionV1,
};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, Visitor};

use super::{
    REPOMAP_MUTATION_ACK_V1_FIELDS, RepoId, RepoMapMutationAck, RepoMapMutationAckDecodePolicyV3,
    RepoMapMutationAckV1Visitor, RevisionId,
};

/// A caller-selected finite policy, independent of the incoming CBOR header.
/// The parser owner must separately bound its physical buffers and retain its
/// grants for at least as long as the returned DTO.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RepoMapDecodeLimitsV3 {
    pub text_bytes: usize,
    pub collection_items: usize,
}

pub(super) struct Seed<'a, A: ?Sized, T> {
    pub(super) admission: &'a A,
    pub(super) limits: RepoMapDecodeLimitsV3,
    marker: PhantomData<fn() -> T>,
}

impl<'a, A: ?Sized, T> Seed<'a, A, T> {
    pub(super) fn new(admission: &'a A, limits: RepoMapDecodeLimitsV3) -> Self {
        Self {
            admission,
            limits,
            marker: PhantomData,
        }
    }
}

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
