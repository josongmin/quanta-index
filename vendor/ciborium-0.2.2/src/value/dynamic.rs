use super::Integer;

use alloc::{boxed::Box, string::String, vec::Vec};

/// A representation of a dynamic CBOR value that can handled dynamically
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub enum Value {
    /// An integer
    Integer(Integer),

    /// Bytes
    Bytes(Vec<u8>),

    /// A float
    Float(f64),

    /// A string
    Text(String),

    /// A boolean
    Bool(bool),

    /// Null
    Null,

    /// Tag
    Tag(u64, Box<Value>),

    /// An array
    Array(Vec<Value>),

    /// A map
    Map(Vec<(Value, Value)>),
}

impl Value {
    /// Returns true if the `Value` is an `Integer`. Returns false otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Integer(17.into());
    ///
    /// assert!(value.is_integer());
    /// ```
    #[must_use]
    pub fn is_integer(&self) -> bool {
        self.as_integer().is_some()
    }

    /// If the `Value` is a `Integer`, returns a reference to the associated `Integer` data.
    /// Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Integer(17.into());
    ///
    /// // We can read the number
    /// assert_eq!(17, value.as_integer().unwrap().try_into().unwrap());
    /// ```
    #[must_use]
    pub fn as_integer(&self) -> Option<Integer> {
        if let Value::Integer(int) = self {
            Some(*int)
        } else {
            None
        }
    }

    /// If the `Value` is a `Integer`, returns a the associated `Integer` data as `Ok`.
    /// Returns `Err(Self)` otherwise.
    ///
    /// ```
    /// # use ciborium::{Value, value::Integer};
    /// #
    /// let value = Value::Integer(17.into());
    /// assert_eq!(value.into_integer(), Ok(Integer::from(17)));
    ///
    /// let value = Value::Bool(true);
    /// assert_eq!(value.into_integer(), Err(Value::Bool(true)));
    /// ```
    pub fn into_integer(self) -> Result<Integer, Self> {
        if let Value::Integer(int) = self {
            Ok(int)
        } else {
            Err(self)
        }
    }

    /// Returns true if the `Value` is a `Bytes`. Returns false otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Bytes(vec![104, 101, 108, 108, 111]);
    ///
    /// assert!(value.is_bytes());
    /// ```
    #[must_use]
    pub fn is_bytes(&self) -> bool {
        self.as_bytes().is_some()
    }

    /// If the `Value` is a `Bytes`, returns a reference to the associated bytes vector.
    /// Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Bytes(vec![104, 101, 108, 108, 111]);
    ///
    /// assert_eq!(std::str::from_utf8(value.as_bytes().unwrap()).unwrap(), "hello");
    /// ```
    #[must_use]
    pub fn as_bytes(&self) -> Option<&Vec<u8>> {
        if let Value::Bytes(bytes) = self {
            Some(bytes)
        } else {
            None
        }
    }

    /// If the `Value` is a `Bytes`, returns a mutable reference to the associated bytes vector.
    /// Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let mut value = Value::Bytes(vec![104, 101, 108, 108, 111]);
    /// value.as_bytes_mut().unwrap().clear();
    ///
    /// assert_eq!(value, Value::Bytes(vec![]));
    /// ```
    pub fn as_bytes_mut(&mut self) -> Option<&mut Vec<u8>> {
        if let Value::Bytes(bytes) = self {
            Some(bytes)
        } else {
            None
        }
    }

    /// If the `Value` is a `Bytes`, returns a the associated `Vec<u8>` data as `Ok`.
    /// Returns `Err(Self)` otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Bytes(vec![104, 101, 108, 108, 111]);
    /// assert_eq!(value.into_bytes(), Ok(vec![104, 101, 108, 108, 111]));
    ///
    /// let value = Value::Bool(true);
    /// assert_eq!(value.into_bytes(), Err(Value::Bool(true)));
    /// ```
    pub fn into_bytes(self) -> Result<Vec<u8>, Self> {
        if let Value::Bytes(vec) = self {
            Ok(vec)
        } else {
            Err(self)
        }
    }

    /// Returns true if the `Value` is a `Float`. Returns false otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Float(17.0.into());
    ///
    /// assert!(value.is_float());
    /// ```
    #[must_use]
    pub fn is_float(&self) -> bool {
        self.as_float().is_some()
    }

