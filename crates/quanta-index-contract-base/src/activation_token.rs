//! Identity of one activation of a search-corpus head.
//!
//! A generation may become active more than once. The catalog owns the
//! incarnation and sequence; this value only carries that authority across
//! the wire and must never be synthesized from a generation number.

use core::fmt;
use core::num::NonZeroU64;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

pub const ACTIVATION_ROOT_INCARNATION_BYTES_V1: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivationTokenValidationErrorV1 {
    ZeroRootIncarnation,
}

impl fmt::Display for ActivationTokenValidationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroRootIncarnation => formatter.write_str("zero activation root incarnation"),
        }
    }
}

impl std::error::Error for ActivationTokenValidationErrorV1 {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchCorpusActivationTokenV1 {
    root_incarnation: [u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
    activation_sequence: NonZeroU64,
}

impl SearchCorpusActivationTokenV1 {
    pub fn new(
        root_incarnation: [u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
        activation_sequence: NonZeroU64,
    ) -> Result<Self, ActivationTokenValidationErrorV1> {
        if root_incarnation == [0; ACTIVATION_ROOT_INCARNATION_BYTES_V1] {
            return Err(ActivationTokenValidationErrorV1::ZeroRootIncarnation);
        }
        Ok(Self {
            root_incarnation,
            activation_sequence,
        })
    }

    #[must_use]
    pub const fn root_incarnation(self) -> [u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1] {
        self.root_incarnation
    }

    #[must_use]
    pub const fn activation_sequence(self) -> NonZeroU64 {
        self.activation_sequence
    }
}

const ACTIVATION_TOKEN_FIELDS_V1: &[&str] = &["root_incarnation", "activation_sequence"];

impl Serialize for SearchCorpusActivationTokenV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchCorpusActivationTokenV1", 2)?;
        state.serialize_field("root_incarnation", &self.root_incarnation)?;
        state.serialize_field("activation_sequence", &self.activation_sequence)?;
        state.end()
    }
}

/// Finite schema cause at the existing activation-token visitor.
#[cfg(feature = "quanta-native-identity-v1")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeActivationTokenDataFailureV1 {
    UnknownField,
    DuplicateField,
    MissingField,
    Semantic,
}

/// The caller retains its original work refusal; no token authority is issued.
#[cfg(feature = "quanta-native-identity-v1")]
pub trait NativeActivationTokenDecodeAdmissionV1 {
    type Error: fmt::Display;
    fn consume_token_work_v1(&self, units: u64) -> Result<(), Self::Error>;
    fn token_invalid_data_v1(
        &self,
        cause: NativeActivationTokenDataFailureV1,
        field: Option<&'static str>,
    ) -> Self::Error;
}

#[derive(Clone, Copy)]
enum TokenDataFailureV1 {
    UnknownField,
    DuplicateField,
    MissingField,
    Semantic,
}
trait TokenDecodePolicyV1: Copy {
    fn work_v1<E: de::Error>(self, units: u64) -> Result<(), E>;
    fn invalid_v1<E: de::Error>(
        self,
        cause: TokenDataFailureV1,
        field: Option<&'static str>,
        ordinary: impl FnOnce() -> E,
    ) -> E;
    fn ordinary_v1(self) -> bool;
}
#[derive(Clone, Copy)]
struct OrdinaryTokenDecodeV1;
impl TokenDecodePolicyV1 for OrdinaryTokenDecodeV1 {
    fn work_v1<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn invalid_v1<E: de::Error>(
        self,
        _: TokenDataFailureV1,
        _: Option<&'static str>,
        ordinary: impl FnOnce() -> E,
    ) -> E {
        ordinary()
    }
    fn ordinary_v1(self) -> bool {
        true
    }
}
#[cfg(feature = "quanta-native-identity-v1")]
struct NativeTokenDecodeV1<'a, P: ?Sized>(&'a P);
#[cfg(feature = "quanta-native-identity-v1")]
impl<P: ?Sized> Copy for NativeTokenDecodeV1<'_, P> {}
#[cfg(feature = "quanta-native-identity-v1")]
impl<P: ?Sized> Clone for NativeTokenDecodeV1<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<P: NativeActivationTokenDecodeAdmissionV1 + ?Sized> TokenDecodePolicyV1
    for NativeTokenDecodeV1<'_, P>
{
    fn work_v1<E: de::Error>(self, units: u64) -> Result<(), E> {
        self.0.consume_token_work_v1(units).map_err(E::custom)
    }
    fn invalid_v1<E: de::Error>(
        self,
        cause: TokenDataFailureV1,
        field: Option<&'static str>,
        _: impl FnOnce() -> E,
    ) -> E {
        let cause = match cause {
            TokenDataFailureV1::UnknownField => NativeActivationTokenDataFailureV1::UnknownField,
            TokenDataFailureV1::DuplicateField => {
                NativeActivationTokenDataFailureV1::DuplicateField
            }
            TokenDataFailureV1::MissingField => NativeActivationTokenDataFailureV1::MissingField,
            TokenDataFailureV1::Semantic => NativeActivationTokenDataFailureV1::Semantic,
        };
        E::custom(self.0.token_invalid_data_v1(cause, field))
    }
    fn ordinary_v1(self) -> bool {
        false
    }
}
struct TokenKeyV1 {
    known: Option<&'static str>,
    ordinary_unknown: Option<String>,
}
impl TokenKeyV1 {
    fn as_str(&self) -> &str {
        self.known
            .or(self.ordinary_unknown.as_deref())
            .unwrap_or("")
    }
}
struct TokenKeySeedV1<P>(P);
impl<'de, P: TokenDecodePolicyV1> serde::de::DeserializeSeed<'de> for TokenKeySeedV1<P> {
    type Value = TokenKeyV1;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_identifier(self)
    }
}

