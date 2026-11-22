//! Bound Serde diagnostics before an unexpected JSON string can be formatted.
//! The underlying JSON decoder still parses the wire. History shape enums
//! contain externally tagged newtype rows.

use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use std::fmt::{self, Write};

const MAX_ERROR_BYTES: usize = 512;
const LARGE_ERROR: &str = "History JSON shape is invalid";
const NEWTYPE_REQUIRED: &str = "History row variant requires a newtype value";

pub(super) fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = <T as serde::Deserialize>::deserialize(BoundedDeserializer(&mut decoder))?;
    decoder.end()?;
    Ok(value)
}

#[derive(Debug)]
struct BoundedError(String);

impl fmt::Display for BoundedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BoundedError {}

impl de::Error for BoundedError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        struct Limited(String);
        impl Write for Limited {
            fn write_str(&mut self, text: &str) -> fmt::Result {
                if text.len() > MAX_ERROR_BYTES - self.0.len() {
                    return Err(fmt::Error);
                }
                self.0.push_str(text);
                Ok(())
            }
        }

        let mut output = Limited(String::with_capacity(MAX_ERROR_BYTES));
        if write!(&mut output, "{message}").is_ok() {
            Self(output.0)
        } else {
            Self(LARGE_ERROR.to_owned())
        }
    }
}

struct BoundedDeserializer<D>(D);

impl<'de, D: Deserializer<'de>> Deserializer<'de> for BoundedDeserializer<D> {
    type Error = D::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.0.deserialize_any(BoundedVisitor(visitor))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.0.deserialize_option(BoundedVisitor(visitor))
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0
            .deserialize_newtype_struct(name, BoundedVisitor(visitor))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0
            .deserialize_enum(name, variants, BoundedVisitor(visitor))
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        self.0.deserialize_ignored_any(BoundedVisitor(visitor))
    }

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

struct BoundedVisitor<V>(V);

macro_rules! bounded_scalar {
    ($($method:ident($type:ty)),+ $(,)?) => {$ (
        fn $method<E: de::Error>(self, value: $type) -> Result<Self::Value, E> {
            self.0.$method::<BoundedError>(value).map_err(E::custom)
        }
    )+};
}

impl<'de, V: Visitor<'de>> Visitor<'de> for BoundedVisitor<V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.expecting(formatter)
    }

    bounded_scalar!(
        visit_bool(bool),
        visit_i8(i8),
        visit_i16(i16),
        visit_i32(i32),
        visit_i64(i64),
        visit_i128(i128),
        visit_u8(u8),
        visit_u16(u16),
        visit_u32(u32),
        visit_u64(u64),
        visit_u128(u128),
        visit_f32(f32),
        visit_f64(f64),
        visit_char(char),
        visit_str(&str),
        visit_borrowed_str(&'de str),
        visit_string(String),
        visit_bytes(&[u8]),
        visit_borrowed_bytes(&'de [u8]),
        visit_byte_buf(Vec<u8>),
    );

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.0.visit_none::<BoundedError>().map_err(E::custom)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.0.visit_unit::<BoundedError>().map_err(E::custom)
    }

    fn visit_some<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        self.0.visit_some(BoundedDeserializer(decoder))
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        decoder: D,
    ) -> Result<Self::Value, D::Error> {
        self.0.visit_newtype_struct(BoundedDeserializer(decoder))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
        self.0.visit_seq(BoundedSeq(access))
    }

    fn visit_map<A: MapAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
        self.0.visit_map(BoundedMap(access))
    }

    fn visit_enum<A: EnumAccess<'de>>(self, access: A) -> Result<Self::Value, A::Error> {
        self.0.visit_enum(BoundedEnum(access))
    }
}

struct BoundedSeed<S>(S);

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for BoundedSeed<S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        self.0.deserialize(BoundedDeserializer(decoder))
    }
}

struct BoundedSeq<A>(A);

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for BoundedSeq<A> {
    type Error = A::Error;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        self.0.next_element_seed(BoundedSeed(seed))
    }

    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}

struct BoundedMap<A>(A);

impl<'de, A: MapAccess<'de>> MapAccess<'de> for BoundedMap<A> {
    type Error = A::Error;

    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Self::Error> {
        self.0.next_key_seed(BoundedSeed(seed))
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        self.0.next_value_seed(BoundedSeed(seed))
    }

    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}

struct BoundedEnum<A>(A);

impl<'de, A: EnumAccess<'de>> EnumAccess<'de> for BoundedEnum<A> {
    type Error = A::Error;
    type Variant = BoundedVariant<A::Variant>;

    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), Self::Error> {
        let (value, variant) = self.0.variant_seed(BoundedSeed(seed))?;
        Ok((value, BoundedVariant(variant)))
    }
}

struct BoundedVariant<A>(A);

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for BoundedVariant<A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Err(de::Error::custom(NEWTYPE_REQUIRED))
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<S::Value, Self::Error> {
        self.0.newtype_variant_seed(BoundedSeed(seed))
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _length: usize,
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        Err(de::Error::custom(NEWTYPE_REQUIRED))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        Err(de::Error::custom(NEWTYPE_REQUIRED))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        entries: Vec<Entry>,
    }

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    enum Entry {
        Number(Option<u64>),
    }

    #[test]
    fn nested_field_variant_and_scalar_errors_are_bounded() -> Result<(), serde_json::Error> {
        assert_eq!(
            from_slice::<Probe>(br#"{"entries":[{"Number":7},{"Number":null}]}"#)?,
            Probe {
                entries: vec![Entry::Number(Some(7)), Entry::Number(None)],
            }
        );
        let token = "x".repeat(32 * 1024);
        for malformed in [
            format!(r#"{{"{token}":[]}}"#),
            format!(r#"{{"entries":[{{"{token}":null}}]}}"#),
            format!(r#"{{"entries":[{{"Number":"{token}"}}]}}"#),
        ] {
            let error = from_slice::<Probe>(malformed.as_bytes())
                .expect_err("malformed shape must be refused")
                .to_string();
            assert!(error.starts_with(LARGE_ERROR));
            assert!(error.contains("line 1 column"));
            assert!(error.len() < MAX_ERROR_BYTES);
        }
        Ok(())
    }
}
