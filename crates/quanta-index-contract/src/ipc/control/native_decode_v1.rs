//! Explicit native policy for the existing active-head visitor family.
use super::*;
use core::marker::PhantomData;
use serde::de::DeserializeSeed;

#[cfg(feature = "quanta-native-identity-v1")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCorpusDataFailureV1 {
    UnknownField,
    DuplicateField,
    MissingField,
    Semantic,
}

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

/// The same original runtime adapter supplies NFC, String backing and work.
/// Native schema/identity causes are finite; the adapter retains exact SourceE.
#[cfg(feature = "quanta-native-identity-v1")]
pub trait NativeCorpusDecodeAdmissionV1:
    quanta_index_contract_base::NativeIdentityDecodeAdmissionV1<Error = NativeCorpusDecodeRefusalV1>
    + quanta_index_contract_base::NativeActivationTokenDecodeAdmissionV1<
        Error = NativeCorpusDecodeRefusalV1,
    >
{
    fn consume_corpus_work_v1(&self, units: u64) -> Result<(), NativeCorpusDecodeRefusalV1>;
    fn refuse_corpus_work_arithmetic_v1(&self) -> NativeCorpusDecodeRefusalV1;
    fn corpus_invalid_data_v1(
        &self,
        cause: NativeCorpusDataFailureV1,
        field: Option<&'static str>,
    ) -> NativeCorpusDecodeRefusalV1;
}

#[derive(Clone, Copy)]
pub(super) enum CorpusDecodeModeV1<'a> {
    Ordinary(PhantomData<&'a ()>),
    #[cfg(feature = "quanta-native-identity-v1")]
    Native(&'a dyn NativeCorpusDecodeAdmissionV1),
}
impl<'a> CorpusDecodeModeV1<'a> {
    pub(super) fn ordinary_v1() -> Self {
        Self::Ordinary(PhantomData)
    }
    pub(super) fn work_v1<E: de::Error>(self, units: u64) -> Result<(), E> {
        match self {
            Self::Ordinary(_) => {
                let _ = units;
                Ok(())
            }
            #[cfg(feature = "quanta-native-identity-v1")]
            Self::Native(owner) => owner.consume_corpus_work_v1(units).map_err(E::custom),
        }
    }
    pub(super) fn bytes_v1<E: de::Error>(self, bytes: usize) -> Result<(), E> {
        match self {
            Self::Ordinary(_) => {
                let _ = bytes;
                Ok(())
            }
            #[cfg(feature = "quanta-native-identity-v1")]
            Self::Native(owner) => {
                let units = u64::try_from(bytes)
                    .map_err(|_| E::custom(owner.refuse_corpus_work_arithmetic_v1()))?;
                owner.consume_corpus_work_v1(units).map_err(E::custom)
            }
        }
    }
    pub(super) fn semantic_v1<E: de::Error>(self, ordinary: impl FnOnce() -> E) -> E {
        match self {
            Self::Ordinary(_) => ordinary(),
            #[cfg(feature = "quanta-native-identity-v1")]
            Self::Native(owner) => {
                E::custom(owner.corpus_invalid_data_v1(NativeCorpusDataFailureV1::Semantic, None))
            }
        }
    }
    pub(super) fn duplicate_v1<E: de::Error>(self, field: &'static str) -> E {
        match self {
            Self::Ordinary(_) => E::duplicate_field(field),
            #[cfg(feature = "quanta-native-identity-v1")]
            Self::Native(owner) => E::custom(
                owner
                    .corpus_invalid_data_v1(NativeCorpusDataFailureV1::DuplicateField, Some(field)),
            ),
        }
    }
    pub(super) fn missing_v1<E: de::Error>(self, field: &'static str) -> E {
        match self {
            Self::Ordinary(_) => E::missing_field(field),
            #[cfg(feature = "quanta-native-identity-v1")]
            Self::Native(owner) => E::custom(
                owner.corpus_invalid_data_v1(NativeCorpusDataFailureV1::MissingField, Some(field)),
            ),
        }
    }
    pub(super) fn unknown_v1<E: de::Error>(self, key: &str, fields: &'static [&'static str]) -> E {
        match self {
            Self::Ordinary(_) => E::unknown_field(key, fields),
            #[cfg(feature = "quanta-native-identity-v1")]
            Self::Native(owner) => E::custom(
                owner.corpus_invalid_data_v1(NativeCorpusDataFailureV1::UnknownField, None),
            ),
        }
    }
    pub(super) fn seed_v1<T: CorpusDecodeValueV1>(self) -> CorpusDecodeSeedV1<'a, T> {
        CorpusDecodeSeedV1 {
            mode: self,
            value: PhantomData,
        }
    }
}

