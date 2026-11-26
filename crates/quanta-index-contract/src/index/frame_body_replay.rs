//! Capture a body before its adjacent tag and replay its typed Serde shape.

use serde::{
    Deserialize, Deserializer,
    de::{
        EnumAccess, Error, IntoDeserializer, MapAccess, SeqAccess, Visitor,
        value::{CowStrDeserializer, MapAccessDeserializer, MapDeserializer, SeqDeserializer},
    },
};

// The normal exporter writes kind first and decodes the typed body directly.
// When body comes first, retain its scalar types and map entries until the
// tag is available. In particular, a CBOR bignum must not become a small u64.
pub(crate) enum BodyReplay<'de> {
    Unit,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    Text(std::borrow::Cow<'de, str>),
    Bytes(std::borrow::Cow<'de, [u8]>),
    None,
    Some(Box<Self>),
    Newtype(Box<Self>),
    Sequence(Vec<Self>),
    Map(Vec<(Self, Self)>),
}

#[cfg(test)]
std::thread_local! {
    pub(crate) static BODY_REPLAY_ENTRY_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl<'de> Deserialize<'de> for BodyReplay<'de> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        #[cfg(test)]
        BODY_REPLAY_ENTRY_COUNT.with(|count| count.set(count.get() + 1));
        struct Capture;

        macro_rules! capture_scalar {
            ($($visit:ident($ty:ty) => $variant:ident),+ $(,)?) => {
                $(fn $visit<E: Error>(self, value: $ty) -> Result<Self::Value, E> {
                    Ok(BodyReplay::$variant(value))
                })+
            };
        }

        impl<'de> Visitor<'de> for Capture {
            type Value = BodyReplay<'de>;

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("any value")
            }

            capture_scalar!(
                visit_bool(bool) => Bool,
                visit_i64(i64) => I64,
                visit_u64(u64) => U64,
                visit_f64(f64) => F64,
            );

            fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
                Ok(BodyReplay::Unit)
            }

            fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
                Ok(BodyReplay::None)
            }

            fn visit_some<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
                BodyReplay::deserialize(decoder).map(|value| BodyReplay::Some(Box::new(value)))
            }

            fn visit_newtype_struct<D: Deserializer<'de>>(
                self,
                decoder: D,
            ) -> Result<Self::Value, D::Error> {
                BodyReplay::deserialize(decoder).map(|value| BodyReplay::Newtype(Box::new(value)))
            }

            fn visit_borrowed_str<E: Error>(self, value: &'de str) -> Result<Self::Value, E> {
                Ok(BodyReplay::Text(std::borrow::Cow::Borrowed(value)))
            }

            fn visit_str<E: Error>(self, value: &str) -> Result<Self::Value, E> {
                self.visit_string(value.to_owned())
            }

            fn visit_string<E: Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(BodyReplay::Text(std::borrow::Cow::Owned(value)))
            }

            fn visit_borrowed_bytes<E: Error>(self, value: &'de [u8]) -> Result<Self::Value, E> {
                Ok(BodyReplay::Bytes(std::borrow::Cow::Borrowed(value)))
            }

            fn visit_bytes<E: Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                self.visit_byte_buf(value.to_vec())
            }

            fn visit_byte_buf<E: Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
                Ok(BodyReplay::Bytes(std::borrow::Cow::Owned(value)))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element()? {
                    values.push(value);
                }
                Ok(BodyReplay::Sequence(values))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry()? {
                    pairs.push(pair);
                }
                Ok(BodyReplay::Map(pairs))
            }

            fn visit_enum<A: EnumAccess<'de>>(self, _: A) -> Result<Self::Value, A::Error> {
                Err(Error::custom(
                    "untagged and internally tagged enums do not support enum input",
                ))
            }
        }

        decoder.deserialize_any(Capture)
    }
}

impl<'de> IntoDeserializer<'de, serde::de::value::Error> for BodyReplay<'de> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

impl<'de> Deserializer<'de> for BodyReplay<'de> {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        use std::borrow::Cow;
        match self {
            Self::Unit => visitor.visit_unit(),
            Self::Bool(value) => visitor.visit_bool(value),
            Self::I64(value) => visitor.visit_i64(value),
            Self::U64(value) => visitor.visit_u64(value),
            Self::F64(value) => visitor.visit_f64(value),
            Self::Text(Cow::Borrowed(value)) => visitor.visit_borrowed_str(value),
            Self::Text(Cow::Owned(value)) => visitor.visit_string(value),
            Self::Bytes(Cow::Borrowed(value)) => visitor.visit_borrowed_bytes(value),
            Self::Bytes(Cow::Owned(value)) => visitor.visit_byte_buf(value),
            Self::None => visitor.visit_none(),
            Self::Some(value) => visitor.visit_some(*value),
            Self::Newtype(value) => visitor.visit_newtype_struct(*value),
            Self::Sequence(items) => {
                SeqDeserializer::new(items.into_iter()).deserialize_any(visitor)
            }
            Self::Map(entries) => {
                MapDeserializer::new(entries.into_iter()).deserialize_any(visitor)
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Unit => visitor.visit_unit(),
            Self::None => visitor.visit_none(),
            Self::Some(value) => visitor.visit_some(*value),
            value => visitor.visit_some(value),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Self::Newtype(value) => visitor.visit_newtype_struct(*value),
            value => visitor.visit_newtype_struct(value),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Self::Text(tag) => visitor.visit_enum(CowStrDeserializer::new(tag)),
            Self::Map(entries) if entries.len() == 1 => {
                MapAccessDeserializer::new(MapDeserializer::new(entries.into_iter()))
                    .deserialize_enum(name, variants, visitor)
            }
            _ => Err(Error::custom("expected a tag or singleton map")),
        }
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf
        unit unit_struct seq tuple tuple_struct map struct identifier
    }
}
