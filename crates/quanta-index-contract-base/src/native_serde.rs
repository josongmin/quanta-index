//! Declarative helpers for closed native records and externally tagged enums.
//! These emit manual Serde implementations without proc-macro derives.

/// Decode a closed record key to its static schema name without allocating a
/// `String` for every known field. Unknown keys retain Serde's field error.
pub struct ClosedFieldKeySeedV1(pub &'static [&'static str]);

struct KeyVisitor {
    fields: &'static [&'static str],
    allow_identifier_bytes: bool,
}

impl serde::de::Visitor<'_> for KeyVisitor {
    type Value = &'static str;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a known record field")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.fields
            .iter()
            .copied()
            .find(|field| *field == value)
            .ok_or_else(|| E::unknown_field(value, self.fields))
    }

    fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        if self.allow_identifier_bytes {
            let name = std::str::from_utf8(value)
                .map_err(|_error| E::invalid_value(serde::de::Unexpected::Bytes(value), &self))?;
            self.visit_str(name)
        } else {
            Err(E::invalid_type(serde::de::Unexpected::Bytes(value), &self))
        }
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for KeyVisitor {
    type Value = &'static str;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_identifier(self)
    }
}

impl ClosedFieldKeySeedV1 {
    /// Preserve matching UTF-8 byte identifiers accepted by Serde-derived DTOs.
    #[must_use]
    pub fn serde_identifier(
        self,
    ) -> impl for<'de> serde::de::DeserializeSeed<'de, Value = &'static str> {
        KeyVisitor {
            fields: self.0,
            allow_identifier_bytes: true,
        }
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for ClosedFieldKeySeedV1 {
    type Value = &'static str;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_identifier(KeyVisitor {
            fields: self.0,
            allow_identifier_bytes: false,
        })
    }
}

/// Implement a closed record codec.
///
/// Fields are required by default, including nullable fields. An explicit
/// `= default` allows omission in a named map.
/// Positional records always require every field in declaration order.
/// `[serde_identifier]` preserves byte-string keys accepted by Serde derives.
#[macro_export]
macro_rules! native_serde_struct {
    ($( [$identifier:ident] )? $name:ident { $($field:ident : $ty:ty $(= $map_default:ident)?),+ $(,)? }) => {
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                let mut state = serializer.serialize_struct(stringify!($name), FIELDS.len())?;
                $(serde::ser::SerializeStruct::serialize_field(&mut state, stringify!($field), &self.$field)?;)+
                serde::ser::SerializeStruct::end(state)
            }
        }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                struct WireVisitor;
                impl<'de> serde::de::Visitor<'de> for WireVisitor {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!("a complete ", stringify!($name), " record"))
                    }
                    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                        $(let mut $field: Option<$ty> = None;)+
                        while let Some(key) = map.next_key_seed($crate::native_serde_struct!(@key_seed FIELDS $(, $identifier)?))? {
                            match key {
                                $(stringify!($field) => {
                                    if $field.is_some() {
                                        return Err(serde::de::Error::duplicate_field(stringify!($field)));
                                    }
                                    $field = Some(map.next_value::<$ty>()?);
                                },)+
                                _ => unreachable!("closed field seed only returns schema keys"),
                            }
                        }
                        Ok($name { $($field: $crate::native_serde_struct!(@map_value $field $(= $map_default)?),)+ })
                    }
                    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                        $(let $field = seq.next_element::<$ty>()?
                            .ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?;)+
                        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                            return Err(serde::de::Error::invalid_length(FIELDS.len().saturating_add(1), &self));
                        }
                        Ok($name { $($field,)+ })
                    }
                }
                deserializer.deserialize_struct(stringify!($name), FIELDS, WireVisitor)
            }
        }
    };
    (@map_value $field:ident) => {
        $field.ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?
    };
    (@map_value $field:ident = default) => {
        $field.unwrap_or_default()
    };
    (@key_seed $fields:ident) => {
        $crate::ClosedFieldKeySeedV1($fields)
    };
    (@key_seed $fields:ident, serde_identifier) => {
        $crate::ClosedFieldKeySeedV1($fields).serde_identifier()
    };
}

