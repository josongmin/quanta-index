//! Manual wire codec for the evidence contract.
//!
//! This repository bans `#[derive(Serialize)]`/`#[derive(Deserialize)]`
//! (`tools/ci/lint/check-rust-derive-allowlist.py`): derive-generated serde code
//! is a build-cost and auditability regression. The contract therefore encodes
//! and decodes its own fields explicitly.
//!
//! Decoding is strict by construction:
//!
//! - [`parse_strict`] rejects a **duplicate object key at any depth** by
//!   implementing `Deserialize` for a wrapper that walks the document itself;
//! - every struct decode calls [`exact_keys`], so an unknown or missing field
//!   is a typed refusal instead of a silently ignored field;
//! - numeric decoding goes through `serde_json`'s checked accessors, so a
//!   string where a number belongs is refused rather than coerced.

use std::fmt;

use serde::de::{Deserialize, Deserializer, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::error::ProtocolError;

pub use serde_json::Value as JsonValue;

/// A value that can be encoded to and decoded from the canonical wire form.
pub trait Wire: Sized {
    /// Encode into a JSON value (no field ordering guarantee; the canonical
    /// writer sorts keys).
    fn encode(&self) -> Result<Value, ProtocolError>;

    /// Decode from a JSON value, refusing unknown, missing or mistyped fields.
    fn decode(value: &Value) -> Result<Self, ProtocolError>;
}

/// Build a JSON object from ordered entries.
#[must_use]
pub fn object(entries: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        let _previous = map.insert(key.to_owned(), value);
    }
    Value::Object(map)
}

/// A JSON number that cannot lose precision.
#[must_use]
pub fn number_u64(value: u64) -> Value {
    Value::Number(Number::from(value))
}

/// A JSON number; a non-finite value is refused, never defaulted.
pub fn number_f64(value: f64) -> Result<Value, ProtocolError> {
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| ProtocolError::semantic(format!("number {value} is not finite")))
}

/// A JSON integer number.
#[must_use]
pub fn number_i32(value: i32) -> Value {
    Value::Number(Number::from(value))
}

/// Require `value` to be an object.
pub fn fields<'a>(value: &'a Value, where_: &str) -> Result<&'a Map<String, Value>, ProtocolError> {
    value
        .as_object()
        .ok_or_else(|| ProtocolError::semantic(format!("{where_} must be an object")))
}

/// Require the object to contain exactly `keys`.
pub fn exact_keys(
    object: &Map<String, Value>,
    keys: &[&str],
    where_: &str,
) -> Result<(), ProtocolError> {
    let missing: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|key| !object.contains_key(*key))
        .collect();
    let unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| !keys.contains(key))
        .collect();
    if missing.is_empty() && unknown.is_empty() {
        return Ok(());
    }
    Err(ProtocolError::semantic(format!(
        "{where_} must contain exactly {keys:?}; missing {missing:?}, unknown {unknown:?}"
    )))
}

/// Fetch one already-validated field.
pub fn field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    where_: &str,
) -> Result<&'a Value, ProtocolError> {
    object
        .get(key)
        .ok_or_else(|| ProtocolError::semantic(format!("{where_} is missing field {key:?}")))
}

impl Wire for String {
    fn encode(&self) -> Result<Value, ProtocolError> {
        Ok(Value::String(self.clone()))
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ProtocolError::semantic("expected a string".to_owned()))
    }
}

impl Wire for bool {
    fn encode(&self) -> Result<Value, ProtocolError> {
        Ok(Value::Bool(*self))
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        value
            .as_bool()
            .ok_or_else(|| ProtocolError::semantic("expected a boolean".to_owned()))
    }
}

impl Wire for u64 {
    fn encode(&self) -> Result<Value, ProtocolError> {
        Ok(number_u64(*self))
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        value
            .as_u64()
            .ok_or_else(|| ProtocolError::semantic("expected a non-negative integer".to_owned()))
    }
}

impl Wire for u32 {
    fn encode(&self) -> Result<Value, ProtocolError> {
        Ok(number_u64(u64::from(*self)))
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        let wide = value
            .as_u64()
            .ok_or_else(|| ProtocolError::semantic("expected a non-negative integer".to_owned()))?;
        u32::try_from(wide).map_err(|error| {
            ProtocolError::semantic(format!("integer {wide} out of range: {error}"))
        })
    }
}

impl Wire for i32 {
    fn encode(&self) -> Result<Value, ProtocolError> {
        Ok(number_i32(*self))
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        let wide = value
            .as_i64()
            .ok_or_else(|| ProtocolError::semantic("expected an integer".to_owned()))?;
        i32::try_from(wide).map_err(|error| {
            ProtocolError::semantic(format!("integer {wide} out of range: {error}"))
        })
    }
}

impl Wire for f64 {
    fn encode(&self) -> Result<Value, ProtocolError> {
        number_f64(*self)
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        value
            .as_f64()
            .ok_or_else(|| ProtocolError::semantic("expected a number".to_owned()))
    }
}

