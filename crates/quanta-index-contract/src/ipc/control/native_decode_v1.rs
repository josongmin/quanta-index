//! One canonical visitor family; ordinary callers own local DATA, native
//! callers loan physical DATA retained outside their highest Source.
use super::{
    Deserialize, Deserializer, GENERATION_SNAPSHOT_FIELDS, GenerationSnapshot, ManifestGeneration,
    MapAccess, RepoId, RevisionId, SEARCH_CORPUS_ACTIVE_HEAD_V1_FIELDS,
    SEARCH_CORPUS_GENERATION_IDENTITY_V1_FIELDS, SEMANTIC_CONTENT_ROOTS_V1_FIELDS,
    SearchCorpusActivationTokenV1, SearchCorpusActiveHeadV1, SearchCorpusGenerationIdentityV1,
    SearchPlaneTrackKind, SemanticContentRootsV1, Visitor, de, fmt,
};
use core::{convert::Infallible, marker::PhantomData};
#[cfg(feature = "quanta-native-identity-v1")]
use quanta_index_contract_base::NativeIdentityCopyErrorV1;
#[cfg(feature = "quanta-native-identity-v1")]
use quanta_index_contract_base::try_copy_string_into_with_native_birth_v1;
#[cfg(feature = "quanta-native-identity-v1")]
use quanta_index_contract_base::{
    NativeActivationTokenDecodeDataV1, NativeIdentityDecodeDataV1,
    NativeManifestGenerationDecodeDataV1,
};
use serde::de::DeserializeSeed;

#[path = "native_decode_v1/borrowed_validation_v1.rs"]
mod borrowed_validation_v1;
pub(super) use borrowed_validation_v1::validate_expected_active_scope_v1;

macro_rules! corpus_data_failure_v1 {
    ($visibility:vis) => { #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        $visibility enum NativeCorpusDataFailureV1 { UnknownField, DuplicateField, MissingField, Semantic }
    };
}
#[cfg(feature = "quanta-native-identity-v1")]
corpus_data_failure_v1!(pub);
#[cfg(not(feature = "quanta-native-identity-v1"))]
corpus_data_failure_v1!(pub(super));
#[cfg(feature = "quanta-native-identity-v1")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCorpusDecodeRefusalV1 {
    Admission,
    NativeAllocation,
    InvalidNativeProducer,
    UnsupportedNativeProducer,
    Identity(quanta_index_contract_base::IdentityValidationErrorV1),
    InvalidData(NativeCorpusDataFailureV1, Option<&'static str>),
}
#[cfg(feature = "quanta-native-identity-v1")]
impl fmt::Display for NativeCorpusDecodeRefusalV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "native corpus decode refused: {self:?}")
    }
}

