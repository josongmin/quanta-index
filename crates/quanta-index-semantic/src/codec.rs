//! Manual CBOR (de)serialization helpers for the scope-level semantic
//! manifest.
//!
//! The workspace bans proc-macro serde derives; the manifest gets a
//! hand-written serde impl via the [`cbor_serde`] declarative macro (the same
//! shape as the search-plane authority codecs), driven through `ciborium`.
//! The lancedb dataset has its own on-disk integrity; this codec is *only*
//! used for the scope-metadata manifest written beside it.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use quanta_index_core::CoreError;
use serde::Serialize;
use serde::de::DeserializeOwned;

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