impl<'de, P: TokenDecodePolicyV1> Visitor<'de> for TokenKeySeedV1<P> {
    type Value = TokenKeyV1;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an activation token field")
    }
    fn visit_str<E: de::Error>(self, key: &str) -> Result<TokenKeyV1, E> {
        let mut known = None;
        for field in ACTIVATION_TOKEN_FIELDS_V1 {
            self.0.work_v1(1)?;
            if key == *field {
                known = Some(*field);
                break;
            }
        }
        let ordinary_unknown = if known.is_none() && self.0.ordinary_v1() {
            Some(key.to_owned())
        } else {
            None
        };
        Ok(TokenKeyV1 {
            known,
            ordinary_unknown,
        })
    }
}

struct SearchCorpusActivationTokenV1Visitor<P>(P);

impl<'de, P: TokenDecodePolicyV1> Visitor<'de> for SearchCorpusActivationTokenV1Visitor<P> {
    type Value = SearchCorpusActivationTokenV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchCorpusActivationTokenV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        self.0.work_v1(1)?;
        let mut root_incarnation = None;
        let mut activation_sequence = None;
        while let Some(key) = map.next_key_seed(TokenKeySeedV1(self.0))? {
            match key.as_str() {
                "root_incarnation" => {
                    if root_incarnation.is_some() {
                        return Err(self.0.invalid_v1(
                            TokenDataFailureV1::DuplicateField,
                            Some("root_incarnation"),
                            || de::Error::duplicate_field("root_incarnation"),
                        ));
                    }
                    self.0.work_v1(17)?;
                    root_incarnation = Some(map.next_value()?);
                }
                "activation_sequence" => {
                    if activation_sequence.is_some() {
                        return Err(self.0.invalid_v1(
                            TokenDataFailureV1::DuplicateField,
                            Some("activation_sequence"),
                            || de::Error::duplicate_field("activation_sequence"),
                        ));
                    }
                    self.0.work_v1(1)?;
                    let sequence: u64 = map.next_value()?;
                    activation_sequence = Some(NonZeroU64::new(sequence).ok_or_else(|| {
                        self.0.invalid_v1(
                            TokenDataFailureV1::Semantic,
                            Some("activation_sequence"),
                            || {
                                de::Error::invalid_value(
                                    de::Unexpected::Unsigned(0),
                                    &"a nonzero u64",
                                )
                            },
                        )
                    })?);
                }
                other => {
                    return Err(self
                        .0
                        .invalid_v1(TokenDataFailureV1::UnknownField, None, || {
                            de::Error::unknown_field(other, ACTIVATION_TOKEN_FIELDS_V1)
                        }));
                }
            }
        }
        let root_incarnation = root_incarnation.ok_or_else(|| {
            self.0.invalid_v1(
                TokenDataFailureV1::MissingField,
                Some("root_incarnation"),
                || de::Error::missing_field("root_incarnation"),
            )
        })?;
        let activation_sequence = activation_sequence.ok_or_else(|| {
            self.0.invalid_v1(
                TokenDataFailureV1::MissingField,
                Some("activation_sequence"),
                || de::Error::missing_field("activation_sequence"),
            )
        })?;
        self.0.work_v1(16)?;
        SearchCorpusActivationTokenV1::new(root_incarnation, activation_sequence).map_err(|cause| {
            self.0.invalid_v1(
                TokenDataFailureV1::Semantic,
                Some("root_incarnation"),
                || de::Error::custom(cause),
            )
        })
    }
}

