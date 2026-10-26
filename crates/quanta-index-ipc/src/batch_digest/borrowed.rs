//! Root-only digest substitution for the existing batch serializer.

use serde::ser::{self, Impossible, SerializeStruct};
use serde::{Serialize, Serializer};

pub(super) struct ClearedRootDigest<'a, B: ?Sized>(pub(super) &'a B);

impl<B: Serialize + ?Sized> Serialize for ClearedRootDigest<'_, B> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(RootSerializer(serializer))
    }
}

struct RootSerializer<S>(S);

struct RootStruct<T> {
    inner: T,
    seen_digest: bool,
}

impl<T: SerializeStruct> SerializeStruct for RootStruct<T> {
    type Ok = T::Ok;
    type Error = T::Error;

    fn serialize_field<V: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &V,
    ) -> Result<(), Self::Error> {
        if key == "batch_digest" {
            if self.seen_digest {
                return Err(ser::Error::custom("duplicate root batch_digest field"));
            }
            self.seen_digest = true;
            self.inner.serialize_field(key, "")
        } else {
            self.inner.serialize_field(key, value)
        }
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        if !self.seen_digest {
            return Err(ser::Error::custom("missing root batch_digest field"));
        }
        self.inner.end()
    }
}

fn unsupported<T, E: ser::Error>() -> Result<T, E> {
    Err(E::custom("batch digest requires a root struct"))
}

impl<S: Serializer> Serializer for RootSerializer<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Impossible<Self::Ok, Self::Error>;
    type SerializeTuple = Impossible<Self::Ok, Self::Error>;
    type SerializeTupleStruct = Impossible<Self::Ok, Self::Error>;
    type SerializeTupleVariant = Impossible<Self::Ok, Self::Error>;
    type SerializeMap = Impossible<Self::Ok, Self::Error>;
    type SerializeStruct = RootStruct<S::SerializeStruct>;
    type SerializeStructVariant = Impossible<Self::Ok, Self::Error>;

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_char(self, _value: char) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_str(self, _value: &str) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_some<V: Serialize + ?Sized>(self, _value: &V) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_newtype_struct<V: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _value: &V,
    ) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_newtype_variant<V: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &V,
    ) -> Result<Self::Ok, Self::Error> {
        unsupported()
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        unsupported()
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        unsupported()
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        unsupported()
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        unsupported()
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        unsupported()
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(RootStruct {
            inner: self.0.serialize_struct(name, len)?,
            seen_digest: false,
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        unsupported()
    }
    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}
