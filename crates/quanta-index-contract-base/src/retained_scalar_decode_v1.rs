//! Retain rejected owned wire backing while invoking the existing scalar
//! Deserialize implementation. No scalar predicate or value is synthesized.
use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};

pub(crate) struct ScalarDataV1<T> {
    pub(crate) output: Option<T>,
    pub(crate) refused_string: Option<String>,
    pub(crate) refused_bytes: Option<Vec<u8>>,
}
impl<T> ScalarDataV1<T> {
    pub(crate) const fn new_v1() -> Self {
        Self {
            output: None,
            refused_string: None,
            refused_bytes: None,
        }
    }
}
pub(crate) fn decode_scalar_into_v1<'de, D: Deserializer<'de>, T: Deserialize<'de> + Copy>(
    d: D,
    data: &mut ScalarDataV1<T>,
    dynamic: bool,
) -> Result<(), D::Error> {
    data.output = Some(T::deserialize(RetainedDeserializerV1 {
        inner: d,
        dynamic,
        string: &mut data.refused_string,
        bytes: &mut data.refused_bytes,
    })?);
    Ok(())
}
struct RetainedDeserializerV1<'a, D> {
    inner: D,
    dynamic: bool,
    string: &'a mut Option<String>,
    bytes: &'a mut Option<Vec<u8>>,
}
struct RetainedVisitorV1<'a, V> {
    inner: V,
    dynamic: bool,
    string: &'a mut Option<String>,
    bytes: &'a mut Option<Vec<u8>>,
}
macro_rules! forward_deserializer_v1 {
    ($($method:ident $(($($arg:ident : $type:ty),+))?),+ $(,)?) => {$(
        fn $method<V: Visitor<'de>>(self, $($($arg: $type,)*)? visitor: V) -> Result<V::Value, Self::Error> {
            let visitor = RetainedVisitorV1 { inner: visitor, string: self.string, bytes: self.bytes, dynamic: self.dynamic };
            if self.dynamic { self.inner.deserialize_any(visitor) }
            else { self.inner.$method($($($arg,)*)? visitor) }
        }
    )+};
}
impl<'de, D: Deserializer<'de>> Deserializer<'de> for RetainedDeserializerV1<'_, D> {
    type Error = D::Error;
    forward_deserializer_v1!(deserialize_any, deserialize_bool, deserialize_i8, deserialize_i16, deserialize_i32, deserialize_i64, deserialize_i128, deserialize_u8, deserialize_u16, deserialize_u32, deserialize_u64, deserialize_u128, deserialize_f32, deserialize_f64, deserialize_char, deserialize_str, deserialize_string, deserialize_bytes, deserialize_byte_buf, deserialize_option, deserialize_unit, deserialize_unit_struct(name: &'static str), deserialize_newtype_struct(name: &'static str), deserialize_seq, deserialize_tuple(len: usize), deserialize_tuple_struct(name: &'static str, len: usize), deserialize_map, deserialize_struct(name: &'static str, fields: &'static [&'static str]), deserialize_enum(name: &'static str, variants: &'static [&'static str]), deserialize_identifier, deserialize_ignored_any);
    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}
macro_rules! forward_visitor_v1 {
    ($($method:ident($type:ty)),+ $(,)?) => {$(
        fn $method<E: de::Error>(self, value: $type) -> Result<Self::Value, E> { self.inner.$method(value) }
    )+};
}
impl<'de, V: Visitor<'de>> Visitor<'de> for RetainedVisitorV1<'_, V> {
    type Value = V::Value;
    fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.inner.expecting(f)
    }
    forward_visitor_v1!(
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
        visit_bytes(&[u8]),
        visit_borrowed_bytes(&'de [u8])
    );
    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        *self.string = Some(value);
        self.inner.visit_str(self.string.as_deref().unwrap_or(""))
    }
    fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        *self.bytes = Some(value);
        self.inner.visit_bytes(self.bytes.as_deref().unwrap_or(&[]))
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_none()
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_unit()
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.inner.visit_some(RetainedDeserializerV1 {
            inner: d,
            dynamic: self.dynamic,
            string: self.string,
            bytes: self.bytes,
        })
    }
    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.inner.visit_newtype_struct(RetainedDeserializerV1 {
            inner: d,
            dynamic: self.dynamic,
            string: self.string,
            bytes: self.bytes,
        })
    }
    fn visit_seq<A: SeqAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_seq(RetainedAccessV1 {
            inner: a,
            dynamic: self.dynamic,
            string: self.string,
            bytes: self.bytes,
        })
    }
    fn visit_map<A: MapAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_map(RetainedAccessV1 {
            inner: a,
            dynamic: self.dynamic,
            string: self.string,
            bytes: self.bytes,
        })
    }
    fn visit_enum<A: de::EnumAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_enum(a)
    }
}
struct RetainedSeedV1<'a, S> {
    inner: S,
    dynamic: bool,
    string: &'a mut Option<String>,
    bytes: &'a mut Option<Vec<u8>>,
}
impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for RetainedSeedV1<'_, S> {
    type Value = S::Value;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.inner.deserialize(RetainedDeserializerV1 {
            inner: d,
            dynamic: self.dynamic,
            string: self.string,
            bytes: self.bytes,
        })
    }
}
struct RetainedAccessV1<'a, A> {
    inner: A,
    dynamic: bool,
    string: &'a mut Option<String>,
    bytes: &'a mut Option<Vec<u8>>,
}
impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for RetainedAccessV1<'_, A> {
    type Error = A::Error;
    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, A::Error> {
        self.inner.next_element_seed(RetainedSeedV1 {
            inner: seed,
            dynamic: self.dynamic,
            string: &mut *self.string,
            bytes: &mut *self.bytes,
        })
    }
    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}
impl<'de, A: MapAccess<'de>> MapAccess<'de> for RetainedAccessV1<'_, A> {
    type Error = A::Error;
    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, A::Error> {
        self.inner.next_key_seed(RetainedSeedV1 {
            inner: seed,
            dynamic: self.dynamic,
            string: &mut *self.string,
            bytes: &mut *self.bytes,
        })
    }
    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, A::Error> {
        self.inner.next_value_seed(RetainedSeedV1 {
            inner: seed,
            dynamic: self.dynamic,
            string: &mut *self.string,
            bytes: &mut *self.bytes,
        })
    }
    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}