    /// If the `Value` is a `Float`, returns a reference to the associated float data.
    /// Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Float(17.0.into());
    ///
    /// // We can read the float number
    /// assert_eq!(value.as_float().unwrap(), 17.0_f64);
    /// ```
    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        if let Value::Float(f) = self {
            Some(*f)
        } else {
            None
        }
    }

    /// If the `Value` is a `Float`, returns a the associated `f64` data as `Ok`.
    /// Returns `Err(Self)` otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Float(17.);
    /// assert_eq!(value.into_float(), Ok(17.));
    ///
    /// let value = Value::Bool(true);
    /// assert_eq!(value.into_float(), Err(Value::Bool(true)));
    /// ```
    pub fn into_float(self) -> Result<f64, Self> {
        if let Value::Float(f) = self {
            Ok(f)
        } else {
            Err(self)
        }
    }

    /// Returns true if the `Value` is a `Text`. Returns false otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Text(String::from("hello"));
    ///
    /// assert!(value.is_text());
    /// ```
    #[must_use]
    pub fn is_text(&self) -> bool {
        self.as_text().is_some()
    }

    /// If the `Value` is a `Text`, returns a reference to the associated `String` data.
    /// Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Text(String::from("hello"));
    ///
    /// // We can read the String
    /// assert_eq!(value.as_text().unwrap(), "hello");
    /// ```
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        if let Value::Text(s) = self {
            Some(s)
        } else {
            None
        }
    }

    /// If the `Value` is a `Text`, returns a mutable reference to the associated `String` data.
    /// Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let mut value = Value::Text(String::from("hello"));
    /// value.as_text_mut().unwrap().clear();
    ///
    /// assert_eq!(value.as_text().unwrap(), &String::from(""));
    /// ```
    pub fn as_text_mut(&mut self) -> Option<&mut String> {
        if let Value::Text(s) = self {
            Some(s)
        } else {
            None
        }
    }

    /// If the `Value` is a `String`, returns a the associated `String` data as `Ok`.
    /// Returns `Err(Self)` otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Text(String::from("hello"));
    /// assert_eq!(value.into_text().as_deref(), Ok("hello"));
    ///
    /// let value = Value::Bool(true);
    /// assert_eq!(value.into_text(), Err(Value::Bool(true)));
    /// ```
    pub fn into_text(self) -> Result<String, Self> {
        if let Value::Text(s) = self {
            Ok(s)
        } else {
            Err(self)
        }
    }

    /// Returns true if the `Value` is a `Bool`. Returns false otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Bool(false);
    ///
    /// assert!(value.is_bool());
    /// ```
    #[must_use]
    pub fn is_bool(&self) -> bool {
        self.as_bool().is_some()
    }

    /// If the `Value` is a `Bool`, returns a copy of the associated boolean value. Returns None
    /// otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Bool(false);
    ///
    /// assert_eq!(value.as_bool().unwrap(), false);
    /// ```
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        if let Value::Bool(b) = self {
            Some(*b)
        } else {
            None
        }
    }

    /// If the `Value` is a `Bool`, returns a the associated `bool` data as `Ok`.
    /// Returns `Err(Self)` otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Bool(false);
    /// assert_eq!(value.into_bool(), Ok(false));
    ///
    /// let value = Value::Float(17.);
    /// assert_eq!(value.into_bool(), Err(Value::Float(17.)));
    /// ```
    pub fn into_bool(self) -> Result<bool, Self> {
        if let Value::Bool(b) = self {
            Ok(b)
        } else {
            Err(self)
        }
    }

    /// Returns true if the `Value` is a `Null`. Returns false otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Null;
    ///
    /// assert!(value.is_null());
    /// ```
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Returns true if the `Value` is a `Tag`. Returns false otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Tag(61, Box::from(Value::Null));
    ///
    /// assert!(value.is_tag());
    /// ```
    #[must_use]
    pub fn is_tag(&self) -> bool {
        self.as_tag().is_some()
    }

    /// If the `Value` is a `Tag`, returns the associated tag value and a reference to the tag `Value`.
    /// Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Tag(61, Box::from(Value::Bytes(vec![104, 101, 108, 108, 111])));
    ///
    /// let (tag, data) = value.as_tag().unwrap();
    /// assert_eq!(tag, 61);
    /// assert_eq!(data, &Value::Bytes(vec![104, 101, 108, 108, 111]));
    /// ```
    #[must_use]
    pub fn as_tag(&self) -> Option<(u64, &Value)> {
        if let Value::Tag(tag, data) = self {
            Some((*tag, data))
        } else {
            None
        }
    }

    /// If the `Value` is a `Tag`, returns the associated tag value and a mutable reference
    /// to the tag `Value`. Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let mut value = Value::Tag(61, Box::from(Value::Bytes(vec![104, 101, 108, 108, 111])));
    ///
    /// let (tag, mut data) = value.as_tag_mut().unwrap();
    /// data.as_bytes_mut().unwrap().clear();
    /// assert_eq!(tag, &61);
    /// assert_eq!(data, &Value::Bytes(vec![]));
    /// ```
    pub fn as_tag_mut(&mut self) -> Option<(&mut u64, &mut Value)> {
        if let Value::Tag(tag, data) = self {
            Some((tag, data.as_mut()))
        } else {
            None
        }
    }

    /// If the `Value` is a `Tag`, returns a the associated pair of `u64` and `Box<value>` data as `Ok`.
    /// Returns `Err(Self)` otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Tag(7, Box::new(Value::Float(12.)));
    /// assert_eq!(value.into_tag(), Ok((7, Box::new(Value::Float(12.)))));
    ///
    /// let value = Value::Bool(true);
    /// assert_eq!(value.into_tag(), Err(Value::Bool(true)));
    /// ```
    pub fn into_tag(self) -> Result<(u64, Box<Value>), Self> {
        if let Value::Tag(tag, value) = self {
            Ok((tag, value))
        } else {
            Err(self)
        }
    }

    /// Returns true if the `Value` is an Array. Returns false otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Array(
    ///     vec![
    ///         Value::Text(String::from("foo")),
    ///         Value::Text(String::from("bar"))
    ///     ]
    /// );
    ///
    /// assert!(value.is_array());
    /// ```
    #[must_use]
    pub fn is_array(&self) -> bool {
        self.as_array().is_some()
    }

    /// If the `Value` is an Array, returns a reference to the associated vector. Returns None
    /// otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Array(
    ///     vec![
    ///         Value::Text(String::from("foo")),
    ///         Value::Text(String::from("bar"))
    ///     ]
    /// );
    ///
    /// // The length of `value` is 2 elements.
    /// assert_eq!(value.as_array().unwrap().len(), 2);
    /// ```
    #[must_use]
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        if let Value::Array(array) = self {
            Some(array)
        } else {
            None
        }
    }

    /// If the `Value` is an Array, returns a mutable reference to the associated vector.
    /// Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let mut value = Value::Array(
    ///     vec![
    ///         Value::Text(String::from("foo")),
    ///         Value::Text(String::from("bar"))
    ///     ]
    /// );
    ///
    /// value.as_array_mut().unwrap().clear();
    /// assert_eq!(value, Value::Array(vec![]));
    /// ```
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        if let Value::Array(list) = self {
            Some(list)
        } else {
            None
        }
    }

    /// If the `Value` is a `Array`, returns a the associated `Vec<Value>` data as `Ok`.
    /// Returns `Err(Self)` otherwise.
    ///
    /// ```
    /// # use ciborium::{Value, value::Integer};
    /// #
    /// let mut value = Value::Array(
    ///     vec![
    ///         Value::Integer(17.into()),
    ///         Value::Float(18.),
    ///     ]
    /// );
    /// assert_eq!(value.into_array(), Ok(vec![Value::Integer(17.into()), Value::Float(18.)]));
    ///
    /// let value = Value::Bool(true);
    /// assert_eq!(value.into_array(), Err(Value::Bool(true)));
    /// ```
    pub fn into_array(self) -> Result<Vec<Value>, Self> {
        if let Value::Array(vec) = self {
            Ok(vec)
        } else {
            Err(self)
        }
    }

    /// Returns true if the `Value` is a Map. Returns false otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Map(
    ///     vec![
    ///         (Value::Text(String::from("foo")), Value::Text(String::from("bar")))
    ///     ]
    /// );
    ///
    /// assert!(value.is_map());
    /// ```
    #[must_use]
    pub fn is_map(&self) -> bool {
        self.as_map().is_some()
    }

    /// If the `Value` is a Map, returns a reference to the associated Map data. Returns None
    /// otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let value = Value::Map(
    ///     vec![
    ///         (Value::Text(String::from("foo")), Value::Text(String::from("bar")))
    ///     ]
    /// );
    ///
    /// // The length of data is 1 entry (1 key/value pair).
    /// assert_eq!(value.as_map().unwrap().len(), 1);
    ///
    /// // The content of the first element is what we expect
    /// assert_eq!(
    ///     value.as_map().unwrap().get(0).unwrap(),
    ///     &(Value::Text(String::from("foo")), Value::Text(String::from("bar")))
    /// );
    /// ```
    #[must_use]
    pub fn as_map(&self) -> Option<&Vec<(Value, Value)>> {
        if let Value::Map(map) = self {
            Some(map)
        } else {
            None
        }
    }

    /// If the `Value` is a Map, returns a mutable reference to the associated Map Data.
    /// Returns None otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let mut value = Value::Map(
    ///     vec![
    ///         (Value::Text(String::from("foo")), Value::Text(String::from("bar")))
    ///     ]
    /// );
    ///
    /// value.as_map_mut().unwrap().clear();
    /// assert_eq!(value, Value::Map(vec![]));
    /// assert_eq!(value.as_map().unwrap().len(), 0);
    /// ```
    pub fn as_map_mut(&mut self) -> Option<&mut Vec<(Value, Value)>> {
        if let Value::Map(map) = self {
            Some(map)
        } else {
            None
        }
    }

    /// If the `Value` is a `Map`, returns a the associated `Vec<(Value, Value)>` data as `Ok`.
    /// Returns `Err(Self)` otherwise.
    ///
    /// ```
    /// # use ciborium::Value;
    /// #
    /// let mut value = Value::Map(
    ///     vec![
    ///         (Value::Text(String::from("key")), Value::Float(18.)),
    ///     ]
    /// );
    /// assert_eq!(value.into_map(), Ok(vec![(Value::Text(String::from("key")), Value::Float(18.))]));
    ///
    /// let value = Value::Bool(true);
    /// assert_eq!(value.into_map(), Err(Value::Bool(true)));
    /// ```
    pub fn into_map(self) -> Result<Vec<(Value, Value)>, Self> {
        if let Value::Map(map) = self {
            Ok(map)
        } else {
            Err(self)
        }
    }
}

