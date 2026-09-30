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
    let mut input = std::io::Cursor::new(bytes);
    let value = ciborium::from_reader(&mut input)
        .map_err(|err| CoreError::Storage(format!("semantic: decode {label}: {err}")))?;
    if usize::try_from(input.position()) != Ok(bytes.len()) {
        return Err(CoreError::Storage(format!(
            "semantic: decode {label}: trailing bytes"
        )));
    }
    Ok(value)
}

/// Wire code for a manifest, contract or sealed manifest written under a
/// format this adapter does not serve.
pub(crate) const FORMAT_UNSUPPORTED_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::GenerationManifestFormatUnsupported;

/// The typed refusal for `format_version` of `what`.
pub(crate) fn format_unsupported(what: &str, format_version: u32, supported: u32) -> CoreError {
    CoreError::Typed {
        code: FORMAT_UNSUPPORTED_CODE,
        message: format!(
            "semantic: {what} has format version {format_version}; this adapter serves format {supported} only — rebuild the generation from its producer"
        ),
    }
}

/// The `format_version` of a manifest-shaped CBOR map, read without
/// decoding the rest, so a foreign format is named typed rather than as a
/// decode failure of the current shape.
struct FormatVersionProbe(u32);

impl<'de> serde::Deserialize<'de> for FormatVersionProbe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ProbeVisitor;

        impl<'de> serde::de::Visitor<'de> for ProbeVisitor {
            type Value = FormatVersionProbe;

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("a map carrying `format_version`")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut format_version: Option<u32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "format_version" {
                        if format_version.is_some() {
                            return Err(serde::de::Error::duplicate_field("format_version"));
                        }
                        format_version = Some(map.next_value()?);
                    } else {
                        let _ignored: serde::de::IgnoredAny = map.next_value()?;
                    }
                }
                format_version
                    .map(FormatVersionProbe)
                    .ok_or_else(|| serde::de::Error::missing_field("format_version"))
            }
        }

        deserializer.deserialize_map(ProbeVisitor)
    }
}

/// Decode `bytes` as the current `what`, refusing any other format typed.
///
/// The format is probed first: bytes that decode as a map naming another
/// `format_version` are refused as [`FORMAT_UNSUPPORTED_CODE`]; bytes that
/// name the current format but do not decode as its shape are corrupt.
pub(crate) fn decode_current_format<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    what: &str,
    supported: u32,
) -> Result<T, CoreError> {
    let FormatVersionProbe(format_version) = decode(bytes, what)?;
    if format_version != supported {
        return Err(format_unsupported(what, format_version, supported));
    }
    decode(bytes, what)
}
