//! Identity of one activation of a search-corpus head.
//!
//! A generation may become active more than once. The catalog owns the
//! incarnation and sequence; this value only carries that authority across
//! the wire and must never be synthesized from a generation number.

use crate::retained_scalar_decode_v1::{ScalarDataV1, decode_scalar_into_v1};
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

/// Work admission with its complete original error separate from the finite
/// Serde-facing schema marker. The adapter stores no token authority.
#[cfg(feature = "quanta-native-identity-v1")]
pub trait NativeActivationTokenDecodeAdmissionV1 {
    type ControlError;
    type Error: fmt::Display + Copy;
    fn consume_token_work_v1(&mut self, units: u64) -> Result<(), Self::ControlError>;
    fn token_work_refusal_v1(&self) -> Self::Error;
    fn token_invalid_data_v1(
        &self,
        cause: NativeActivationTokenDataFailureV1,
        field: Option<&'static str>,
    ) -> Self::Error;
}

struct TokenDecodeStateV1<E> {
    output: Option<SearchCorpusActivationTokenV1>,
    root_incarnation: ScalarDataV1<[u8; ACTIVATION_ROOT_INCARNATION_BYTES_V1]>,
    activation_sequence: ScalarDataV1<u64>,
    refused_map: Option<String>,
    owned_keys: [Option<String>; 2],
    pending_key: Option<String>,
    seen: [bool; 2],
    work_failure: Option<E>,
}
impl<E> TokenDecodeStateV1<E> {
    const fn new_v1() -> Self {
        Self {
            output: None,
            root_incarnation: ScalarDataV1::new_v1(),
            activation_sequence: ScalarDataV1::new_v1(),
            refused_map: None,
            owned_keys: [None, None],
            pending_key: None,
            seen: [false; 2],
            work_failure: None,
        }
    }
}

