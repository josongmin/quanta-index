//! Declarative macros that fold the repetitive manual `serde` impls used for
//! newtype IDs in this crate (and re-used by downstream crates that build other
//! newtype identifiers).
//!
//! Why declarative macros, not the serde proc-macro derive attribute: the
//! workspace bans serde proc-macro derives (enforced by
//! `check-rust-derive-allowlist.py`) because derive expansion dominates cold-build time
//! and hides wire shape from review. These `macro_rules!` expansions stay
//! crate-local, expand fast, and emit the same hand-written
//! `serde::ser::Serializer` / `serde::de::Visitor` code we previously had
//! transcribed by hand for every newtype.

/// Define a `String`-backed newtype with manual `serde::Serialize` and
/// `serde::Deserialize` impls.
#[macro_export]
macro_rules! string_newtype {
    ($Name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
        pub struct $Name(String);

        impl $Name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl serde::Serialize for $Name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> serde::Deserialize<'de> for $Name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct NewtypeVisitor;

                impl serde::de::Visitor<'_> for NewtypeVisitor {
                    type Value = $Name;

                    fn expecting(
                        &self,
                        formatter: &mut core::fmt::Formatter<'_>,
                    ) -> core::fmt::Result {
                        formatter.write_str(concat!("a string for ", stringify!($Name)))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        Ok($Name(value.to_owned()))
                    }

                    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        Ok($Name(value))
                    }
                }

                deserializer.deserialize_string(NewtypeVisitor)
            }
        }
    };
}

/// Define a `u64`-backed newtype with manual `serde::Serialize` and
/// `serde::Deserialize` impls.
#[macro_export]
macro_rules! u64_newtype {
    ($Name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Default)]
        pub struct $Name(u64);

        impl $Name {
            pub const ZERO: Self = Self(0);

            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl serde::Serialize for $Name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_u64(self.0)
            }
        }

        impl<'de> serde::Deserialize<'de> for $Name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct NewtypeVisitor;

                impl serde::de::Visitor<'_> for NewtypeVisitor {
                    type Value = $Name;

                    fn expecting(
                        &self,
                        formatter: &mut core::fmt::Formatter<'_>,
                    ) -> core::fmt::Result {
                        formatter.write_str(concat!(
                            "an unsigned 64-bit integer for ",
                            stringify!($Name),
                        ))
                    }

                    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        Ok($Name(value))
                    }

                    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        match u64::try_from(value) {
                            Ok(unsigned) => Ok($Name(unsigned)),
                            Err(_err) => Err(serde::de::Error::invalid_value(
                                serde::de::Unexpected::Signed(value),
                                &"a non-negative integer",
                            )),
                        }
                    }

                    fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        match u64::try_from(value) {
                            Ok(unsigned) => Ok($Name(unsigned)),
                            Err(_err) => Err(serde::de::Error::invalid_value(
                                serde::de::Unexpected::Other("u128 out of range for u64"),
                                &"a value fitting in u64",
                            )),
                        }
                    }
                }

                deserializer.deserialize_u64(NewtypeVisitor)
            }
        }
    };
}