/// Preserve the Serde newtype representation while avoiding a proc macro.
#[macro_export]
macro_rules! native_serde_newtype {
    ($name:ident, [u8; $length:literal]) => {
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_newtype_struct(stringify!($name), &self.0)
            }
        }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct WireVisitor;
                impl<'de> serde::de::Visitor<'de> for WireVisitor {
                    type Value = $name;
                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str(concat!("a ", stringify!($name), " newtype"))
                    }
                    fn visit_newtype_struct<D: serde::Deserializer<'de>>(
                        self,
                        deserializer: D,
                    ) -> Result<Self::Value, D::Error> {
                        struct ArrayVisitor;
                        impl<'de> serde::de::Visitor<'de> for ArrayVisitor {
                            type Value = [u8; $length];
                            fn expecting(
                                &self,
                                formatter: &mut std::fmt::Formatter<'_>,
                            ) -> std::fmt::Result {
                                formatter.write_str(concat!(
                                    "exactly ",
                                    stringify!($length),
                                    " bytes"
                                ))
                            }
                            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                                self,
                                mut seq: A,
                            ) -> Result<Self::Value, A::Error> {
                                let mut bytes = [0_u8; $length];
                                for (index, byte) in bytes.iter_mut().enumerate() {
                                    *byte = seq.next_element::<u8>()?.ok_or_else(|| {
                                        serde::de::Error::invalid_length(index, &self)
                                    })?;
                                }
                                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                                    let expected: usize = $length;
                                    return Err(serde::de::Error::invalid_length(
                                        expected.saturating_add(1),
                                        &self,
                                    ));
                                }
                                Ok(bytes)
                            }
                        }
                        deserializer
                            .deserialize_tuple($length, ArrayVisitor)
                            .map($name)
                    }
                }
                deserializer.deserialize_newtype_struct(stringify!($name), WireVisitor)
            }
        }
    };
    ($name:ident, $inner:ty) => {
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_newtype_struct(stringify!($name), &self.0)
            }
        }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct WireVisitor;
                impl<'de> serde::de::Visitor<'de> for WireVisitor {
                    type Value = $name;
                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str(concat!("a ", stringify!($name), " newtype"))
                    }
                    fn visit_newtype_struct<D: serde::Deserializer<'de>>(
                        self,
                        deserializer: D,
                    ) -> Result<Self::Value, D::Error> {
                        <$inner as serde::Deserialize>::deserialize(deserializer).map($name)
                    }
                }
                deserializer.deserialize_newtype_struct(stringify!($name), WireVisitor)
            }
        }
    };
}