/// Physical state of one token occurrence.
///
/// Every owned key, partial field,
/// candidate, and complete work error stays here through the Source finisher.
/// The caller retains the deserializer's full error in its external error slot.
#[cfg(feature = "quanta-native-identity-v1")]
pub struct NativeActivationTokenDecodeDataV1<E> {
    state: TokenDecodeStateV1<E>,
    attempted: bool,
    completed: bool,
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<E> NativeActivationTokenDecodeDataV1<E> {
    #[must_use]
    pub const fn new_v1() -> Self {
        Self {
            state: TokenDecodeStateV1::new_v1(),
            attempted: false,
            completed: false,
        }
    }
    #[must_use]
    pub const fn is_fresh_v1(&self) -> bool {
        !self.attempted
    }
    #[must_use]
    pub fn work_failure_v1(&self) -> Option<&E> {
        self.state.work_failure.as_ref()
    }
    /// Pure transfer to another external slot. Keep this DATA until finishing.
    pub fn complete_into_slot_v1(
        &mut self,
        output: &mut Option<SearchCorpusActivationTokenV1>,
    ) -> Result<(), crate::NativeIdentityDecodeDataRefusalV1> {
        if output.is_some() {
            return Err(crate::NativeIdentityDecodeDataRefusalV1::OccupiedOutput);
        }
        if !self.completed || self.state.work_failure.is_some() || self.state.output.is_none() {
            return Err(crate::NativeIdentityDecodeDataRefusalV1::MissingResult);
        }
        *output = self.state.output.take();
        Ok(())
    }
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<E> Default for NativeActivationTokenDecodeDataV1<E> {
    fn default() -> Self {
        Self::new_v1()
    }
}

#[derive(Clone, Copy)]
enum TokenDataFailureV1 {
    UnknownField,
    DuplicateField,
    MissingField,
    Semantic,
}
trait TokenDecodePolicyV1 {
    type OriginalError;
    fn dynamic_input_v1(&self) -> bool;
    fn work_v1<E: de::Error>(
        &mut self,
        failure: &mut Option<Self::OriginalError>,
        units: u64,
    ) -> Result<(), E>;
    fn invalid_v1<E: de::Error>(
        &self,
        cause: TokenDataFailureV1,
        field: Option<&'static str>,
        ordinary: impl FnOnce() -> E,
    ) -> E;
}
struct OrdinaryTokenDecodeV1;
impl TokenDecodePolicyV1 for OrdinaryTokenDecodeV1 {
    type OriginalError = core::convert::Infallible;
    fn dynamic_input_v1(&self) -> bool {
        false
    }
    fn work_v1<E: de::Error>(
        &mut self,
        _: &mut Option<Self::OriginalError>,
        _: u64,
    ) -> Result<(), E> {
        Ok(())
    }
    fn invalid_v1<E: de::Error>(
        &self,
        _: TokenDataFailureV1,
        _: Option<&'static str>,
        ordinary: impl FnOnce() -> E,
    ) -> E {
        ordinary()
    }
}
#[cfg(feature = "quanta-native-identity-v1")]
struct NativeTokenDecodeV1<'a, P: ?Sized>(&'a mut P);
#[cfg(feature = "quanta-native-identity-v1")]
impl<P: NativeActivationTokenDecodeAdmissionV1 + ?Sized> TokenDecodePolicyV1
    for NativeTokenDecodeV1<'_, P>
{
    type OriginalError = P::ControlError;
    fn dynamic_input_v1(&self) -> bool {
        true
    }
    fn work_v1<E: de::Error>(
        &mut self,
        failure: &mut Option<Self::OriginalError>,
        units: u64,
    ) -> Result<(), E> {
        if let Err(cause) = self.0.consume_token_work_v1(units) {
            *failure = Some(cause);
            return Err(E::custom(self.0.token_work_refusal_v1()));
        }
        Ok(())
    }
    fn invalid_v1<E: de::Error>(
        &self,
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
}
struct TokenKeySeedV1<'a, P: TokenDecodePolicyV1> {
    policy: &'a mut P,
    state: &'a mut TokenDecodeStateV1<P::OriginalError>,
}
impl<'de, P: TokenDecodePolicyV1> serde::de::DeserializeSeed<'de> for TokenKeySeedV1<'_, P> {
    type Value = usize;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<usize, D::Error> {
        d.deserialize_identifier(self)
    }
}
impl<P: TokenDecodePolicyV1> TokenKeySeedV1<'_, P> {
    fn classify_v1<E: de::Error>(&mut self, borrowed: Option<&str>) -> Result<usize, E> {
        let key = borrowed.or(self.state.pending_key.as_deref()).unwrap_or("");
        for (index, ((field, seen), owned)) in ACTIVATION_TOKEN_FIELDS_V1
            .iter()
            .zip(self.state.seen.iter_mut())
            .zip(self.state.owned_keys.iter_mut())
            .enumerate()
        {
            self.policy.work_v1(&mut self.state.work_failure, 1)?;
            if key == *field {
                if *seen {
                    return Err(self.policy.invalid_v1(
                        TokenDataFailureV1::DuplicateField,
                        Some(field),
                        || E::duplicate_field(field),
                    ));
                }
                *seen = true;
                *owned = self.state.pending_key.take();
                return Ok(index);
            }
        }
        Err(self
            .policy
            .invalid_v1(TokenDataFailureV1::UnknownField, None, || {
                E::unknown_field(key, ACTIVATION_TOKEN_FIELDS_V1)
            }))
    }
}
impl<P: TokenDecodePolicyV1> Visitor<'_> for TokenKeySeedV1<'_, P> {
    type Value = usize;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an activation token field")
    }
    fn visit_str<E: de::Error>(mut self, key: &str) -> Result<usize, E> {
        self.classify_v1(Some(key))
    }
    fn visit_string<E: de::Error>(mut self, key: String) -> Result<usize, E> {
        self.state.pending_key = Some(key);
        self.classify_v1(None)
    }
}
struct SearchCorpusActivationTokenV1Visitor<'a, P: TokenDecodePolicyV1> {
    policy: &'a mut P,
    state: &'a mut TokenDecodeStateV1<P::OriginalError>,
}
impl<'de, P: TokenDecodePolicyV1> Visitor<'de> for SearchCorpusActivationTokenV1Visitor<'_, P> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a SearchCorpusActivationTokenV1 map")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<(), E> {
        Err(E::invalid_type(de::Unexpected::Str(value), &self))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<(), E> {
        self.state.refused_map = Some(value);
        Err(E::invalid_type(
            de::Unexpected::Str(self.state.refused_map.as_deref().unwrap_or("")),
            &self,
        ))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        self.policy.work_v1(&mut self.state.work_failure, 1)?;
        while let Some(key) = map.next_key_seed(TokenKeySeedV1 {
            policy: &mut *self.policy,
            state: &mut *self.state,
        })? {
            match key {
                0 => {
                    self.policy.work_v1(&mut self.state.work_failure, 17)?;
                    map.next_value_seed(TokenScalarSeedV1(
                        &mut self.state.root_incarnation,
                        self.policy.dynamic_input_v1(),
                    ))?;
                }
                1 => {
                    self.policy.work_v1(&mut self.state.work_failure, 1)?;
                    map.next_value_seed(TokenScalarSeedV1(
                        &mut self.state.activation_sequence,
                        self.policy.dynamic_input_v1(),
                    ))?;
                    if self.state.activation_sequence.output == Some(0) {
                        return Err(self.policy.invalid_v1(
                            TokenDataFailureV1::Semantic,
                            Some("activation_sequence"),
                            || {
                                de::Error::invalid_value(
                                    de::Unexpected::Unsigned(0),
                                    &"a nonzero u64",
                                )
                            },
                        ));
                    }
                }
                _ => return Err(de::Error::custom("invalid canonical token field index")),
            }
        }
        let root = self.state.root_incarnation.output.ok_or_else(|| {
            self.policy.invalid_v1(
                TokenDataFailureV1::MissingField,
                Some("root_incarnation"),
                || de::Error::missing_field("root_incarnation"),
            )
        })?;
        let sequence = self
            .state
            .activation_sequence
            .output
            .and_then(NonZeroU64::new)
            .ok_or_else(|| {
                self.policy.invalid_v1(
                    TokenDataFailureV1::MissingField,
                    Some("activation_sequence"),
                    || de::Error::missing_field("activation_sequence"),
                )
            })?;
        self.state.output = Some(SearchCorpusActivationTokenV1::new(root, sequence).map_err(
            |cause| {
                self.policy
                    .invalid_v1(TokenDataFailureV1::Semantic, None, || {
                        de::Error::custom(cause)
                    })
            },
        )?);
        Ok(())
    }
}
struct TokenScalarSeedV1<'a, T>(&'a mut ScalarDataV1<T>, bool);
impl<'de, T: Deserialize<'de> + Copy> serde::de::DeserializeSeed<'de> for TokenScalarSeedV1<'_, T> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        decode_scalar_into_v1(d, self.0, self.1)
    }
}
fn decode_token_into_v1<'de, D: Deserializer<'de>, P: TokenDecodePolicyV1>(
    d: D,
    policy: &mut P,
    state: &mut TokenDecodeStateV1<P::OriginalError>,
) -> Result<(), D::Error> {
    let dynamic = policy.dynamic_input_v1();
    let visitor = SearchCorpusActivationTokenV1Visitor { policy, state };
    if dynamic {
        d.deserialize_any(visitor)
    } else {
        d.deserialize_struct(
            "SearchCorpusActivationTokenV1",
            ACTIVATION_TOKEN_FIELDS_V1,
            visitor,
        )
    }
}
#[cfg(feature = "quanta-native-identity-v1")]
struct TokenDecodeSeedV1<'a, P: NativeActivationTokenDecodeAdmissionV1 + ?Sized> {
    admission: &'a mut P,
    data: &'a mut NativeActivationTokenDecodeDataV1<P::ControlError>,
}
#[cfg(feature = "quanta-native-identity-v1")]
impl<'de, P: NativeActivationTokenDecodeAdmissionV1 + ?Sized> serde::de::DeserializeSeed<'de>
    for TokenDecodeSeedV1<'_, P>
{
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        if self.data.attempted {
            return Err(de::Error::custom(
                "native activation token DATA is already used",
            ));
        }
        self.data.attempted = true;
        decode_token_into_v1(
            d,
            &mut NativeTokenDecodeV1(self.admission),
            &mut self.data.state,
        )?;
        self.data.completed = true;
        Ok(())
    }
}
impl SearchCorpusActivationTokenV1 {
    /// Park the complete decoder error before returning a finite unit status.
    #[cfg(feature = "quanta-native-identity-v1")]
    pub fn try_decode_into_v1<
        'de,
        D: Deserializer<'de>,
        P: NativeActivationTokenDecodeAdmissionV1 + ?Sized,
    >(
        deserializer: D,
        admission: &mut P,
        data: &mut NativeActivationTokenDecodeDataV1<P::ControlError>,
        failure: &mut Option<D::Error>,
    ) -> Result<(), crate::NativeIdentityDecodeDataRefusalV1> {
        use crate::NativeIdentityDecodeDataRefusalV1 as Refusal;
        if failure.is_some() {
            return Err(Refusal::OccupiedOutput);
        }
        if !data.is_fresh_v1() {
            return Err(Refusal::UsedData);
        }
        match serde::de::DeserializeSeed::deserialize(
            Self::native_decode_seed_v1(admission, data),
            deserializer,
        ) {
            Ok(()) => Ok(()),
            Err(cause) => {
                *failure = Some(cause);
                Err(Refusal::OperationRefused)
            }
        }
    }
    /// Decode one occurrence into caller-owned DATA. Capture the full returned
    /// deserializer error outside the Source before invoking its finisher.
    #[cfg(feature = "quanta-native-identity-v1")]
    pub fn native_decode_seed_v1<'data, 'de, P: NativeActivationTokenDecodeAdmissionV1 + ?Sized>(
        admission: &'data mut P,
        data: &'data mut NativeActivationTokenDecodeDataV1<P::ControlError>,
    ) -> impl serde::de::DeserializeSeed<'de, Value = ()> + 'data {
        TokenDecodeSeedV1 { admission, data }
    }
}
impl<'de> Deserialize<'de> for SearchCorpusActivationTokenV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut state = TokenDecodeStateV1::new_v1();
        decode_token_into_v1(d, &mut OrdinaryTokenDecodeV1, &mut state)?;
        state
            .output
            .ok_or_else(|| de::Error::custom("activation token visitor produced no result"))
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
        type ControlError = Box<u8>;
        type Error = &'static str;
        fn token_work_refusal_v1(&self) -> Self::Error {
            "native token work refusal"
        }

        fn consume_token_work_v1(&mut self, _units: u64) -> Result<(), Self::ControlError> {
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
        let mut admission = Admission::default();
        let mut data = NativeActivationTokenDecodeDataV1::new_v1();
        let mut deserializer = serde_json::Deserializer::from_str(&wire);
        let error = SearchCorpusActivationTokenV1::native_decode_seed_v1(&mut admission, &mut data)
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
            let mut admission = Admission::default();
            let mut data = NativeActivationTokenDecodeDataV1::new_v1();
            let mut deserializer = serde_json::Deserializer::from_str(&wire);
            SearchCorpusActivationTokenV1::native_decode_seed_v1(&mut admission, &mut data)
                .deserialize(&mut deserializer)
                .expect("nonzero sequence is admitted");
            deserializer.end().expect("whole token consumed");
            let mut output = None;
            data.complete_into_slot_v1(&mut output)
                .expect("complete token");
            let token = output.expect("token output");
            assert_eq!(token.activation_sequence().get(), sequence);
            assert_eq!(admission.invalid.get(), None);
        }
    }
    #[test]
    fn owned_token_keys_and_wrong_owned_array_or_sequence_values_remain_external_v1() {
        for wire in [
            r#"{"activation_sequence":"owned-refused"}"#,
            r#"{"root_incarnation":[7,7,"owned-refused"]}"#,
        ] {
            let value: serde_json::Value = serde_json::from_str(wire).expect("fixture");
            let mut data = NativeActivationTokenDecodeDataV1::new_v1();
            let mut admission = Admission::default();
            let mut error = None;
            assert!(
                SearchCorpusActivationTokenV1::try_decode_into_v1(
                    value,
                    &mut admission,
                    &mut data,
                    &mut error
                )
                .is_err()
            );
            assert!(error.is_some());
            assert!(data.state.owned_keys.iter().any(Option::is_some));
            let refused = data
                .state
                .activation_sequence
                .refused_string
                .as_ref()
                .or(data.state.root_incarnation.refused_string.as_ref());
            assert_eq!(refused.map(String::as_str), Some("owned-refused"));
            let mut output = None;
            assert!(data.complete_into_slot_v1(&mut output).is_err());
        }
    }
}
