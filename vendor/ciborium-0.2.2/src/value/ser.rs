// SPDX-License-Identifier: Apache-2.0

use super::{Error, Value};

use alloc::{vec, vec::Vec};

use ::serde::ser::{self, SerializeMap as _, SerializeSeq as _, SerializeTupleVariant as _};

// Return a narrower representation only when its widened bits are identical.
// This is the same decision made by narrowing and widening an f64, without
// rounding a value that cannot be serialized losslessly as f32.
fn exact_f32(value: f64) -> Option<f32> {
    let bits = value.to_bits();
    let sign = if bits & 0x8000_0000_0000_0000 == 0 {
        0
    } else {
        0x8000_0000
    };
    let Ok(exponent) = u16::try_from(bits.checked_shr(52)? & 0x7ff) else {
        return None;
    };
    let fraction = bits & 0x000f_ffff_ffff_ffff;

    let magnitude = match exponent {
        0 if fraction == 0 => 0,
        874..=896 => {
            let shift = u32::from(926_u16.checked_sub(exponent)?);
            let significand = 0x0010_0000_0000_0000 | fraction;
            let discarded = 1_u64.checked_shl(shift)?.checked_sub(1)?;
            if significand & discarded != 0 {
                return None;
            }
            match u32::try_from(significand.checked_shr(shift)?) {
                Ok(value) => value,
                Err(..) => return None,
            }
        }
        897..=1150 => {
            if fraction & 0x1fff_ffff != 0 {
                return None;
            }
            let narrow_exponent = u32::from(exponent.checked_sub(896)?).checked_shl(23)?;
            let Ok(narrow_fraction) = u32::try_from(fraction.checked_shr(29)?) else {
                return None;
            };
            narrow_exponent | narrow_fraction
        }
        2047 => match u32::try_from(fraction.checked_shr(29)?) {
            Ok(value) => 0x7f80_0000 | value,
            Err(..) => return None,
        },
        _ => return None,
    };

    let candidate = f32::from_bits(sign | magnitude);
    (f64::from(candidate).to_bits() == bits).then_some(candidate)
}

impl ser::Serialize for Value {
    #[inline]
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Bytes(x) => serializer.serialize_bytes(x),
            Value::Bool(x) => serializer.serialize_bool(*x),
            Value::Text(x) => serializer.serialize_str(x),
            Value::Null => serializer.serialize_unit(),

            Value::Tag(t, v) => {
                let mut acc = serializer.serialize_tuple_variant("@@TAG@@", 0, "@@TAGGED@@", 2)?;
                acc.serialize_field(t)?;
                acc.serialize_field(v)?;
                acc.end()
            }

            Value::Float(x) => {
                if let Some(y) = exact_f32(*x) {
                    serializer.serialize_f32(y)
                } else {
                    serializer.serialize_f64(*x)
                }
            }

            Value::Integer(x) => {
                if let Ok(x) = u8::try_from(*x) {
                    serializer.serialize_u8(x)
                } else if let Ok(x) = i8::try_from(*x) {
                    serializer.serialize_i8(x)
                } else if let Ok(x) = u16::try_from(*x) {
                    serializer.serialize_u16(x)
                } else if let Ok(x) = i16::try_from(*x) {
                    serializer.serialize_i16(x)
                } else if let Ok(x) = u32::try_from(*x) {
                    serializer.serialize_u32(x)
                } else if let Ok(x) = i32::try_from(*x) {
                    serializer.serialize_i32(x)
                } else if let Ok(x) = u64::try_from(*x) {
                    serializer.serialize_u64(x)
                } else if let Ok(x) = i64::try_from(*x) {
                    serializer.serialize_i64(x)
                } else if let Ok(x) = u128::try_from(*x) {
                    serializer.serialize_u128(x)
                } else {
                    serializer.serialize_i128(i128::from(*x))
                }
            }

            Value::Array(x) => {
                let mut map = serializer.serialize_seq(Some(x.len()))?;

                for v in x {
                    map.serialize_element(v)?;
                }

                map.end()
            }

            Value::Map(x) => {
                let mut map = serializer.serialize_map(Some(x.len()))?;

                for (k, v) in x {
                    map.serialize_entry(k, v)?;
                }

                map.end()
            }
        }
    }
}

macro_rules! mkserialize {
    ($($f:ident($v:ty)),+ $(,)?) => {
        $(
            #[inline]
            fn $f(self, v: $v) -> Result<Self::Ok, Self::Error> {
                Ok(v.into())
            }
        )+
    };
}

struct Tagged {
    tag: Option<u64>,
    val: Option<Value>,
}

