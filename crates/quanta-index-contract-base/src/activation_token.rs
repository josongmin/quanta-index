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

struct SearchCorpusActivationTokenV1Visitor;

impl<'de> Visitor<'de> for SearchCorpusActivationTokenV1Visitor {
    type Value = SearchCorpusActivationTokenV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchCorpusActivationTokenV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut root_incarnation = None;
        let mut activation_sequence = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "root_incarnation" => {
                    if root_incarnation.is_some() {
                        return Err(de::Error::duplicate_field("root_incarnation"));
                    }
                    root_incarnation = Some(map.next_value()?);
                }
                "activation_sequence" => {
                    if activation_sequence.is_some() {
                        return Err(de::Error::duplicate_field("activation_sequence"));
                    }
                    activation_sequence = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, ACTIVATION_TOKEN_FIELDS_V1)),
            }
        }
        SearchCorpusActivationTokenV1::new(
            root_incarnation.ok_or_else(|| de::Error::missing_field("root_incarnation"))?,
            activation_sequence.ok_or_else(|| de::Error::missing_field("activation_sequence"))?,
        )
        .map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for SearchCorpusActivationTokenV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchCorpusActivationTokenV1",
            ACTIVATION_TOKEN_FIELDS_V1,
            SearchCorpusActivationTokenV1Visitor,
        )
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
