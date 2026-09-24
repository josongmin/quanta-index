//! Declarative macros that fold the repetitive manual `serde` impls used for
//! newtype IDs in this crate.
//!
//! Why declarative macros, not the serde proc-macro derive attribute:
//! the workspace bans serde proc-macro derives (enforced by
//! `check-rust-derive-allowlist.py`) because derive expansion dominates cold-build time
//! and hides wire shape from review. These `macro_rules!` expansions stay
//! crate-local, expand fast, and emit the same hand-written
//! `serde::ser::Serializer` / `serde::de::Visitor` code we previously had
//! transcribed by hand for every newtype. The macro bodies below contain only
//! `impl serde::Serialize` / `impl serde::Deserialize` blocks — no proc-macro
//! derive is ever emitted.

/// Define a `String`-backed newtype with manual `serde::Serialize` and
/// `serde::Deserialize` impls.
///
/// Expansion contract:
/// * `pub struct $Name(String)` with `Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd`.
/// * `$Name::new(impl Into<String>) -> Self`, `$Name::as_str(&self) -> &str`,
///   `$Name::into_inner(self) -> String`.
/// * `serde::Serialize` calls `serialize_str(&self.0)`.
/// * `serde::Deserialize` drives a unit visitor through `deserialize_string`,
///   handling both `visit_str` (borrowed) and `visit_string` (owned) so the
///   wire decoder can avoid an extra copy when the input is already owned.
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
///
/// Expansion contract:
/// * `pub struct $Name(u64)` with `Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Default`.
/// * `$Name::ZERO`, `$Name::new(u64) -> Self`, `$Name::get(self) -> u64`,
///   all `const fn` / associated `const`.
/// * `serde::Serialize` calls `serialize_u64(self.0)`.
/// * `serde::Deserialize` drives a unit visitor through `deserialize_u64`,
///   accepting `visit_u64`, `visit_i64` (with `u64::try_from` guard), and
///   `visit_u128` (with `u64::try_from` guard). This matches what
///   `serde_json`, `ciborium`, and `toml` emit for non-negative integers.
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