struct Named<T> {
    name: &'static str,
    data: T,
    tag: Option<Tagged>,
}

struct Map {
    data: Vec<(Value, Value)>,
    temp: Option<Value>,
}

struct Serializer<T>(T);

impl ser::Serializer for Serializer<()> {
    type Ok = Value;
    type Error = Error;

    type SerializeSeq = Serializer<Vec<Value>>;
    type SerializeTuple = Serializer<Vec<Value>>;
    type SerializeTupleStruct = Serializer<Vec<Value>>;
    type SerializeTupleVariant = Serializer<Named<Vec<Value>>>;
    type SerializeMap = Serializer<Map>;
    type SerializeStruct = Serializer<Vec<(Value, Value)>>;
    type SerializeStructVariant = Serializer<Named<Vec<(Value, Value)>>>;

    mkserialize! {
        serialize_bool(bool),

        serialize_f32(f32),
        serialize_f64(f64),

        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),

        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8]),
    }

    #[inline]
    fn serialize_none(self) -> Result<Value, Error> {
        Ok(Value::Null)
    }

    #[inline]
    fn serialize_some<U: ?Sized + ser::Serialize>(self, value: &U) -> Result<Value, Error> {
        value.serialize(self)
    }

    #[inline]
    fn serialize_unit(self) -> Result<Value, Error> {
        self.serialize_none()
    }

    #[inline]
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, Error> {
        self.serialize_unit()
    }

    #[inline]
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Value, Error> {
        Ok(variant.into())
    }

    #[inline]
    fn serialize_newtype_struct<U: ?Sized + ser::Serialize>(
        self,
        _name: &'static str,
        value: &U,
    ) -> Result<Value, Error> {
        value.serialize(self)
    }

    #[inline]
    fn serialize_newtype_variant<U: ?Sized + ser::Serialize>(
        self,
        name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &U,
    ) -> Result<Value, Error> {
        Ok(match (name, variant) {
            ("@@TAG@@", "@@UNTAGGED@@") => Value::serialized(value)?,
            _ => vec![(variant.into(), Value::serialized(value)?)].into(),
        })
    }

    #[inline]
    fn serialize_seq(self, length: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        Ok(Serializer(Vec::with_capacity(length.unwrap_or(0))))
    }

    #[inline]
    fn serialize_tuple(self, length: usize) -> Result<Self::SerializeTuple, Error> {
        self.serialize_seq(Some(length))
    }

    #[inline]
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleStruct, Error> {
        self.serialize_seq(Some(length))
    }

    #[inline]
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        _index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeTupleVariant, Error> {
        Ok(Serializer(Named {
            name: variant,
            data: Vec::with_capacity(length),
            tag: match (name, variant) {
                ("@@TAG@@", "@@TAGGED@@") => Some(Tagged {
                    tag: None,
                    val: None,
                }),

                _ => None,
            },
        }))
    }

    #[inline]
    fn serialize_map(self, length: Option<usize>) -> Result<Self::SerializeMap, Error> {
        Ok(Serializer(Map {
            data: Vec::with_capacity(length.unwrap_or(0)),
            temp: None,
        }))
    }

    #[inline]
    fn serialize_struct(
        self,
        _name: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStruct, Error> {
        Ok(Serializer(Vec::with_capacity(length)))
    }

    #[inline]
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        length: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        Ok(Serializer(Named {
            name: variant,
            data: Vec::with_capacity(length),
            tag: None,
        }))
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

impl ser::SerializeSeq for Serializer<Vec<Value>> {
    type Ok = Value;
    type Error = Error;

    #[inline]
    fn serialize_element<U: ?Sized + ser::Serialize>(&mut self, value: &U) -> Result<(), Error> {
        self.0.push(Value::serialized(&value)?);
        Ok(())
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.0.into())
    }
}

impl ser::SerializeTuple for Serializer<Vec<Value>> {
    type Ok = Value;
    type Error = Error;

    #[inline]
    fn serialize_element<U: ?Sized + ser::Serialize>(&mut self, value: &U) -> Result<(), Error> {
        self.0.push(Value::serialized(&value)?);
        Ok(())
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.0.into())
    }
}

impl ser::SerializeTupleStruct for Serializer<Vec<Value>> {
    type Ok = Value;
    type Error = Error;

    #[inline]
    fn serialize_field<U: ?Sized + ser::Serialize>(&mut self, value: &U) -> Result<(), Error> {
        self.0.push(Value::serialized(&value)?);
        Ok(())
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.0.into())
    }
}

impl ser::SerializeTupleVariant for Serializer<Named<Vec<Value>>> {
    type Ok = Value;
    type Error = Error;

