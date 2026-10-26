// SPDX-License-Identifier: Apache-2.0

use crate::value::Value;
use alloc::vec::Vec;
use core::cmp::Ordering;
use serde::{de, ser};

/// Manually serialize values to compare them.
fn serialized_canonical_cmp(v1: &Value, v2: &Value) -> Ordering {
    // There is an optimization to be done here, but it would take a lot more code
    // and using mixing keys, Arrays or Maps as CanonicalValue is probably not the
    // best use of this type as it is meant mainly to be used as keys.

    let mut bytes1 = Vec::new();
    drop(crate::ser::into_writer(v1, &mut bytes1));
    let mut bytes2 = Vec::new();
    drop(crate::ser::into_writer(v2, &mut bytes2));

    match bytes1.len().cmp(&bytes2.len()) {
        Ordering::Equal => bytes1.cmp(&bytes2),
        x @ (Ordering::Less | Ordering::Greater) => x,
    }
}

/// Compares two values uses canonical comparison, as defined in both
/// RFC 7049 Section 3.9 (regarding key sorting) and RFC 8949 4.2.3 (as errata).
///
/// In short, the comparison follow the following rules:
///   - If two keys have different lengths, the shorter one sorts earlier;
///   - If two keys have the same length, the one with the lower value in
///     (byte-wise) lexical order sorts earlier.
///
/// This specific comparison allows Maps and sorting that respect these two rules.
pub(crate) fn cmp_value(v1: &Value, v2: &Value) -> Ordering {
    if let (Value::Integer(i), Value::Integer(o)) = (v1, v2) {
        // CBOR length and sign ordering differ from ordinary i128 ordering.
        return i.canonical_cmp(o);
    }
    if let (Value::Text(s), Value::Text(o)) = (v1, v2) {
        return match s.len().cmp(&o.len()) {
            Ordering::Equal => s.cmp(o),
            x @ (Ordering::Less | Ordering::Greater) => x,
        };
    }
    if let (Value::Bool(s), Value::Bool(o)) = (v1, v2) {
        return s.cmp(o);
    }
    if matches!((v1, v2), (Value::Null, Value::Null)) {
        return Ordering::Equal;
    }
    if let (Value::Tag(t, v), Value::Tag(ot, ov)) = (v1, v2) {
        return Value::from(*t)
            .partial_cmp(&Value::from(*ot))
            .filter(|order| *order != Ordering::Equal)
            .unwrap_or_else(|| {
                v.partial_cmp(ov)
                    .unwrap_or_else(|| serialized_canonical_cmp(v1, v2))
            });
    }
    serialized_canonical_cmp(v1, v2)
}

/// A CBOR Value that impl Ord and Eq to allow sorting of values as defined in both
/// RFC 7049 Section 3.9 (regarding key sorting) and RFC 8949 4.2.3 (as errata).
///
/// Since a regular [Value] can be
#[derive(Clone, Debug)]
pub struct CanonicalValue(Value);

impl PartialEq for CanonicalValue {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for CanonicalValue {}

impl From<Value> for CanonicalValue {
    fn from(v: Value) -> Self {
        Self(v)
    }
}

impl From<CanonicalValue> for Value {
    fn from(v: CanonicalValue) -> Self {
        v.0
    }
}

impl ser::Serialize for CanonicalValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> de::Deserialize<'de> for CanonicalValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        Value::deserialize(deserializer).map(Into::into)
    }

    fn deserialize_in_place<D>(deserializer: D, place: &mut Self) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        Value::deserialize_in_place(deserializer, &mut place.0)
    }
}

impl Ord for CanonicalValue {
    fn cmp(&self, other: &Self) -> Ordering {
        cmp_value(&self.0, &other.0)
    }
}

impl PartialOrd for CanonicalValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
