//! The read epoch of one auxiliary authority (QI-BB-020 W2).
//!
//! Every durable mutation of a history, runtime-metadata or structural
//! authority generation produces a new immutable snapshot and names it
//! with the next epoch of that `(repo, revision, generation, domain)`.
//! A response that read an auxiliary authority says which epoch it read;
//! a keyset continuation carries that epoch back so the next page is cut
//! from the same snapshot, never from a newer one where a row could
//! appear twice or not at all.
//!
//! Epochs are monotone within one search-plane state root: they are
//! persisted with the rows they stamp, so a restart continues the
//! sequence rather than reusing a number for different content. They are
//! not comparable across state roots.

use core::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// The epoch of one auxiliary authority snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AuxEpochV1(u64);

impl AuxEpochV1 {
    /// The epoch of an authority that no stamped mutation has produced
    /// yet: a generation restored from rows written before epochs were
    /// stamped, or one that has only been created in memory.
    pub const GENESIS: Self = Self(0);

    #[must_use]
    pub const fn new(epoch: u64) -> Self {
        Self(epoch)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The epoch the next mutation produces, or `None` when the sequence
    /// is exhausted; a caller must refuse rather than wrap or saturate,
    /// since either would reuse an epoch for different content.
    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }
}

impl fmt::Display for AuxEpochV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl Serialize for AuxEpochV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(self.0)
    }
}

struct AuxEpochV1Visitor;

impl de::Visitor<'_> for AuxEpochV1Visitor {
    type Value = AuxEpochV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an auxiliary epoch as an unsigned 64-bit integer")
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(AuxEpochV1(value))
    }
}

impl<'de> Deserialize<'de> for AuxEpochV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_u64(AuxEpochV1Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::AuxEpochV1;

    #[test]
    fn epoch_round_trips_as_a_bare_unsigned_integer() {
        let epoch = AuxEpochV1::new(42);
        let json = serde_json::to_value(epoch).expect("epoch serializes");
        assert_eq!(json, serde_json::json!(42));
        let decoded: AuxEpochV1 = serde_json::from_value(json).expect("epoch decodes");
        assert_eq!(decoded, epoch);

        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&epoch, &mut cbor).expect("epoch encodes to CBOR");
        let decoded: AuxEpochV1 =
            ciborium::de::from_reader(cbor.as_slice()).expect("epoch decodes from CBOR");
        assert_eq!(decoded, epoch);
    }

    #[test]
    fn epoch_refuses_every_shape_but_an_unsigned_integer() {
        for wrong in [
            serde_json::json!(-1),
            serde_json::json!("7"),
            serde_json::json!(7.5),
            serde_json::json!(null),
            serde_json::json!({ "epoch": 7 }),
        ] {
            assert!(
                serde_json::from_value::<AuxEpochV1>(wrong.clone()).is_err(),
                "{wrong} must not decode as an epoch"
            );
        }
    }

    #[test]
    fn next_is_checked_not_wrapping() {
        assert_eq!(AuxEpochV1::GENESIS.checked_next(), Some(AuxEpochV1::new(1)));
        assert_eq!(AuxEpochV1::new(u64::MAX).checked_next(), None);
        assert!(AuxEpochV1::new(3) < AuxEpochV1::new(4));
    }
}