    #[inline]
    fn serialize_field<U: ?Sized + ser::Serialize>(&mut self, value: &U) -> Result<(), Error> {
        match self.0.tag.as_mut() {
            Some(tag) => match tag.tag {
                None => match value.serialize(crate::tag::Serializer) {
                    Ok(t) => tag.tag = Some(t),
                    Err(..) => return Err(ser::Error::custom("expected tag")),
                },

                Some(..) => tag.val = Some(Value::serialized(value)?),
            },

            None => self.0.data.push(Value::serialized(value)?),
        }

        Ok(())
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(match self.0.tag {
            Some(tag) => match tag {
                Tagged {
                    tag: Some(t),
                    val: Some(v),
                } => Value::Tag(t, v.into()),
                _ => return Err(ser::Error::custom("invalid tag input")),
            },

            None => vec![(self.0.name.into(), self.0.data.into())].into(),
        })
    }
}

impl ser::SerializeMap for Serializer<Map> {
    type Ok = Value;
    type Error = Error;

    #[inline]
    fn serialize_key<U: ?Sized + ser::Serialize>(&mut self, key: &U) -> Result<(), Error> {
        self.0.temp = Some(Value::serialized(key)?);
        Ok(())
    }

    #[inline]
    fn serialize_value<U: ?Sized + ser::Serialize>(&mut self, value: &U) -> Result<(), Error> {
        let key = self
            .0
            .temp
            .take()
            .ok_or_else(|| ser::Error::custom("expected map key"))?;
        let val = Value::serialized(&value)?;

        self.0.data.push((key, val));
        Ok(())
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.0.data.into())
    }
}

impl ser::SerializeStruct for Serializer<Vec<(Value, Value)>> {
    type Ok = Value;
    type Error = Error;

    #[inline]
    fn serialize_field<U: ?Sized + ser::Serialize>(
        &mut self,
        key: &'static str,
        value: &U,
    ) -> Result<(), Error> {
        let k = Value::serialized(&key)?;
        let v = Value::serialized(&value)?;
        self.0.push((k, v));
        Ok(())
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.0.into())
    }
}

impl ser::SerializeStructVariant for Serializer<Named<Vec<(Value, Value)>>> {
    type Ok = Value;
    type Error = Error;

    #[inline]
    fn serialize_field<U: ?Sized + ser::Serialize>(
        &mut self,
        key: &'static str,
        value: &U,
    ) -> Result<(), Self::Error> {
        let k = Value::serialized(&key)?;
        let v = Value::serialized(&value)?;
        self.0.data.push((k, v));
        Ok(())
    }

    #[inline]
    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(vec![(self.0.name.into(), self.0.data.into())].into())
    }
}

impl Value {
    /// Serializes an object into a `Value`
    #[inline]
    pub fn serialized<T: ?Sized + ser::Serialize>(value: &T) -> Result<Self, Error> {
        value.serialize(Serializer(()))
    }
}

#[cfg(test)]
mod tests {
    use super::exact_f32;

    #[test]
    fn exact_f32_ieee_boundaries() {
        // Inputs are independent IEEE-754 f64 bit patterns; expected results
        // are f32 bit patterns, including the sign of zero and NaN payloads.
        let cases: &[(u64, Option<u32>)] = &[
            (0x0000_0000_0000_0000, Some(0x0000_0000)),
            (0x8000_0000_0000_0000, Some(0x8000_0000)),
            (0x36a0_0000_0000_0000, Some(0x0000_0001)),
            (0x36a0_0000_0000_0001, None),
            (0x380f_ffff_c000_0000, Some(0x007f_ffff)),
            (0x3810_0000_0000_0000, Some(0x0080_0000)),
            (0x47ef_ffff_e000_0000, Some(0x7f7f_ffff)),
            (0x47f0_0000_0000_0000, None),
            (0x3ff0_0000_0000_0000, Some(0x3f80_0000)),
            (0x3ff1_9999_9999_999a, None),
            (0x7ff0_0000_0000_0000, Some(0x7f80_0000)),
            (0xfff0_0000_0000_0000, Some(0xff80_0000)),
            (0x7ff8_0000_0000_0000, Some(0x7fc0_0000)),
            (0xfff8_0000_0000_0000, Some(0xffc0_0000)),
            (0x7ff8_0000_0000_0001, None),
            (0x7ff0_0000_0000_0001, None),
        ];

        for &(source, expected) in cases {
            assert_eq!(
                exact_f32(f64::from_bits(source)).map(f32::to_bits),
                expected
            );
        }
    }
}
