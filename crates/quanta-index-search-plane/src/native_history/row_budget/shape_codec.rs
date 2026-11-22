//! Allocation-free field and variant identifiers for the History shape pass.

use std::fmt;

use serde::{
    Deserializer,
    de::{self, DeserializeSeed, Visitor},
};

/// Resolve only declared History shape fields without allocating key strings.
pub(in crate::native_history) struct ShapeFieldSeed(
    pub(in crate::native_history) &'static [&'static str],
);

impl<'de> DeserializeSeed<'de> for ShapeFieldSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        struct Key(&'static [&'static str]);
        impl Visitor<'_> for Key {
            type Value = &'static str;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a known History row field")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                self.0
                    .iter()
                    .copied()
                    .find(|field| *field == value)
                    .ok_or_else(|| E::unknown_field(value, self.0))
            }

            fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                let text = std::str::from_utf8(value)
                    .map_err(|_| E::invalid_value(de::Unexpected::Bytes(value), &self))?;
                self.visit_str(text)
            }
        }
        decoder.deserialize_identifier(Key(self.0))
    }
}

macro_rules! closed_shape {
    ($vis:vis struct $name:ident {
        $($field:ident $(as $wire:ident)? : $ty:ty $(= $map_default:ident)?),+ $(,)?
    }) => {
        $vis struct $name {
            $($field: $ty,)+
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                const FIELDS: &[&str] = &[$($crate::native_history::row_budget::shape_codec::closed_shape!(@name $field $(as $wire)?)),+];
                struct Shape;
                impl<'de> serde::de::Visitor<'de> for Shape {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!("a ", stringify!($name), " History row shape"))
                    }

                    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                        $(let mut $field: Option<$ty> = None;)+
                        while let Some(key) = map.next_key_seed($crate::native_history::row_budget::shape_codec::ShapeFieldSeed(FIELDS))? {
                            match key {
                                $($crate::native_history::row_budget::shape_codec::closed_shape!(@name $field $(as $wire)?) => {
                                    if $field.is_some() {
                                        return Err(serde::de::Error::duplicate_field($crate::native_history::row_budget::shape_codec::closed_shape!(@name $field $(as $wire)?)));
                                    }
                                    $field = Some(map.next_value::<$ty>()?);
                                },)+
                                _ => unreachable!("row field seed only returns schema keys"),
                            }
                        }
                        Ok($name { $($field: $crate::native_history::row_budget::shape_codec::closed_shape!(@map_value $field $(= $map_default)? $(as $wire)?),)+ })
                    }

                    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                        let mut index = 0usize;
                        $(let $field = seq.next_element::<$ty>()?
                            .ok_or_else(|| serde::de::Error::invalid_length(index, &self))?;
                          index = index.saturating_add(1);)+
                        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                            return Err(serde::de::Error::invalid_length(index.saturating_add(1), &self));
                        }
                        Ok($name { $($field,)+ })
                    }
                }
                decoder.deserialize_struct(stringify!($name), FIELDS, Shape)
            }
        }
    };
    (@name $field:ident) => { stringify!($field) };
    (@name $field:ident as $wire:ident) => { stringify!($wire) };
    (@map_value $field:ident) => { $field.ok_or_else(|| serde::de::Error::missing_field(stringify!($field)))? };
    (@map_value $field:ident as $wire:ident) => { $field.ok_or_else(|| serde::de::Error::missing_field(stringify!($wire)))? };
    (@map_value $field:ident = default) => { $field.unwrap_or_default() };
    (@map_value $field:ident = default as $wire:ident) => { $field.unwrap_or_default() };
}

pub(in crate::native_history) struct ShapeVariantSeed(
    pub(in crate::native_history) &'static [&'static str],
);

impl<'de> DeserializeSeed<'de> for ShapeVariantSeed {
    type Value = &'static str;

    fn deserialize<D: Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        struct Variant(&'static [&'static str]);
        impl Visitor<'_> for Variant {
            type Value = &'static str;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a History row variant")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                self.0
                    .iter()
                    .copied()
                    .find(|name| *name == value)
                    .ok_or_else(|| E::unknown_variant(value, self.0))
            }

            fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                let text = std::str::from_utf8(value)
                    .map_err(|_| E::invalid_value(de::Unexpected::Bytes(value), &self))?;
                self.visit_str(text)
            }
        }
        decoder.deserialize_identifier(Variant(self.0))
    }
}

macro_rules! newtype_shape {
    (enum $name:ident { $($variant:ident : $ty:ty),+ $(,)? }) => {
        enum $name {
            $($variant($ty),)+
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
                const VARIANTS: &[&str] = &[$(stringify!($variant)),+];
                struct Shape;
                impl<'de> serde::de::Visitor<'de> for Shape {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str(concat!("a ", stringify!($name), " History row variant"))
                    }

                    fn visit_enum<A: serde::de::EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
                        let (name, value) = serde::de::EnumAccess::variant_seed(data, $crate::native_history::row_budget::shape_codec::ShapeVariantSeed(VARIANTS))?;
                        match name {
                            $(stringify!($variant) => serde::de::VariantAccess::newtype_variant::<$ty>(value).map($name::$variant),)+
                            _ => unreachable!("row variant seed only returns schema names"),
                        }
                    }
                }
                decoder.deserialize_enum(stringify!($name), VARIANTS, Shape)
            }
        }
    };
}

pub(in crate::native_history) use {closed_shape, newtype_shape};