macro_rules! implfrom {
    ($($v:ident($t:ty)),+ $(,)?) => {
        $(
            impl From<$t> for Value {
                #[inline]
                fn from(value: $t) -> Self {
                    Self::$v(value.into())
                }
            }
        )+
    };
}

implfrom! {
    Integer(Integer),
    Integer(u64),
    Integer(i64),
    Integer(u32),
    Integer(i32),
    Integer(u16),
    Integer(i16),
    Integer(u8),
    Integer(i8),

    Bytes(Vec<u8>),
    Bytes(&[u8]),

    Float(f64),
    Float(f32),

    Text(String),
    Text(&str),

    Bool(bool),

    Array(&[Value]),
    Array(Vec<Value>),

    Map(&[(Value, Value)]),
    Map(Vec<(Value, Value)>),
}

impl From<u128> for Value {
    #[inline]
    fn from(value: u128) -> Self {
        if let Ok(x) = Integer::try_from(value) {
            return Value::Integer(x);
        }

        let mut bytes = &value.to_be_bytes()[..];
        while let Some((first, rest)) = bytes.split_first() {
            if *first != 0 {
                break;
            }
            bytes = rest;
        }

        Value::Tag(ciborium_ll::tag::BIGPOS, Value::Bytes(bytes.into()).into())
    }
}

impl From<i128> for Value {
    #[inline]
    fn from(value: i128) -> Self {
        if let Ok(x) = Integer::try_from(value) {
            return Value::Integer(x);
        }

        let bits = u128::from_be_bytes(value.to_be_bytes());
        let (tag, raw) = if value.is_negative() {
            (ciborium_ll::tag::BIGNEG, bits ^ !0)
        } else {
            (ciborium_ll::tag::BIGPOS, bits)
        };

        let mut bytes = &raw.to_be_bytes()[..];
        while let Some((first, rest)) = bytes.split_first() {
            if *first != 0 {
                break;
            }
            bytes = rest;
        }

        Value::Tag(tag, Value::Bytes(bytes.into()).into())
    }
}

impl From<char> for Value {
    #[inline]
    fn from(value: char) -> Self {
        let mut v = String::with_capacity(1);
        v.push(value);
        Value::Text(v)
    }
}
