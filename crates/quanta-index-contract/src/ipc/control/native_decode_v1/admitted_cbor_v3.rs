//! Allocation policy for the existing corpus-head visitor on canonical CBOR.
//! The ordinary and Native JSON paths keep the same field and semantic body.

use super::{
    CorpusPolicyV1, HeadNodeV1, IdentityDataV1, ManifestDataV1, ManifestGeneration,
    NativeCorpusDataFailureV1, NativeCorpusDecodeDataV1, NativeIdentityCopyErrorV1, NodeSeedV1,
    RepoId, RevisionId, SearchCorpusActivationTokenV1, SearchCorpusActiveHeadV1, StringDataV1,
    TokenDataV1, de,
};
use quanta_index_contract_base::{DecodeBackingRefusalV1, SourceIdentityDecodeAdmissionV1};
use serde::{Deserialize, Deserializer, de::DeserializeSeed};

#[derive(Debug)]
pub enum AdmittedCorpusHeadRefusalV3<E> {
    Work(E),
    Backing(DecodeBackingRefusalV1),
}

struct AdmittedCborCorpusPolicyV3<'a, A: ?Sized, W> {
    admission: &'a A,
    work: &'a mut W,
}

impl<A, W, E> CorpusPolicyV1 for AdmittedCborCorpusPolicyV3<'_, A, W>
where
    A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    W: FnMut(u64) -> Result<(), E>,
{
    type OriginalError = AdmittedCorpusHeadRefusalV3<E>;
    type Funding = ();

    fn dynamic_input_v1(&self) -> bool {
        true
    }

    fn work_v1<D: de::Error>(
        &mut self,
        failure: &mut Option<Self::OriginalError>,
        units: u64,
    ) -> Result<(), D> {
        (self.work)(units).map_err(|cause| {
            *failure = Some(AdmittedCorpusHeadRefusalV3::Work(cause));
            D::custom("admitted corpus-head work refused")
        })
    }

    fn bytes_v1<D: de::Error>(
        &mut self,
        failure: &mut Option<Self::OriginalError>,
        bytes: usize,
    ) -> Result<(), D> {
        let units = u64::try_from(bytes)
            .map_err(|_overflow| D::custom("admitted corpus-head byte count overflow"))?;
        self.work_v1(failure, units)
    }

    fn invalid_v1<D: de::Error>(
        &self,
        _cause: NativeCorpusDataFailureV1,
        _field: Option<&'static str>,
        ordinary: impl FnOnce() -> D,
    ) -> D {
        ordinary()
    }

    fn string_v1<D: de::Error>(
        &mut self,
        value: &str,
        data: &mut StringDataV1<Self::OriginalError>,
    ) -> Result<(), D> {
        let units = u64::try_from(value.len())
            .map_err(|_overflow| D::custom("admitted corpus-head string length overflow"))?;
        if let Err(cause) = (self.work)(units) {
            data.failure = Some(NativeIdentityCopyErrorV1::Admission(
                AdmittedCorpusHeadRefusalV3::Work(cause),
            ));
            return Err(D::custom("admitted corpus-head copy work refused"));
        }
        self.admission
            .reserve_text(&mut data.backing, value.len())
            .map_err(|cause| {
                data.failure = Some(NativeIdentityCopyErrorV1::Admission(
                    AdmittedCorpusHeadRefusalV3::Backing(cause),
                ));
                D::custom("admitted corpus-head string backing refused")
            })?;
        if data.backing.capacity() < value.len() {
            return Err(D::custom("admitted corpus-head string capacity absent"));
        }
        data.backing.push_str(value);
        Ok(())
    }

    fn repo_v1<'de, D: Deserializer<'de>>(
        &mut self,
        decoder: D,
        data: &mut IdentityDataV1<RepoId, Self::OriginalError, Self::Funding>,
    ) -> Result<(), D::Error> {
        data.output = Some(self.admission.decode_repo_id_v1(decoder)?);
        Ok(())
    }

    fn revision_v1<'de, D: Deserializer<'de>>(
        &mut self,
        decoder: D,
        data: &mut IdentityDataV1<RevisionId, Self::OriginalError, Self::Funding>,
    ) -> Result<(), D::Error> {
        data.output = Some(self.admission.decode_revision_id_v1(decoder)?);
        Ok(())
    }

    fn manifest_v1<'de, D: Deserializer<'de>>(
        &mut self,
        decoder: D,
        data: &mut ManifestDataV1,
    ) -> Result<(), D::Error> {
        data.output = Some(ManifestGeneration::deserialize(decoder)?);
        Ok(())
    }

    fn token_v1<'de, D: Deserializer<'de>>(
        &mut self,
        decoder: D,
        data: &mut TokenDataV1<Self::OriginalError>,
    ) -> Result<(), D::Error> {
        data.output = Some(SearchCorpusActivationTokenV1::deserialize(decoder)?);
        Ok(())
    }
}

struct AdmittedCborHeadSeedV3<'a, A: ?Sized, W, E> {
    admission: &'a A,
    work: &'a mut W,
    data: &'a mut NativeCorpusDecodeDataV1<AdmittedCorpusHeadRefusalV3<E>, ()>,
}

impl<'de, A, W, E> DeserializeSeed<'de> for AdmittedCborHeadSeedV3<'_, A, W, E>
where
    A: SourceIdentityDecodeAdmissionV1 + ?Sized,
    W: FnMut(u64) -> Result<(), E>,
{
    type Value = SearchCorpusActiveHeadV1;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        if !self.data.is_fresh_v1() {
            return Err(de::Error::custom("admitted corpus-head DATA was reused"));
        }
        self.data.attempted = true;
        NodeSeedV1::<_, HeadNodeV1>::new_v1(
            &mut AdmittedCborCorpusPolicyV3 {
                admission: self.admission,
                work: self.work,
            },
            &mut self.data.state,
        )
        .deserialize(decoder)?;
        self.data.completed = true;
        let mut output = None;
        self.data
            .complete_into_slot_v1(&mut output)
            .map_err(de::Error::custom)?;
        output.ok_or_else(|| de::Error::custom("admitted corpus-head output absent"))
    }
}

impl SearchCorpusActiveHeadV1 {
    pub fn deserialize_seed_with_admission_v3<'a, A, W, E>(
        admission: &'a A,
        data: &'a mut NativeCorpusDecodeDataV1<AdmittedCorpusHeadRefusalV3<E>, ()>,
        work: &'a mut W,
    ) -> impl for<'de> DeserializeSeed<'de, Value = Self> + 'a
    where
        A: SourceIdentityDecodeAdmissionV1 + ?Sized,
        W: FnMut(u64) -> Result<(), E>,
    {
        AdmittedCborHeadSeedV3 {
            admission,
            work,
            data,
        }
    }
}