impl<T: Wire> Wire for Option<T> {
    fn encode(&self) -> Result<Value, ProtocolError> {
        self.as_ref().map_or(Ok(Value::Null), Wire::encode)
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        if value.is_null() {
            Ok(None)
        } else {
            T::decode(value).map(Some)
        }
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn encode(&self) -> Result<Value, ProtocolError> {
        let mut items = Vec::with_capacity(self.len());
        for item in self {
            items.push(item.encode()?);
        }
        Ok(Value::Array(items))
    }

    fn decode(value: &Value) -> Result<Self, ProtocolError> {
        let array = value
            .as_array()
            .ok_or_else(|| ProtocolError::semantic("expected an array".to_owned()))?;
        let mut items = Vec::with_capacity(array.len());
        for item in array {
            items.push(T::decode(item)?);
        }
        Ok(items)
    }
}

/// A parsed JSON document that contained no duplicate object key.
pub struct StrictValue(pub Value);

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value with no duplicate object keys")
    }

    fn visit_bool<E: DeError>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }

    fn visit_i64<E: DeError>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E: DeError>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E: DeError>(self, value: f64) -> Result<Self::Value, E> {
        let number = Number::from_f64(value).ok_or_else(|| E::custom("non-finite number"))?;
        Ok(StrictValue(Value::Number(number)))
    }

    fn visit_str<E: DeError>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value.to_owned())))
    }

    fn visit_string<E: DeError>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }

    fn visit_none<E: DeError>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_unit<E: DeError>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        StrictValue::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<StrictValue>()? {
            items.push(item.0);
        }
        Ok(StrictValue(Value::Array(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(A::Error::custom(format!("duplicate JSON key {key:?}")));
            }
            let value = map.next_value::<StrictValue>()?;
            let _previous = object.insert(key, value.0);
        }
        Ok(StrictValue(Value::Object(object)))
    }
}

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor)
    }
}

/// Parse JSON text, refusing duplicate object keys at any depth and trailing data.
pub fn parse_strict(text: &str) -> Result<Value, ProtocolError> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let strict = StrictValue::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(strict.0)
}

/// Implement [`Wire`] for a flat struct with the listed fields.
///
/// The `tag` form encodes an extra `"kind"` discriminant, which is how the
/// payload variants stay distinguishable on the wire.
#[macro_export]
macro_rules! impl_wire {
    ($name:ident { $($field:ident),* $(,)? }) => {
        impl $crate::codec::Wire for $name {
            fn encode(&self) -> Result<$crate::codec::JsonValue, $crate::error::ProtocolError> {
                let entries: Vec<(&str, $crate::codec::JsonValue)> = vec![
                    $(
                        (
                            stringify!($field),
                            $crate::codec::Wire::encode(&self.$field)?,
                        ),
                    )*
                ];
                Ok($crate::codec::object(entries))
            }

            fn decode(
                value: &$crate::codec::JsonValue,
            ) -> Result<Self, $crate::error::ProtocolError> {
                let object = $crate::codec::fields(value, stringify!($name))?;
                let keys: Vec<&str> = vec![$( stringify!($field) ),*];
                $crate::codec::exact_keys(object, &keys, stringify!($name))?;
                Ok(Self {
                    $(
                        $field: $crate::codec::Wire::decode($crate::codec::field(
                            object,
                            stringify!($field),
                            stringify!($name),
                        )?)?,
                    )*
                })
            }
        }
    };
    ($name:ident tag $tag:literal { $($field:ident),* $(,)? }) => {
        impl $crate::codec::Wire for $name {
            fn encode(&self) -> Result<$crate::codec::JsonValue, $crate::error::ProtocolError> {
                let mut entries: Vec<(&str, $crate::codec::JsonValue)> =
                    vec![("kind", $crate::codec::JsonValue::String($tag.to_owned()))];
                $(
                    entries.push((
                        stringify!($field),
                        $crate::codec::Wire::encode(&self.$field)?,
                    ));
                )*
                Ok($crate::codec::object(entries))
            }

            fn decode(
                value: &$crate::codec::JsonValue,
            ) -> Result<Self, $crate::error::ProtocolError> {
                let object = $crate::codec::fields(value, stringify!($name))?;
                let mut keys: Vec<&str> = vec!["kind"];
                $( keys.push(stringify!($field)); )*
                $crate::codec::exact_keys(object, &keys, stringify!($name))?;
                let found = $crate::codec::field(object, "kind", stringify!($name))?;
                if found.as_str() != Some($tag) {
                    return Err($crate::error::ProtocolError::semantic(format!(
                        "{}: kind must be {:?}, found {:?}",
                        stringify!($name),
                        $tag,
                        found
                    )));
                }
                Ok(Self {
                    $(
                        $field: $crate::codec::Wire::decode($crate::codec::field(
                            object,
                            stringify!($field),
                            stringify!($name),
                        )?)?,
                    )*
                })
            }
        }
    };
}
