//! Bounded immutable payload with compact CBOR byte-string encoding.

use super::MAX_NATIVE_IMMUTABLE_OBJECT_WIRE_BYTES_V1;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error as _, SeqAccess, Visitor},
};
use std::{fmt, ops::Deref};

const MAX_PAYLOAD_BYTES_V1: usize = MAX_NATIVE_IMMUTABLE_OBJECT_WIRE_BYTES_V1 as usize;
const TOO_LARGE_V1: &str = "immutable object payload exceeds 64 MiB";

fn valid_len_v1(len: usize) -> Result<(), &'static str> {
    if len > MAX_PAYLOAD_BYTES_V1 {
        Err(TOO_LARGE_V1)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImmutableObjectPayloadV1(Vec<u8>);

impl NativeImmutableObjectPayloadV1 {
    pub fn new(bytes: Vec<u8>) -> Result<Self, &'static str> {
        valid_len_v1(bytes.len())?;
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl Deref for NativeImmutableObjectPayloadV1 {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Serialize for NativeImmutableObjectPayloadV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.as_slice())
    }
}

impl<'de> Deserialize<'de> for NativeImmutableObjectPayloadV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Payload;

        impl<'de> Visitor<'de> for Payload {
            type Value = NativeImmutableObjectPayloadV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("at most 64 MiB of immutable object payload bytes")
            }

            fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                valid_len_v1(value.len()).map_err(E::custom)?;
                let mut bytes = Vec::new();
                bytes.try_reserve_exact(value.len()).map_err(E::custom)?;
                bytes.extend_from_slice(value);
                Ok(NativeImmutableObjectPayloadV1(bytes))
            }

            fn visit_byte_buf<E: serde::de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
                valid_len_v1(value.len()).map_err(E::custom)?;
                Ok(NativeImmutableObjectPayloadV1(value))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                if seq.size_hint().is_some_and(|hint| hint > MAX_PAYLOAD_BYTES_V1) {
                    return Err(A::Error::custom(TOO_LARGE_V1));
                }
                let mut bytes = Vec::new();
                while bytes.len() < MAX_PAYLOAD_BYTES_V1 {
                    let Some(byte) = seq.next_element::<u8>()? else {
                        return Ok(NativeImmutableObjectPayloadV1(bytes));
                    };
                    bytes.try_reserve(1).map_err(A::Error::custom)?;
                    bytes.push(byte);
                }
                if seq.next_element::<u8>()?.is_some() {
                    return Err(A::Error::custom(TOO_LARGE_V1));
                }
                Ok(NativeImmutableObjectPayloadV1(bytes))
            }
        }

        deserializer.deserialize_bytes(Payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_payload_uses_cbor_byte_string_and_preserves_json_bytes() {
        let payload = NativeImmutableObjectPayloadV1::new(b"abc".to_vec()).unwrap();
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&payload, &mut cbor).unwrap();
        assert_eq!(cbor, [0x43, b'a', b'b', b'c']);
        let decoded: NativeImmutableObjectPayloadV1 =
            ciborium::de::from_reader(cbor.as_slice()).unwrap();
        assert_eq!(decoded.as_slice(), b"abc");
        let json = serde_json::to_vec(&payload).unwrap();
        assert_eq!(json, b"[97,98,99]");
        assert_eq!(serde_json::from_slice::<NativeImmutableObjectPayloadV1>(&json).unwrap(), payload);
        assert!(valid_len_v1(MAX_PAYLOAD_BYTES_V1 + 1).is_err());
    }
}
