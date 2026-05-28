//! Manual CBOR (de)serialization helpers for the persisted semantic store.
//!
//! The workspace bans proc-macro serde derives; durable shapes here get
//! hand-written serde impls via the [`cbor_serde`] declarative macro (same
//! shape as the search-plane authority codecs), driven through `ciborium`.
//! Nothing in this module performs unchecked arithmetic, indexing, or `as`
//! casts so it satisfies the crate's deny-level lint posture.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use quanta_index_core::CoreError;
use serde::Serialize;
use serde::de::DeserializeOwned;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Generate manual `serde::Serialize` / `serde::Deserialize` impls for a struct.
///
/// Applies to a flat named-field struct. Field order is the wire order; every
/// field is required on decode (a missing field fails closed rather than
/// defaulting).
macro_rules! cbor_serde {
    ($ty:ident { $($field:ident : $field_ty:ty),+ $(,)? }) => {
        impl serde::Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                use serde::ser::SerializeStruct as _;
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                let mut state =
                    serializer.serialize_struct(stringify!($ty), FIELDS.len())?;
                $(state.serialize_field(stringify!($field), &self.$field)?;)+
                state.end()
            }
        }

        impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct StructVisitor;

                impl<'de> serde::de::Visitor<'de> for StructVisitor {
                    type Value = $ty;

                    fn expecting(
                        &self,
                        formatter: &mut core::fmt::Formatter<'_>,
                    ) -> core::fmt::Result {
                        formatter.write_str(concat!("struct ", stringify!($ty)))
                    }

                    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                    where
                        A: serde::de::MapAccess<'de>,
                    {
                        const FIELDS: &[&str] = &[$(stringify!($field)),+];
                        $(let mut $field: Option<$field_ty> = None;)+
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(
                                    stringify!($field) => {
                                        if $field.is_some() {
                                            return Err(serde::de::Error::duplicate_field(
                                                stringify!($field),
                                            ));
                                        }
                                        $field = Some(map.next_value()?);
                                    }
                                )+
                                _ => {
                                    return Err(serde::de::Error::unknown_field(
                                        key.as_str(),
                                        FIELDS,
                                    ));
                                }
                            }
                        }
                        Ok($ty {
                            $(
                                $field: $field.ok_or_else(|| {
                                    serde::de::Error::missing_field(stringify!($field))
                                })?,
                            )+
                        })
                    }
                }

                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                deserializer.deserialize_struct(stringify!($ty), FIELDS, StructVisitor)
            }
        }
    };
}

pub(crate) use cbor_serde;

/// Encode a value to CBOR bytes, mapping codec failure to a typed storage error.
pub(crate) fn encode<T: Serialize>(value: &T, label: &str) -> Result<Vec<u8>, CoreError> {
    let mut buffer = Vec::new();
    ciborium::into_writer(value, &mut buffer)
        .map_err(|err| CoreError::Storage(format!("semantic: encode {label}: {err}")))?;
    Ok(buffer)
}

/// Decode CBOR bytes into a value, mapping codec failure to a typed storage error.
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8], label: &str) -> Result<T, CoreError> {
    ciborium::from_reader(bytes)
        .map_err(|err| CoreError::Storage(format!("semantic: decode {label}: {err}")))
}

/// Pack `f32` lanes as little-endian bytes for compact, deterministic storage.
pub(crate) fn f32_slice_to_le_bytes(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len().saturating_mul(4));
    for lane in vector {
        bytes.extend_from_slice(&lane.to_le_bytes());
    }
    bytes
}

/// Unpack little-endian `f32` lanes; rejects a length that is not a multiple of 4.
pub(crate) fn le_bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>, CoreError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(CoreError::Storage(format!(
            "semantic: vector byte length {} is not a multiple of 4",
            bytes.len()
        )));
    }
    let mut vector = Vec::with_capacity(bytes.len().saturating_div(4));
    for lane in bytes.chunks_exact(4) {
        let window: [u8; 4] = lane
            .try_into()
            .map_err(|err| CoreError::Storage(format!("semantic: vector lane slice: {err}")))?;
        vector.push(f32::from_le_bytes(window));
    }
    Ok(vector)
}

fn fnv1a64_fold(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// FNV-1a 64-bit content checksum over an ordered set of byte parts.
///
/// Each part is length-framed (its byte length folded in before its bytes,
/// preceded by the part count) so moving bytes across a part boundary changes
/// the digest — a plain concatenation would alias such truncations.
/// Deterministic and infallible by construction; used to fail closed on a
/// corrupted or truncated persisted generation, not for cryptographic purposes.
pub(crate) fn content_checksum(parts: &[&[u8]]) -> String {
    let mut hash = FNV_OFFSET;
    hash = fnv1a64_fold(hash, &parts.len().to_le_bytes());
    for part in parts {
        hash = fnv1a64_fold(hash, &part.len().to_le_bytes());
        hash = fnv1a64_fold(hash, part);
    }
    format!("{hash:016x}")
}