/// Native adapters admit the actual producer callback.
///
/// They cannot supply a canonical DTO, replace an identity input, or
/// substitute an NFC verdict. String birth admission must prepay the byte
/// copy work and retain the actual backing funding; validation work is separate.
/// Use a self-describing deserializer (such as JSON), with all input, owned
/// container and escaped scratch backing separately admitted and retained.
#[cfg(feature = "quanta-native-identity-v1")]
pub trait NativeCorpusDecodeAdmissionV1:
    quanta_index_contract_base::NativeIdentityDecodeAdmissionV1<Error = NativeCorpusDecodeRefusalV1>
    + quanta_index_contract_base::NativeActivationTokenDecodeAdmissionV1<
        ControlError = Self::OriginalError,
        Error = NativeCorpusDecodeRefusalV1,
    >
{
    fn consume_corpus_work_v1(&mut self, units: u64) -> Result<(), Self::OriginalError>;
    fn refuse_corpus_work_arithmetic_v1(&mut self) -> Self::OriginalError;
    fn admit_corpus_string_birth_v1(
        &mut self,
        bytes: usize,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Self::OriginalError>;
    fn corpus_invalid_data_v1(
        &self,
        cause: NativeCorpusDataFailureV1,
        field: Option<&'static str>,
    ) -> NativeCorpusDecodeRefusalV1;
}

struct KeyDataV1<E> {
    owned: [Option<String>; 5],
    pending: Option<String>,
    refused_map: Option<String>,
    seen: [bool; 5],
    failure: Option<E>,
}
impl<E> KeyDataV1<E> {
    fn new_v1() -> Self {
        Self {
            owned: core::array::from_fn(|_| None),
            pending: None,
            refused_map: None,
            seen: [false; 5],
            failure: None,
        }
    }
}
struct StringDataV1<E> {
    backing: String,
    #[cfg(feature = "quanta-native-identity-v1")]
    failure: Option<NativeIdentityCopyErrorV1<E>>,
    marker: PhantomData<E>,
}
impl<E> StringDataV1<E> {
    const fn new_v1() -> Self {
        Self {
            backing: String::new(),
            #[cfg(feature = "quanta-native-identity-v1")]
            failure: None,
            marker: PhantomData,
        }
    }
}
struct IdentityDataV1<T, E, F> {
    output: Option<T>,
    #[cfg(feature = "quanta-native-identity-v1")]
    native: NativeIdentityDecodeDataV1<T, E, F>,
    marker: PhantomData<(E, F)>,
}
impl<T, E, F> IdentityDataV1<T, E, F> {
    fn new_v1() -> Self {
        Self {
            output: None,
            #[cfg(feature = "quanta-native-identity-v1")]
            native: NativeIdentityDecodeDataV1::new_v1(),
            marker: PhantomData,
        }
    }
}
struct TokenDataV1<E> {
    output: Option<SearchCorpusActivationTokenV1>,
    #[cfg(feature = "quanta-native-identity-v1")]
    native: NativeActivationTokenDecodeDataV1<E>,
    marker: PhantomData<E>,
}
impl<E> TokenDataV1<E> {
    fn new_v1() -> Self {
        Self {
            output: None,
            #[cfg(feature = "quanta-native-identity-v1")]
            native: NativeActivationTokenDecodeDataV1::new_v1(),
            marker: PhantomData,
        }
    }
}
struct ManifestDataV1 {
    output: Option<ManifestGeneration>,
    #[cfg(feature = "quanta-native-identity-v1")]
    native: NativeManifestGenerationDecodeDataV1,
}
impl ManifestDataV1 {
    fn new_v1() -> Self {
        Self {
            output: None,
            #[cfg(feature = "quanta-native-identity-v1")]
            native: NativeManifestGenerationDecodeDataV1::new_v1(),
        }
    }
}
#[derive(Default)]
struct TrackDataV1 {
    output: Option<SearchPlaneTrackKind>,
    owned: Option<String>,
}
struct RootsDataV1<E> {
    output: Option<SemanticContentRootsV1>,
    row: StringDataV1<E>,
    membership: StringDataV1<E>,
    keys: KeyDataV1<E>,
}
impl<E> RootsDataV1<E> {
    fn new_v1() -> Self {
        Self {
            output: None,
            row: StringDataV1::new_v1(),
            membership: StringDataV1::new_v1(),
            keys: KeyDataV1::new_v1(),
        }
    }
}
struct SnapshotDataV1<E, F> {
    output: Option<GenerationSnapshot>,
    repo: IdentityDataV1<RepoId, E, F>,
    revision: IdentityDataV1<RevisionId, E, F>,
    track: TrackDataV1,
    generation: ManifestDataV1,
    digest: StringDataV1<E>,
    keys: KeyDataV1<E>,
}
impl<E, F> SnapshotDataV1<E, F> {
    fn new_v1() -> Self {
        Self {
            output: None,
            repo: IdentityDataV1::new_v1(),
            revision: IdentityDataV1::new_v1(),
            track: TrackDataV1::default(),
            generation: ManifestDataV1::new_v1(),
            digest: StringDataV1::new_v1(),
            keys: KeyDataV1::new_v1(),
        }
    }
}
struct GenerationDataV1<E, F> {
    output: Option<SearchCorpusGenerationIdentityV1>,
    lexical: SnapshotDataV1<E, F>,
    semantic: SnapshotDataV1<E, F>,
    content: RootsDataV1<E>,
    keys: KeyDataV1<E>,
}
impl<E, F> GenerationDataV1<E, F> {
    fn new_v1() -> Self {
        Self {
            output: None,
            lexical: SnapshotDataV1::new_v1(),
            semantic: SnapshotDataV1::new_v1(),
            content: RootsDataV1::new_v1(),
            keys: KeyDataV1::new_v1(),
        }
    }
}
struct HeadDataV1<E, F> {
    output: Option<SearchCorpusActiveHeadV1>,
    validation_failure: Option<super::SearchCorpusGenerationIdentityValidationErrorV1>,
    #[cfg(feature = "quanta-native-identity-v1")]
    activation_failure: Option<super::SearchCorpusActivationValidationErrorV1>,
    generation: GenerationDataV1<E, F>,
    token: TokenDataV1<E>,
    keys: KeyDataV1<E>,
}
impl<E, F> HeadDataV1<E, F> {
    fn new_v1() -> Self {
        Self {
            output: None,
            validation_failure: None,
            #[cfg(feature = "quanta-native-identity-v1")]
            activation_failure: None,
            generation: GenerationDataV1::new_v1(),
            token: TokenDataV1::new_v1(),
            keys: KeyDataV1::new_v1(),
        }
    }
}

/// Borrowed view of the complete retained producer failure. The finite Serde
/// marker is separate; this view never formats, clones, or replaces its cause.
#[cfg(feature = "quanta-native-identity-v1")]
pub enum NativeCorpusDecodeFailureV1<'a, E> {
    Work(&'a E),
    Copy(&'a NativeIdentityCopyErrorV1<E>),
    Identity(&'a quanta_index_contract_base::NativeIdentityConstructionErrorV1<E>),
    Validation(&'a super::SearchCorpusGenerationIdentityValidationErrorV1),
    Activation(&'a super::SearchCorpusActivationValidationErrorV1),
}
/// One head occurrence.
///
/// This is a physical product, not a collection arena:
/// each arbitrarily repeated head needs a distinct caller-admitted DATA loan.
/// Actual candidates drop before nested identity backing and funding banks.
/// No borrowed input, callback, Current/Source, or execution authority is held.
#[cfg(feature = "quanta-native-identity-v1")]
pub struct NativeCorpusDecodeDataV1<E, F> {
    state: HeadDataV1<E, F>,
    attempted: bool,
    completed: bool,
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<E, F> NativeCorpusDecodeDataV1<E, F> {
    #[must_use]
    pub fn new_v1() -> Self {
        Self {
            state: HeadDataV1::new_v1(),
            attempted: false,
            completed: false,
        }
    }
    #[must_use]
    pub const fn is_fresh_v1(&self) -> bool {
        !self.attempted
    }
    /// This fail-fast visitor records one complete producer refusal. The
    /// caller separately retains the full returned deserializer error.
    #[must_use]
    pub fn failure_v1(&self) -> Option<NativeCorpusDecodeFailureV1<'_, E>> {
        let generation = &self.state.generation;
        if let Some(cause) = [
            self.state.keys.failure.as_ref(),
            generation.keys.failure.as_ref(),
            generation.lexical.keys.failure.as_ref(),
            generation.semantic.keys.failure.as_ref(),
            generation.content.keys.failure.as_ref(),
            self.state.token.native.work_failure_v1(),
        ]
        .into_iter()
        .flatten()
        .next()
        {
            return Some(NativeCorpusDecodeFailureV1::Work(cause));
        }
        if let Some(cause) = [
            generation.lexical.digest.failure.as_ref(),
            generation.semantic.digest.failure.as_ref(),
            generation.content.row.failure.as_ref(),
            generation.content.membership.failure.as_ref(),
        ]
        .into_iter()
        .flatten()
        .next()
        {
            return Some(NativeCorpusDecodeFailureV1::Copy(cause));
        }
        if let Some(cause) = [
            generation.lexical.repo.native.failure_v1(),
            generation.lexical.revision.native.failure_v1(),
            generation.semantic.repo.native.failure_v1(),
            generation.semantic.revision.native.failure_v1(),
        ]
        .into_iter()
        .flatten()
        .next()
        {
            return Some(NativeCorpusDecodeFailureV1::Identity(cause));
        }
        if let Some(cause) = self.state.activation_failure.as_ref() {
            return Some(NativeCorpusDecodeFailureV1::Activation(cause));
        }
        if let Some(cause) = self.state.validation_failure.as_ref() {
            return Some(NativeCorpusDecodeFailureV1::Validation(cause));
        }
        None
    }
    /// Pure transfer to another external slot or publication after finishing.
    /// Retain this DATA and its funding until that external owner has finished.
    pub fn complete_into_slot_v1(
        &mut self,
        output: &mut Option<SearchCorpusActiveHeadV1>,
    ) -> Result<(), quanta_index_contract_base::NativeIdentityDecodeDataRefusalV1> {
        use quanta_index_contract_base::NativeIdentityDecodeDataRefusalV1 as Refusal;
        if output.is_some() {
            return Err(Refusal::OccupiedOutput);
        }
        if !self.completed || self.state.output.is_none() {
            return Err(Refusal::MissingResult);
        }
        *output = self.state.output.take();
        Ok(())
    }
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<E, F> Default for NativeCorpusDecodeDataV1<E, F> {
    fn default() -> Self {
        Self::new_v1()
    }
}

trait CorpusPolicyV1 {
    type OriginalError;
    fn dynamic_input_v1(&self) -> bool;
    type Funding;
    fn work_v1<E: de::Error>(
        &mut self,
        failure: &mut Option<Self::OriginalError>,
        units: u64,
    ) -> Result<(), E>;
    fn bytes_v1<E: de::Error>(
        &mut self,
        failure: &mut Option<Self::OriginalError>,
        bytes: usize,
    ) -> Result<(), E>;
    fn invalid_v1<E: de::Error>(
        &self,
        cause: NativeCorpusDataFailureV1,
        field: Option<&'static str>,
        ordinary: impl FnOnce() -> E,
    ) -> E;
    fn string_v1<E: de::Error>(
        &mut self,
        value: &str,
        data: &mut StringDataV1<Self::OriginalError>,
    ) -> Result<(), E>;
    fn repo_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut IdentityDataV1<RepoId, Self::OriginalError, Self::Funding>,
    ) -> Result<(), D::Error>;
    fn revision_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut IdentityDataV1<RevisionId, Self::OriginalError, Self::Funding>,
    ) -> Result<(), D::Error>;
    fn manifest_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut ManifestDataV1,
    ) -> Result<(), D::Error>;
    fn token_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut TokenDataV1<Self::OriginalError>,
    ) -> Result<(), D::Error>;
    fn semantic_v1<E: de::Error>(&self, ordinary: impl FnOnce() -> E) -> E {
        self.invalid_v1(NativeCorpusDataFailureV1::Semantic, None, ordinary)
    }
    fn missing_v1<E: de::Error>(&self, field: &'static str) -> E {
        self.invalid_v1(NativeCorpusDataFailureV1::MissingField, Some(field), || {
            E::missing_field(field)
        })
    }
}
struct OrdinaryCorpusPolicyV1;
impl CorpusPolicyV1 for OrdinaryCorpusPolicyV1 {
    type OriginalError = Infallible;
    fn dynamic_input_v1(&self) -> bool {
        false
    }
    type Funding = ();
    fn work_v1<E: de::Error>(&mut self, _: &mut Option<Infallible>, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn bytes_v1<E: de::Error>(&mut self, _: &mut Option<Infallible>, _: usize) -> Result<(), E> {
        Ok(())
    }
    fn invalid_v1<E: de::Error>(
        &self,
        _: NativeCorpusDataFailureV1,
        _: Option<&'static str>,
        ordinary: impl FnOnce() -> E,
    ) -> E {
        ordinary()
    }
    fn string_v1<E: de::Error>(
        &mut self,
        value: &str,
        data: &mut StringDataV1<Infallible>,
    ) -> Result<(), E> {
        value.clone_into(&mut data.backing);
        Ok(())
    }
    fn repo_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut IdentityDataV1<RepoId, Infallible, ()>,
    ) -> Result<(), D::Error> {
        data.output = Some(RepoId::deserialize(d)?);
        Ok(())
    }
    fn revision_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut IdentityDataV1<RevisionId, Infallible, ()>,
    ) -> Result<(), D::Error> {
        data.output = Some(RevisionId::deserialize(d)?);
        Ok(())
    }
    fn manifest_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut ManifestDataV1,
    ) -> Result<(), D::Error> {
        data.output = Some(ManifestGeneration::deserialize(d)?);
        Ok(())
    }
    fn token_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut TokenDataV1<Infallible>,
    ) -> Result<(), D::Error> {
        data.output = Some(SearchCorpusActivationTokenV1::deserialize(d)?);
        Ok(())
    }
}
#[cfg(feature = "quanta-native-identity-v1")]
struct NativeCorpusPolicyV1<'a, P: ?Sized>(&'a mut P);
#[cfg(feature = "quanta-native-identity-v1")]
impl<P: NativeCorpusDecodeAdmissionV1 + ?Sized> CorpusPolicyV1 for NativeCorpusPolicyV1<'_, P> {
    type OriginalError = P::OriginalError;
    fn dynamic_input_v1(&self) -> bool {
        true
    }
    type Funding = P::Funding;
    fn work_v1<E: de::Error>(
        &mut self,
        failure: &mut Option<P::OriginalError>,
        units: u64,
    ) -> Result<(), E> {
        if let Err(cause) = self.0.consume_corpus_work_v1(units) {
            *failure = Some(cause);
            return Err(E::custom(NativeCorpusDecodeRefusalV1::Admission));
        }
        Ok(())
    }
    fn bytes_v1<E: de::Error>(
        &mut self,
        failure: &mut Option<P::OriginalError>,
        bytes: usize,
    ) -> Result<(), E> {
        let Ok(units) = u64::try_from(bytes) else {
            *failure = Some(self.0.refuse_corpus_work_arithmetic_v1());
            return Err(E::custom(NativeCorpusDecodeRefusalV1::Admission));
        };
        self.work_v1(failure, units)
    }
    fn invalid_v1<E: de::Error>(
        &self,
        cause: NativeCorpusDataFailureV1,
        field: Option<&'static str>,
        _: impl FnOnce() -> E,
    ) -> E {
        E::custom(self.0.corpus_invalid_data_v1(cause, field))
    }
    fn string_v1<E: de::Error>(
        &mut self,
        value: &str,
        data: &mut StringDataV1<P::OriginalError>,
    ) -> Result<(), E> {
        if let Err(cause) =
            try_copy_string_into_with_native_birth_v1(value, &mut data.backing, |bytes, birth| {
                self.0.admit_corpus_string_birth_v1(bytes, birth)
            })
        {
            let marker = match &cause {
                NativeIdentityCopyErrorV1::Admission(_)
                | NativeIdentityCopyErrorV1::AdmissionAfterReserveFailure { .. } => {
                    NativeCorpusDecodeRefusalV1::Admission
                }
                NativeIdentityCopyErrorV1::NativeAllocationFailed(_) => {
                    NativeCorpusDecodeRefusalV1::NativeAllocation
                }
                NativeIdentityCopyErrorV1::InvalidNativeProducer
                | NativeIdentityCopyErrorV1::InvalidNativeProducerAfterReserveFailure(_)
                | NativeIdentityCopyErrorV1::InvalidNativeCapacity => {
                    NativeCorpusDecodeRefusalV1::InvalidNativeProducer
                }
            };
            data.failure = Some(cause);
            return Err(E::custom(marker));
        }
        Ok(())
    }
    fn repo_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut IdentityDataV1<RepoId, P::OriginalError, P::Funding>,
    ) -> Result<(), D::Error> {
        RepoId::native_decode_seed_v1(&mut *self.0, &mut data.native).deserialize(d)?;
        data.native
            .complete_into_slot_v1(&mut data.output)
            .map_err(de::Error::custom)
    }
    fn revision_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut IdentityDataV1<RevisionId, P::OriginalError, P::Funding>,
    ) -> Result<(), D::Error> {
        RevisionId::native_decode_seed_v1(&mut *self.0, &mut data.native).deserialize(d)?;
        data.native
            .complete_into_slot_v1(&mut data.output)
            .map_err(de::Error::custom)
    }
    fn manifest_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut ManifestDataV1,
    ) -> Result<(), D::Error> {
        ManifestGeneration::native_decode_seed_v1(&mut data.native).deserialize(d)?;
        data.native
            .complete_into_slot_v1(&mut data.output)
            .map_err(de::Error::custom)
    }
    fn token_v1<'de, D: Deserializer<'de>>(
        &mut self,
        d: D,
        data: &mut TokenDataV1<P::OriginalError>,
    ) -> Result<(), D::Error> {
        SearchCorpusActivationTokenV1::native_decode_seed_v1(&mut *self.0, &mut data.native)
            .deserialize(d)?;
        data.native
            .complete_into_slot_v1(&mut data.output)
            .map_err(de::Error::custom)
    }
}