impl SearchCorpusActivationTokenV1 {
    fn decode_with_policy_v1<'de, D: Deserializer<'de>, P: TokenDecodePolicyV1>(
        deserializer: D,
        policy: P,
    ) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "SearchCorpusActivationTokenV1",
            ACTIVATION_TOKEN_FIELDS_V1,
            SearchCorpusActivationTokenV1Visitor(policy),
        )
    }
    #[cfg(feature = "quanta-native-identity-v1")]
    pub fn native_decode_seed_v1<'a, 'de, P: NativeActivationTokenDecodeAdmissionV1 + ?Sized>(
        admission: &'a P,
    ) -> impl serde::de::DeserializeSeed<'de, Value = Self> + 'a {
        TokenDecodeSeedV1(NativeTokenDecodeV1(admission))
    }
}
#[cfg(feature = "quanta-native-identity-v1")]
struct TokenDecodeSeedV1<P>(P);
#[cfg(feature = "quanta-native-identity-v1")]
impl<'de, P: TokenDecodePolicyV1> serde::de::DeserializeSeed<'de> for TokenDecodeSeedV1<P> {
    type Value = SearchCorpusActivationTokenV1;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        Self::Value::decode_with_policy_v1(d, self.0)
    }
}
impl<'de> Deserialize<'de> for SearchCorpusActivationTokenV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::decode_with_policy_v1(deserializer, OrdinaryTokenDecodeV1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn activation_token_round_trips_and_rejects_invalid_wire() {
        let token = SearchCorpusActivationTokenV1::new(
            [7; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
            NonZeroU64::new(3).expect("fixture is positive"),
        )
        .expect("fixture incarnation is nonzero");
        let wire = to_value(token).expect("serialize activation token");
        assert_eq!(
            serde_json::from_value::<SearchCorpusActivationTokenV1>(wire)
                .expect("decode activation token"),
            token
        );
        let invalid = [
            json!({"root_incarnation": vec![0; 16], "activation_sequence": 3}),
            json!({"root_incarnation": vec![7; 16], "activation_sequence": 0}),
            json!({"root_incarnation": vec![7; 15], "activation_sequence": 3}),
            json!({"root_incarnation": vec![7; 17], "activation_sequence": 3}),
            json!({"root_incarnation": vec![7; 16]}),
            json!({"root_incarnation": vec![7; 16], "activation_sequence": 3, "extra": 1}),
        ];
        for value in invalid {
            assert!(
                serde_json::from_value::<SearchCorpusActivationTokenV1>(value).is_err(),
                "invalid activation token must be refused"
            );
        }
        assert!(
            serde_json::from_str::<SearchCorpusActivationTokenV1>(
                r#"{"root_incarnation":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7],"activation_sequence":3,"activation_sequence":4}"#,
            )
            .is_err(),
            "duplicate activation sequence must be refused"
        );
    }
}

#[cfg(all(test, feature = "quanta-native-identity-v1"))]
mod native_token_decode_tests_v1 {
    use super::*;
    use core::cell::Cell;
    use serde::de::DeserializeSeed as _;

    #[derive(Default)]
    struct Admission {
        invalid: Cell<Option<(NativeActivationTokenDataFailureV1, Option<&'static str>)>>,
    }

    impl NativeActivationTokenDecodeAdmissionV1 for Admission {
        type Error = &'static str;

        fn consume_token_work_v1(&self, _units: u64) -> Result<(), Self::Error> {
            Ok(())
        }

        fn token_invalid_data_v1(
            &self,
            cause: NativeActivationTokenDataFailureV1,
            field: Option<&'static str>,
        ) -> Self::Error {
            self.invalid.set(Some((cause, field)));
            "native token semantic refusal"
        }
    }

    #[test]
    fn native_sequence_boundaries_preserve_semantic_and_ordinary_errors_v1() {
        fn token_wire(sequence: u64) -> String {
            format!(
                r#"{{"root_incarnation":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7],"activation_sequence":{sequence}}}"#
            )
        }

        let wire = token_wire(0);
        let admission = Admission::default();
        let mut deserializer = serde_json::Deserializer::from_str(&wire);
        let error = SearchCorpusActivationTokenV1::native_decode_seed_v1(&admission)
            .deserialize(&mut deserializer)
            .unwrap_err();
        assert!(error.to_string().contains("native token semantic refusal"));
        assert_eq!(
            admission.invalid.get(),
            Some((
                NativeActivationTokenDataFailureV1::Semantic,
                Some("activation_sequence")
            ))
        );
        let ordinary = serde_json::from_str::<SearchCorpusActivationTokenV1>(&wire).unwrap_err();
        assert!(ordinary.to_string().contains("expected a nonzero u64"));

        for sequence in [1, u64::MAX] {
            let wire = token_wire(sequence);
            let admission = Admission::default();
            let mut deserializer = serde_json::Deserializer::from_str(&wire);
            let token = SearchCorpusActivationTokenV1::native_decode_seed_v1(&admission)
                .deserialize(&mut deserializer)
                .expect("nonzero sequence is admitted");
            deserializer.end().expect("whole token consumed");
            assert_eq!(token.activation_sequence().get(), sequence);
            assert_eq!(admission.invalid.get(), None);
        }
    }
}