pub(super) trait CorpusDecodeValueV1: Sized {
    fn decode_v1<'de, D: Deserializer<'de>>(
        deserializer: D,
        mode: CorpusDecodeModeV1<'_>,
    ) -> Result<Self, D::Error>;
}
pub(super) struct CorpusDecodeSeedV1<'a, T> {
    mode: CorpusDecodeModeV1<'a>,
    value: PhantomData<T>,
}
impl<'de, T: CorpusDecodeValueV1> DeserializeSeed<'de> for CorpusDecodeSeedV1<'_, T> {
    type Value = T;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        T::decode_v1(deserializer, self.mode)
    }
}

macro_rules! scalar_v1 {
    ($($type:ty),+ $(,)?) => {$(
        impl CorpusDecodeValueV1 for $type {
            fn decode_v1<'de,D:Deserializer<'de>>(deserializer:D, mode:CorpusDecodeModeV1<'_>)->Result<Self,D::Error>{
                mode.work_v1(1)?;
                Self::deserialize(deserializer)
            }
        }
    )+};
}
scalar_v1!(String, SearchPlaneTrackKind, ManifestGeneration);

macro_rules! identity_v1 {
    ($($type:ty),+ $(,)?) => {$(
        impl CorpusDecodeValueV1 for $type {
            fn decode_v1<'de,D:Deserializer<'de>>(deserializer:D, mode:CorpusDecodeModeV1<'_>)->Result<Self,D::Error>{
                mode.work_v1(1)?;
                match mode {
                    CorpusDecodeModeV1::Ordinary(_) => Self::deserialize(deserializer),
                    #[cfg(feature="quanta-native-identity-v1")]
                    CorpusDecodeModeV1::Native(owner) => Self::native_decode_seed_v1(owner).deserialize(deserializer),
                }
            }
        }
    )+};
}
identity_v1!(RepoId, RevisionId, SearchCorpusActivationTokenV1);

pub(super) struct CorpusKeyV1 {
    known: Option<&'static str>,
    ordinary_unknown: Option<String>,
}
impl CorpusKeyV1 {
    pub(super) fn as_str(&self) -> &str {
        self.known
            .or(self.ordinary_unknown.as_deref())
            .unwrap_or("")
    }
}
pub(super) struct CorpusKeySeedV1<'a>(pub CorpusDecodeModeV1<'a>, pub &'static [&'static str]);
impl<'de> DeserializeSeed<'de> for CorpusKeySeedV1<'_> {
    type Value = CorpusKeyV1;
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<CorpusKeyV1, D::Error> {
        impl<'de> Visitor<'de> for CorpusKeySeedV1<'_> {
            type Value = CorpusKeyV1;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a canonical corpus field")
            }
            fn visit_str<E: de::Error>(self, key: &str) -> Result<CorpusKeyV1, E> {
                let mut known = None;
                for field in self.1 {
                    self.0.work_v1(1)?;
                    if key == *field {
                        known = Some(*field);
                        break;
                    }
                }
                let ordinary_unknown = match (known, self.0) {
                    (None, CorpusDecodeModeV1::Ordinary(_)) => Some(key.to_owned()),
                    _ => None,
                };
                Ok(CorpusKeyV1 {
                    known,
                    ordinary_unknown,
                })
            }
        }
        deserializer.deserialize_identifier(self)
    }
}

impl SearchCorpusActiveHeadV1 {
    #[cfg(feature = "quanta-native-identity-v1")]
    pub fn native_decode_seed_v1<'a, 'de>(
        admission: &'a dyn NativeCorpusDecodeAdmissionV1,
    ) -> impl DeserializeSeed<'de, Value = Self> + 'a {
        CorpusDecodeModeV1::Native(admission).seed_v1::<Self>()
    }
}