struct KeySeedV1<'a, P: CorpusPolicyV1> {
    policy: &'a mut P,
    data: &'a mut KeyDataV1<P::OriginalError>,
    fields: &'static [&'static str],
}
impl<'de, P: CorpusPolicyV1> DeserializeSeed<'de> for KeySeedV1<'_, P> {
    type Value = usize;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<usize, D::Error> {
        d.deserialize_identifier(self)
    }
}
impl<P: CorpusPolicyV1> KeySeedV1<'_, P> {
    fn classify_v1<E: de::Error>(&mut self, borrowed: Option<&str>) -> Result<usize, E> {
        let key = borrowed.or(self.data.pending.as_deref()).unwrap_or("");
        for (index, ((field, seen), owned)) in self
            .fields
            .iter()
            .zip(self.data.seen.iter_mut())
            .zip(self.data.owned.iter_mut())
            .enumerate()
        {
            self.policy.work_v1(&mut self.data.failure, 1)?;
            if key == *field {
                if *seen {
                    return Err(self.policy.invalid_v1(
                        NativeCorpusDataFailureV1::DuplicateField,
                        Some(field),
                        || E::duplicate_field(field),
                    ));
                }
                *seen = true;
                *owned = self.data.pending.take();
                return Ok(index);
            }
        }
        Err(self
            .policy
            .invalid_v1(NativeCorpusDataFailureV1::UnknownField, None, || {
                E::unknown_field(key, self.fields)
            }))
    }
}
impl<P: CorpusPolicyV1> Visitor<'_> for KeySeedV1<'_, P> {
    type Value = usize;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a canonical corpus field")
    }
    fn visit_str<E: de::Error>(mut self, key: &str) -> Result<usize, E> {
        self.classify_v1(Some(key))
    }
    fn visit_string<E: de::Error>(mut self, key: String) -> Result<usize, E> {
        self.data.pending = Some(key);
        self.classify_v1(None)
    }
}
trait NodeV1<P: CorpusPolicyV1> {
    type Data;
    const NAME: &'static str;
    const FIELDS: &'static [&'static str];
    fn keys_v1(data: &mut Self::Data) -> &mut KeyDataV1<P::OriginalError>;
    fn field_v1<'de, A: MapAccess<'de>>(
        index: usize,
        map: &mut A,
        policy: &mut P,
        data: &mut Self::Data,
    ) -> Result<(), A::Error>;
    fn finish_v1<E: de::Error>(policy: &mut P, data: &mut Self::Data) -> Result<(), E>;
}
struct NodeSeedV1<'a, P: CorpusPolicyV1, N: NodeV1<P>> {
    policy: &'a mut P,
    data: &'a mut N::Data,
    node: PhantomData<N>,
}
impl<'a, P: CorpusPolicyV1, N: NodeV1<P>> NodeSeedV1<'a, P, N> {
    fn new_v1(policy: &'a mut P, data: &'a mut N::Data) -> Self {
        Self {
            policy,
            data,
            node: PhantomData,
        }
    }
}
impl<'de, P: CorpusPolicyV1, N: NodeV1<P>> DeserializeSeed<'de> for NodeSeedV1<'_, P, N> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        if self.policy.dynamic_input_v1() {
            d.deserialize_any(self)
        } else {
            d.deserialize_struct(N::NAME, N::FIELDS, self)
        }
    }
}
impl<'de, P: CorpusPolicyV1, N: NodeV1<P>> Visitor<'de> for NodeSeedV1<'_, P, N> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a {} map", N::NAME)
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        Err(E::invalid_type(de::Unexpected::Str(value), &self))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<(), E> {
        N::keys_v1(self.data).refused_map = Some(value);
        Err(E::invalid_type(
            de::Unexpected::Str(N::keys_v1(self.data).refused_map.as_deref().unwrap_or("")),
            &"a canonical corpus map",
        ))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        self.policy.work_v1(&mut N::keys_v1(self.data).failure, 1)?;
        while let Some(index) = map.next_key_seed(KeySeedV1 {
            policy: &mut *self.policy,
            data: N::keys_v1(self.data),
            fields: N::FIELDS,
        })? {
            N::field_v1(index, &mut map, self.policy, self.data)?;
        }
        for (index, field) in N::FIELDS.iter().enumerate() {
            if !N::keys_v1(self.data)
                .seen
                .get(index)
                .copied()
                .unwrap_or(false)
            {
                return Err(self.policy.missing_v1(field));
            }
        }
        N::finish_v1(self.policy, self.data)
    }
}
struct StringSeedV1<'a, P: CorpusPolicyV1> {
    policy: &'a mut P,
    data: &'a mut StringDataV1<P::OriginalError>,
}
impl<'de, P: CorpusPolicyV1> DeserializeSeed<'de> for StringSeedV1<'_, P> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_string(self)
    }
}
impl<P: CorpusPolicyV1> Visitor<'_> for StringSeedV1<'_, P> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a corpus string")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        self.policy.string_v1(value, self.data)
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<(), E> {
        self.data.backing = value;
        Ok(())
    }
}
struct TrackSeedV1<'a, P> {
    policy: &'a mut P,
    data: &'a mut TrackDataV1,
}
impl<'de, P: CorpusPolicyV1> DeserializeSeed<'de> for TrackSeedV1<'_, P> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_str(self)
    }
}
impl<P: CorpusPolicyV1> TrackSeedV1<'_, P> {
    fn classify_v1<E: de::Error>(&mut self, borrowed: Option<&str>) -> Result<(), E> {
        let value = borrowed.or(self.data.owned.as_deref()).unwrap_or("");
        self.data.output = Some(match value {
            "Lexical" => SearchPlaneTrackKind::Lexical,
            "Semantic" => SearchPlaneTrackKind::Semantic,
            "Structural" => SearchPlaneTrackKind::Structural,
            other => {
                return Err(self
                    .policy
                    .semantic_v1(|| E::unknown_variant(other, SearchPlaneTrackKind::VARIANTS)));
            }
        });
        Ok(())
    }
}
impl<P: CorpusPolicyV1> Visitor<'_> for TrackSeedV1<'_, P> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a SearchPlaneTrackKind string")
    }
    fn visit_str<E: de::Error>(mut self, value: &str) -> Result<(), E> {
        self.classify_v1(Some(value))
    }
    fn visit_string<E: de::Error>(mut self, value: String) -> Result<(), E> {
        self.data.owned = Some(value);
        self.classify_v1(None)
    }
}
// Scalar seeds preserve canonical identity/token producers, never an owned
// native adapter around ordinary Deserialize.
struct RepoSeedV1<'a, P: CorpusPolicyV1>(
    &'a mut P,
    &'a mut IdentityDataV1<RepoId, P::OriginalError, P::Funding>,
);
impl<'de, P: CorpusPolicyV1> DeserializeSeed<'de> for RepoSeedV1<'_, P> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        self.0.repo_v1(d, self.1)
    }
}
struct RevisionSeedV1<'a, P: CorpusPolicyV1>(
    &'a mut P,
    &'a mut IdentityDataV1<RevisionId, P::OriginalError, P::Funding>,
);
impl<'de, P: CorpusPolicyV1> DeserializeSeed<'de> for RevisionSeedV1<'_, P> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        self.0.revision_v1(d, self.1)
    }
}
struct TokenSeedV1<'a, P: CorpusPolicyV1>(&'a mut P, &'a mut TokenDataV1<P::OriginalError>);
impl<'de, P: CorpusPolicyV1> DeserializeSeed<'de> for TokenSeedV1<'_, P> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        self.0.token_v1(d, self.1)
    }
}
struct ManifestSeedV1<'a, P: CorpusPolicyV1>(&'a mut P, &'a mut ManifestDataV1);
impl<'de, P: CorpusPolicyV1> DeserializeSeed<'de> for ManifestSeedV1<'_, P> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        self.0.manifest_v1(d, self.1)
    }
}
struct RootsNodeV1;
impl<P: CorpusPolicyV1> NodeV1<P> for RootsNodeV1 {
    type Data = RootsDataV1<P::OriginalError>;
    const NAME: &'static str = "SemanticContentRootsV1";
    const FIELDS: &'static [&'static str] = SEMANTIC_CONTENT_ROOTS_V1_FIELDS;
    fn keys_v1(data: &mut Self::Data) -> &mut KeyDataV1<P::OriginalError> {
        &mut data.keys
    }
    fn field_v1<'de, A: MapAccess<'de>>(
        index: usize,
        map: &mut A,
        policy: &mut P,
        data: &mut Self::Data,
    ) -> Result<(), A::Error> {
        policy.work_v1(&mut data.keys.failure, 1)?;
        let slot = match index {
            0 => &mut data.row,
            1 => &mut data.membership,
            _ => return Err(de::Error::custom("invalid canonical root field index")),
        };
        map.next_value_seed(StringSeedV1 { policy, data: slot })
    }
    fn finish_v1<E: de::Error>(policy: &mut P, data: &mut Self::Data) -> Result<(), E> {
        data.output = Some(SemanticContentRootsV1 {
            row_root_digest: core::mem::take(&mut data.row.backing),
            membership_root_digest: core::mem::take(&mut data.membership.backing),
        });
        let roots = data
            .output
            .as_ref()
            .ok_or_else(|| E::custom("missing root candidate"))?;
        policy.bytes_v1(&mut data.keys.failure, roots.row_root_digest.len())?;
        policy.bytes_v1(&mut data.keys.failure, roots.membership_root_digest.len())?;
        if !roots.is_canonical_v1() {
            return Err(policy.semantic_v1(|| {
                E::custom("semantic content roots must be canonical sha256 digests")
            }));
        }
        Ok(())
    }
}
struct SnapshotNodeV1;
impl<P: CorpusPolicyV1> NodeV1<P> for SnapshotNodeV1 {
    type Data = SnapshotDataV1<P::OriginalError, P::Funding>;
    const NAME: &'static str = "GenerationSnapshot";
    const FIELDS: &'static [&'static str] = GENERATION_SNAPSHOT_FIELDS;
    fn keys_v1(data: &mut Self::Data) -> &mut KeyDataV1<P::OriginalError> {
        &mut data.keys
    }
    fn field_v1<'de, A: MapAccess<'de>>(
        index: usize,
        map: &mut A,
        policy: &mut P,
        data: &mut Self::Data,
    ) -> Result<(), A::Error> {
        policy.work_v1(&mut data.keys.failure, 1)?;
        match index {
            0 => map.next_value_seed(RepoSeedV1(policy, &mut data.repo)),
            1 => map.next_value_seed(RevisionSeedV1(policy, &mut data.revision)),
            2 => map.next_value_seed(TrackSeedV1 {
                policy,
                data: &mut data.track,
            }),
            3 => map.next_value_seed(ManifestSeedV1(policy, &mut data.generation)),
            4 => map.next_value_seed(StringSeedV1 {
                policy,
                data: &mut data.digest,
            }),
            _ => Err(de::Error::custom("invalid canonical snapshot field index")),
        }
    }
    fn finish_v1<E: de::Error>(policy: &mut P, data: &mut Self::Data) -> Result<(), E> {
        data.output = Some(GenerationSnapshot {
            repo_id: data
                .repo
                .output
                .take()
                .ok_or_else(|| policy.missing_v1("repo_id"))?,
            revision_id: data
                .revision
                .output
                .take()
                .ok_or_else(|| policy.missing_v1("revision_id"))?,
            track: data
                .track
                .output
                .take()
                .ok_or_else(|| policy.missing_v1("track"))?,
            manifest_generation: data
                .generation
                .output
                .take()
                .ok_or_else(|| policy.missing_v1("manifest_generation"))?,
            manifest_digest: core::mem::take(&mut data.digest.backing),
        });
        Ok(())
    }
}
struct GenerationNodeV1;
impl<P: CorpusPolicyV1> NodeV1<P> for GenerationNodeV1 {
    type Data = GenerationDataV1<P::OriginalError, P::Funding>;
    const NAME: &'static str = "SearchCorpusGenerationIdentityV1";
    const FIELDS: &'static [&'static str] = SEARCH_CORPUS_GENERATION_IDENTITY_V1_FIELDS;
    fn keys_v1(data: &mut Self::Data) -> &mut KeyDataV1<P::OriginalError> {
        &mut data.keys
    }
    fn field_v1<'de, A: MapAccess<'de>>(
        index: usize,
        map: &mut A,
        policy: &mut P,
        data: &mut Self::Data,
    ) -> Result<(), A::Error> {
        match index {
            0 => map.next_value_seed(NodeSeedV1::<_, SnapshotNodeV1>::new_v1(
                policy,
                &mut data.lexical,
            )),
            1 => map.next_value_seed(NodeSeedV1::<_, SnapshotNodeV1>::new_v1(
                policy,
                &mut data.semantic,
            )),
            2 => map.next_value_seed(NodeSeedV1::<_, RootsNodeV1>::new_v1(
                policy,
                &mut data.content,
            )),
            _ => Err(de::Error::custom(
                "invalid canonical generation field index",
            )),
        }
    }
    fn finish_v1<E: de::Error>(policy: &mut P, data: &mut Self::Data) -> Result<(), E> {
        data.output = Some(SearchCorpusGenerationIdentityV1 {
            lexical: data
                .lexical
                .output
                .take()
                .ok_or_else(|| policy.missing_v1("lexical"))?,
            semantic: data
                .semantic
                .output
                .take()
                .ok_or_else(|| policy.missing_v1("semantic"))?,
            semantic_content: data
                .content
                .output
                .take()
                .ok_or_else(|| policy.missing_v1("semantic_content"))?,
        });
        Ok(())
    }
}
struct HeadNodeV1;
impl<P: CorpusPolicyV1> NodeV1<P> for HeadNodeV1 {
    type Data = HeadDataV1<P::OriginalError, P::Funding>;
    const NAME: &'static str = "SearchCorpusActiveHeadV1";
    const FIELDS: &'static [&'static str] = SEARCH_CORPUS_ACTIVE_HEAD_V1_FIELDS;
    fn keys_v1(data: &mut Self::Data) -> &mut KeyDataV1<P::OriginalError> {
        &mut data.keys
    }
    fn field_v1<'de, A: MapAccess<'de>>(
        index: usize,
        map: &mut A,
        policy: &mut P,
        data: &mut Self::Data,
    ) -> Result<(), A::Error> {
        match index {
            0 => map.next_value_seed(NodeSeedV1::<_, GenerationNodeV1>::new_v1(
                policy,
                &mut data.generation,
            )),
            1 => {
                policy.work_v1(&mut data.keys.failure, 1)?;
                map.next_value_seed(TokenSeedV1(policy, &mut data.token))
            }
            _ => Err(de::Error::custom("invalid canonical head field index")),
        }
    }
    fn finish_v1<E: de::Error>(policy: &mut P, data: &mut Self::Data) -> Result<(), E> {
        data.output = Some(SearchCorpusActiveHeadV1 {
            generation: data
                .generation
                .output
                .take()
                .ok_or_else(|| policy.missing_v1("generation"))?,
            activation_token: data
                .token
                .output
                .take()
                .ok_or_else(|| policy.missing_v1("activation_token"))?,
        });
        let value = data
            .output
            .as_ref()
            .ok_or_else(|| E::custom("missing head candidate"))?;
        borrowed_validation_v1::validate_head_with_policy_v1(
            value,
            policy,
            &mut data.keys.failure,
            &mut data.validation_failure,
        )
    }
}