/// Closed externally tagged enum.
///
/// Explicit indices preserve the original declaration order independently of
/// the three variant-shape groups.
/// `[serde_identifier]` preserves byte-string variant and record names.
#[macro_export]
macro_rules! native_serde_enum {
    ($( [$identifier:ident] )? $name:ident {
        units { $($unit:ident = $unit_index:literal),* $(,)? }
        newtypes { $($newtype:ident = $newtype_index:literal => $inner:ty),* $(,)? }
        records { $($record:ident = $record_index:literal { $($field:ident : $ty:ty),+ $(,)? }),* $(,)? }
        $(custom_records { $($custom:ident = $custom_index:literal { $($custom_field:ident),+ $(,)? } => $codec:ident),* $(,)? })?
    }) => {
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self {
                    $(Self::$unit => serializer.serialize_unit_variant(stringify!($name), $unit_index, stringify!($unit)),)*
                    $(Self::$newtype(value) => serializer.serialize_newtype_variant(stringify!($name), $newtype_index, stringify!($newtype), value),)*
                    $($(Self::$custom { $($custom_field,)+ } => $codec::serialize_variant(
                        serializer, stringify!($name), $custom_index, stringify!($custom), $($custom_field),+
                    ),)*)?
                    $(Self::$record { $($field,)+ } => {
                        const FIELDS: &[&str] = &[$(stringify!($field)),+];
                        let mut state = serializer.serialize_struct_variant(stringify!($name), $record_index, stringify!($record), FIELDS.len())?;
                        $(serde::ser::SerializeStructVariant::serialize_field(&mut state, stringify!($field), $field)?;)+
                        serde::ser::SerializeStructVariant::end(state)
                    },)*
                }
            }
        }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                const VARIANTS: &[&str] = &[$(stringify!($unit),)* $(stringify!($newtype),)* $(stringify!($record),)* $($(stringify!($custom),)*)?];
                fn key_seed(fields: &'static [&'static str]) -> impl for<'wire> serde::de::DeserializeSeed<'wire, Value = &'static str> {
                    $crate::native_serde_struct!(@key_seed fields $(, $identifier)?)
                }
                struct WireVisitor;
                impl<'de> serde::de::Visitor<'de> for WireVisitor {
                    type Value = $name;
                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!("a closed ", stringify!($name), " variant"))
                    }
                    fn visit_enum<A: serde::de::EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
                        let (name, variant) = serde::de::EnumAccess::variant_seed(data, key_seed(VARIANTS))?;
                        match name {
                            $(stringify!($unit) => {
                                serde::de::VariantAccess::unit_variant(variant)?;
                                Ok($name::$unit)
                            },)*
                            $(stringify!($newtype) => serde::de::VariantAccess::newtype_variant::<$inner>(variant).map($name::$newtype),)*
                            $($(stringify!($custom) => {
                                let ($($custom_field,)+) = $codec::deserialize_variant(variant)?;
                                Ok($name::$custom { $($custom_field,)+ })
                            },)*)?
                            $(stringify!($record) => {
                                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                                struct RecordVisitor;
                                impl<'de> serde::de::Visitor<'de> for RecordVisitor {
                                    type Value = $name;
                                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                                        formatter.write_str(concat!("a complete ", stringify!($name), "::", stringify!($record), " record"))
                                    }
                                    fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                                        $(let mut $field: Option<$ty> = None;)+
                                        while let Some(key) = map.next_key_seed(key_seed(FIELDS))? {
                                            match key {
                                                $(stringify!($field) => {
                                                    if $field.is_some() {
                                                        return Err(serde::de::Error::duplicate_field(stringify!($field)));
                                                    }
                                                    $field = Some(map.next_value::<$ty>()?);
                                                },)+
                                                _ => unreachable!("closed field seed only returns schema keys"),
                                            }
                                        }
                                        Ok($name::$record { $($field: $field.ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?,)+ })
                                    }
                                    fn visit_seq<M: serde::de::SeqAccess<'de>>(self, mut seq: M) -> Result<Self::Value, M::Error> {
                                        $(let $field = seq.next_element::<$ty>()?
                                            .ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))?;)+
                                        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                                            return Err(serde::de::Error::invalid_length(FIELDS.len().saturating_add(1), &self));
                                        }
                                        Ok($name::$record { $($field,)+ })
                                    }
                                }
                                serde::de::VariantAccess::struct_variant(variant, FIELDS, RecordVisitor)
                            },)*
                            _ => unreachable!("closed variant seed only returns schema names"),
                        }
                    }
                }
                deserializer.deserialize_enum(stringify!($name), VARIANTS, WireVisitor)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    #[derive(Debug, Eq, PartialEq)]
    struct ClosedRecord {
        count: u64,
        optional: Option<u64>,
    }
    native_serde_struct!(ClosedRecord { count: u64, optional: Option<u64> });

    #[derive(Debug, Eq, PartialEq)]
    struct MapDefaultRecord {
        count: u64,
        optional: Option<u64>,
    }
    native_serde_struct!([serde_identifier] MapDefaultRecord {
        count: u64,
        optional: Option<u64> = default,
    });

    #[derive(Debug, Eq, PartialEq)]
    struct Bytes([u8; 2]);
    native_serde_newtype!(Bytes, [u8; 2]);

    #[derive(Debug, Eq, PartialEq)]
    enum Choice {
        Unit,
        Payload(Bytes),
        Record { count: u64, optional: Option<u64> },
    }
    native_serde_enum!(Choice {
        units { Unit = 0 }
        newtypes { Payload = 1 => Bytes }
        records { Record = 2 { count: u64, optional: Option<u64> } }
    });

    #[derive(Debug, Eq, PartialEq)]
    enum CompatibleChoice {
        Record { count: u64 },
    }
    native_serde_enum!([serde_identifier] CompatibleChoice {
        units {}
        newtypes {}
        records { Record = 0 { count: u64 } }
    });

    #[test]
    fn closed_record_preserves_named_wire_and_requires_explicit_nullable_field() {
        let record = ClosedRecord {
            count: 7,
            optional: None,
        };
        assert_eq!(
            serde_json::to_string(&record).expect("encode"),
            r#"{"count":7,"optional":null}"#
        );
        assert_eq!(
            serde_json::from_str::<ClosedRecord>(r#"{"optional":null,"count":7}"#).expect("decode"),
            record
        );
        for bad in [
            r#"{"count":7}"#,
            r#"{"count":7,"optional":null,"optional":null}"#,
            r#"{"count":7,"optional":null,"other":1}"#,
        ] {
            assert!(serde_json::from_str::<ClosedRecord>(bad).is_err());
        }
    }

    #[test]
    fn closed_record_sequence_refuses_missing_and_extra_slots() {
        assert_eq!(
            serde_json::from_str::<ClosedRecord>("[7,null]").expect("decode"),
            ClosedRecord {
                count: 7,
                optional: None
            }
        );
        for bad in ["[7]", "[7,null,0]"] {
            assert!(serde_json::from_str::<ClosedRecord>(bad).is_err());
        }
    }

    #[test]
    fn map_default_preserves_omission_without_accepting_incomplete_sequences() {
        let expected = MapDefaultRecord {
            count: 7,
            optional: None,
        };
        for input in [
            r#"{"count":7}"#,
            r#"{"count":7,"optional":null}"#,
            "[7,null]",
        ] {
            assert_eq!(
                serde_json::from_str::<MapDefaultRecord>(input).expect("decode"),
                expected
            );
        }
        assert_eq!(
            serde_json::to_string(&expected).expect("encode"),
            r#"{"count":7,"optional":null}"#,
        );
        for input in [
            "{}",
            "[7]",
            "[7,null,0]",
            r#"{"count":7,"optional":null,"optional":null}"#,
            r#"{"count":7,"extra":null}"#,
        ] {
            assert!(serde_json::from_str::<MapDefaultRecord>(input).is_err());
        }
        let cbor = b"\xa1\x65count\x07";
        assert_eq!(
            ciborium::from_reader::<MapDefaultRecord, _>(&cbor[..]).expect("decode named CBOR"),
            expected
        );
    }

    #[test]
    fn serde_identifiers_accept_known_byte_names_without_loosening_strict_records() {
        let record = b"\xa2\x45count\x07\x48optional\xf6";
        assert_eq!(
            ciborium::from_reader::<MapDefaultRecord, _>(&record[..]).expect("decode byte keys"),
            MapDefaultRecord {
                count: 7,
                optional: None
            },
        );
        assert!(ciborium::from_reader::<ClosedRecord, _>(&record[..]).is_err());
        let duplicate = b"\xa2\x45count\x07\x65count\x08";
        assert!(ciborium::from_reader::<MapDefaultRecord, _>(&duplicate[..]).is_err());
        let variant = b"\xa1\x46Record\xa1\x45count\x07";
        assert_eq!(
            ciborium::from_reader::<CompatibleChoice, _>(&variant[..]).expect("decode byte names"),
            CompatibleChoice::Record { count: 7 },
        );
    }

    #[test]
    fn fixed_array_newtype_preserves_wire_and_checks_length() {
        assert_eq!(
            serde_json::to_string(&Bytes([1, 2])).expect("encode"),
            "[1,2]"
        );
        assert_eq!(
            serde_json::from_str::<Bytes>("[1,2]").expect("decode"),
            Bytes([1, 2])
        );
        for bad in ["[1]", "[1,2,3]", "[1,256]"] {
            assert!(serde_json::from_str::<Bytes>(bad).is_err());
        }
    }

    #[test]
    fn enum_variant_shapes_match_fixed_wire_goldens() {
        for (value, wire) in [
            (Choice::Unit, r#""Unit""#),
            (Choice::Payload(Bytes([1, 2])), r#"{"Payload":[1,2]}"#),
            (
                Choice::Record {
                    count: 7,
                    optional: None,
                },
                r#"{"Record":{"count":7,"optional":null}}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&value).expect("encode"), wire);
            assert_eq!(serde_json::from_str::<Choice>(wire).expect("decode"), value);
        }
    }

    #[test]
    fn enum_record_refuses_unknown_duplicate_missing_and_extra_fields() {
        for bad in [
            r#"{"Record":{"count":7}}"#,
            r#"{"Record":{"count":7,"optional":null,"other":1}}"#,
            r#"{"Record":{"count":7,"optional":null,"optional":null}}"#,
            r#"{"Record":[7,null,0]}"#,
            r#""Unknown""#,
        ] {
            assert!(serde_json::from_str::<Choice>(bad).is_err());
        }
    }

    #[test]
    fn cbor_shapes_match_independent_fixed_wire_goldens() {
        // Definite maps and arrays, UTF-8 field/variant names, and explicit null.
        const RECORD: &[u8] = &[
            0xa2, 0x65, 0x63, 0x6f, 0x75, 0x6e, 0x74, 0x07, 0x68, 0x6f, 0x70, 0x74, 0x69, 0x6f,
            0x6e, 0x61, 0x6c, 0xf6,
        ];
        const ARRAY_NEWTYPE: &[u8] = &[0x82, 0x01, 0x02];
        const ENUM_UNIT: &[u8] = &[0x64, 0x55, 0x6e, 0x69, 0x74];
        const ENUM_NEWTYPE: &[u8] = &[
            0xa1, 0x67, 0x50, 0x61, 0x79, 0x6c, 0x6f, 0x61, 0x64, 0x82, 0x01, 0x02,
        ];
        const ENUM_RECORD: &[u8] = &[
            0xa1, 0x66, 0x52, 0x65, 0x63, 0x6f, 0x72, 0x64, 0xa2, 0x65, 0x63, 0x6f, 0x75, 0x6e,
            0x74, 0x07, 0x68, 0x6f, 0x70, 0x74, 0x69, 0x6f, 0x6e, 0x61, 0x6c, 0xf6,
        ];

        let record = ClosedRecord {
            count: 7,
            optional: None,
        };
        let mut encoded = Vec::new();
        ciborium::into_writer(&record, &mut encoded).expect("encode record");
        assert_eq!(encoded, RECORD);
        assert_eq!(
            ciborium::from_reader::<ClosedRecord, _>(RECORD).expect("decode fixed record"),
            record
        );

        let bytes = Bytes([1, 2]);
        encoded.clear();
        ciborium::into_writer(&bytes, &mut encoded).expect("encode fixed-array newtype");
        assert_eq!(encoded, ARRAY_NEWTYPE);
        assert_eq!(
            ciborium::from_reader::<Bytes, _>(ARRAY_NEWTYPE).expect("decode fixed-array newtype"),
            bytes
        );

        for (value, golden) in [
            (Choice::Unit, ENUM_UNIT),
            (Choice::Payload(Bytes([1, 2])), ENUM_NEWTYPE),
            (
                Choice::Record {
                    count: 7,
                    optional: None,
                },
                ENUM_RECORD,
            ),
        ] {
            encoded.clear();
            ciborium::into_writer(&value, &mut encoded).expect("encode enum variant");
            assert_eq!(encoded, golden);
            assert_eq!(
                ciborium::from_reader::<Choice, _>(golden).expect("decode fixed enum variant"),
                value
            );
        }
    }

    #[test]
    fn cbor_closed_records_refuse_missing_duplicate_and_unknown_fields() {
        // These malformed payloads are literal CBOR, not generated by the codec under test.
        const MISSING: &[u8] = &[0xa1, 0x65, 0x63, 0x6f, 0x75, 0x6e, 0x74, 0x07];
        const DUPLICATE: &[u8] = &[
            0xa3, 0x65, 0x63, 0x6f, 0x75, 0x6e, 0x74, 0x07, 0x68, 0x6f, 0x70, 0x74, 0x69, 0x6f,
            0x6e, 0x61, 0x6c, 0xf6, 0x68, 0x6f, 0x70, 0x74, 0x69, 0x6f, 0x6e, 0x61, 0x6c, 0xf6,
        ];
        const UNKNOWN: &[u8] = &[
            0xa3, 0x65, 0x63, 0x6f, 0x75, 0x6e, 0x74, 0x07, 0x68, 0x6f, 0x70, 0x74, 0x69, 0x6f,
            0x6e, 0x61, 0x6c, 0xf6, 0x65, 0x65, 0x78, 0x74, 0x72, 0x61, 0x01,
        ];
        const ENUM_MISSING: &[u8] = &[
            0xa1, 0x66, 0x52, 0x65, 0x63, 0x6f, 0x72, 0x64, 0xa1, 0x65, 0x63, 0x6f, 0x75, 0x6e,
            0x74, 0x07,
        ];
        const ENUM_DUPLICATE: &[u8] = &[
            0xa1, 0x66, 0x52, 0x65, 0x63, 0x6f, 0x72, 0x64, 0xa3, 0x65, 0x63, 0x6f, 0x75, 0x6e,
            0x74, 0x07, 0x68, 0x6f, 0x70, 0x74, 0x69, 0x6f, 0x6e, 0x61, 0x6c, 0xf6, 0x68, 0x6f,
            0x70, 0x74, 0x69, 0x6f, 0x6e, 0x61, 0x6c, 0xf6,
        ];
        const ENUM_UNKNOWN: &[u8] = &[
            0xa1, 0x66, 0x52, 0x65, 0x63, 0x6f, 0x72, 0x64, 0xa3, 0x65, 0x63, 0x6f, 0x75, 0x6e,
            0x74, 0x07, 0x68, 0x6f, 0x70, 0x74, 0x69, 0x6f, 0x6e, 0x61, 0x6c, 0xf6, 0x65, 0x65,
            0x78, 0x74, 0x72, 0x61, 0x01,
        ];
        for bad in [MISSING, DUPLICATE, UNKNOWN] {
            assert!(ciborium::from_reader::<ClosedRecord, _>(bad).is_err());
        }
        for bad in [ENUM_MISSING, ENUM_DUPLICATE, ENUM_UNKNOWN] {
            assert!(ciborium::from_reader::<Choice, _>(bad).is_err());
        }
        for bad in [&[0x81, 0x01][..], &[0x83, 0x01, 0x02, 0x03][..]] {
            assert!(ciborium::from_reader::<Bytes, _>(bad).is_err());
        }
    }
}