macro_rules! ordinary_node_v1 {
    ($function:ident, $node:ty, $data:ty, $output:ty) => {
        pub(super) fn $function<'de, D: Deserializer<'de>>(d: D) -> Result<$output, D::Error> {
            let mut data = <$data>::new_v1();
            NodeSeedV1::<_, $node>::new_v1(&mut OrdinaryCorpusPolicyV1, &mut data)
                .deserialize(d)?;
            data.output
                .ok_or_else(|| de::Error::custom("canonical corpus visitor produced no result"))
        }
    };
}
ordinary_node_v1!(
    ordinary_roots_v1,
    RootsNodeV1,
    RootsDataV1<Infallible>,
    SemanticContentRootsV1
);
ordinary_node_v1!(ordinary_snapshot_v1, SnapshotNodeV1, SnapshotDataV1<Infallible, ()>, GenerationSnapshot);
ordinary_node_v1!(ordinary_generation_v1, GenerationNodeV1, GenerationDataV1<Infallible, ()>, SearchCorpusGenerationIdentityV1);
ordinary_node_v1!(ordinary_head_v1, HeadNodeV1, HeadDataV1<Infallible, ()>, SearchCorpusActiveHeadV1);
pub(super) fn ordinary_track_v1<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<SearchPlaneTrackKind, D::Error> {
    let mut data = TrackDataV1::default();
    TrackSeedV1 {
        policy: &mut OrdinaryCorpusPolicyV1,
        data: &mut data,
    }
    .deserialize(d)?;
    data.output
        .ok_or_else(|| de::Error::custom("canonical track visitor produced no result"))
}
#[cfg(feature = "quanta-native-identity-v1")]
struct NativeHeadSeedV1<'a, P: NativeCorpusDecodeAdmissionV1 + ?Sized> {
    admission: &'a mut P,
    data: &'a mut NativeCorpusDecodeDataV1<P::OriginalError, P::Funding>,
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<'de, P: NativeCorpusDecodeAdmissionV1 + ?Sized> DeserializeSeed<'de>
    for NativeHeadSeedV1<'_, P>
{
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        if self.data.attempted {
            return Err(de::Error::custom("native corpus DATA is already used"));
        }
        self.data.attempted = true;
        NodeSeedV1::<_, HeadNodeV1>::new_v1(
            &mut NativeCorpusPolicyV1(self.admission),
            &mut self.data.state,
        )
        .deserialize(d)?;
        self.data.completed = true;
        Ok(())
    }
}
impl SearchCorpusActiveHeadV1 {
    /// Park the full deserializer error in a second external slot before the
    /// unit-status return. Occupied/reused DATA is refused before decoder polls.
    #[cfg(feature = "quanta-native-identity-v1")]
    pub fn try_decode_into_v1<
        'de,
        D: Deserializer<'de>,
        P: NativeCorpusDecodeAdmissionV1 + ?Sized,
    >(
        deserializer: D,
        admission: &mut P,
        data: &mut NativeCorpusDecodeDataV1<P::OriginalError, P::Funding>,
        failure: &mut Option<D::Error>,
    ) -> Result<(), quanta_index_contract_base::NativeIdentityDecodeDataRefusalV1> {
        use quanta_index_contract_base::NativeIdentityDecodeDataRefusalV1 as Refusal;
        if failure.is_some() {
            return Err(Refusal::OccupiedOutput);
        }
        if !data.is_fresh_v1() {
            return Err(Refusal::UsedData);
        }
        match Self::native_decode_seed_v1(admission, data).deserialize(deserializer) {
            Ok(()) => Ok(()),
            Err(cause) => {
                *failure = Some(cause);
                Err(Refusal::OperationRefused)
            }
        }
    }
    /// Unit decode into one externally retained occurrence. On error, move the
    /// returned complete `D::Error` to the caller's external error slot before
    /// finishing Source. All partial DTO/backing/full work errors remain DATA.
    #[cfg(feature = "quanta-native-identity-v1")]
    pub fn native_decode_seed_v1<'data, 'de, P: NativeCorpusDecodeAdmissionV1 + ?Sized>(
        admission: &'data mut P,
        data: &'data mut NativeCorpusDecodeDataV1<P::OriginalError, P::Funding>,
    ) -> impl DeserializeSeed<'de, Value = ()> + 'data {
        NativeHeadSeedV1 { admission, data }
    }
}

#[cfg(all(test, feature = "quanta-native-identity-v1"))]
#[path = "native_decode_tests_v1.rs"]
mod native_corpus_decode_tests_v1;
