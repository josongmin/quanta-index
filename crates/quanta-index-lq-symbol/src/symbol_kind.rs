//! `SymbolKind` — the closed enum of symbol categories the LEX-05 lane
//! emits.
//!
//! v1 ships the 12 kinds named in the LEX-05 spec sheet §3.3 subset:
//! `Function`, `Method`, `Class`, `Struct`, `Enum`, `Trait`, `Interface`,
//! `Variable`, `Constant`, `Module`, `Macro`, `TypeAlias`. The full
//! 20-value enum named in the spec is deferred to a follow-up; the values
//! not yet present are simply not emitted by any v1 extractor — no silent
//! re-tagging.
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

/// v1 symbol category. Wire form is `SCREAMING_SNAKE_CASE`
/// (`FUNCTION`, `METHOD`, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Variable,
    Constant,
    Module,
    Macro,
    TypeAlias,
}

impl SymbolKind {
    /// Stable `SCREAMING_SNAKE_CASE` wire string.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Function => "FUNCTION",
            Self::Method => "METHOD",
            Self::Class => "CLASS",
            Self::Struct => "STRUCT",
            Self::Enum => "ENUM",
            Self::Trait => "TRAIT",
            Self::Interface => "INTERFACE",
            Self::Variable => "VARIABLE",
            Self::Constant => "CONSTANT",
            Self::Module => "MODULE",
            Self::Macro => "MACRO",
            Self::TypeAlias => "TYPE_ALIAS",
        }
    }

    /// Inverse of [`SymbolKind::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "FUNCTION" => Self::Function,
            "METHOD" => Self::Method,
            "CLASS" => Self::Class,
            "STRUCT" => Self::Struct,
            "ENUM" => Self::Enum,
            "TRAIT" => Self::Trait,
            "INTERFACE" => Self::Interface,
            "VARIABLE" => Self::Variable,
            "CONSTANT" => Self::Constant,
            "MODULE" => Self::Module,
            "MACRO" => Self::Macro,
            "TYPE_ALIAS" => Self::TypeAlias,
            _ => return None,
        };
        Some(v)
    }

    /// The complete set of v1 values, in declaration order.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Function,
            Self::Method,
            Self::Class,
            Self::Struct,
            Self::Enum,
            Self::Trait,
            Self::Interface,
            Self::Variable,
            Self::Constant,
            Self::Module,
            Self::Macro,
            Self::TypeAlias,
        ]
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for SymbolKind {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for SymbolKind {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = SymbolKind;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SymbolKind SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<SymbolKind, E> {
                SymbolKind::from_code_str(v).ok_or_else(|| E::unknown_variant(v, &["<SymbolKind>"]))
            }
        }
        de.deserialize_str(V)
    }
}

#[cfg(test)]
mod tests {
    use super::SymbolKind;

    #[test]
    fn all_codes_unique_and_screaming_snake_case() {
        let mut seen: Vec<&'static str> = Vec::new();
        for k in SymbolKind::all() {
            let s = k.as_code_str();
            assert!(!seen.contains(&s), "duplicate: {s}");
            assert!(
                s.chars().all(|c| c.is_ascii_uppercase() || c == '_'),
                "not SCREAMING_SNAKE_CASE: {s}"
            );
            seen.push(s);
        }
        assert_eq!(seen.len(), 12);
    }

    #[test]
    fn roundtrip_via_code_str() {
        for k in SymbolKind::all() {
            assert_eq!(SymbolKind::from_code_str(k.as_code_str()), Some(*k));
        }
    }

    #[test]
    fn unknown_returns_none() {
        assert_eq!(SymbolKind::from_code_str("NOT_A_KIND"), None);
        assert_eq!(SymbolKind::from_code_str(""), None);
        assert_eq!(SymbolKind::from_code_str("function"), None);
    }

    #[test]
    fn serde_roundtrip_via_ciborium() {
        for k in SymbolKind::all() {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(k, &mut buf);
            assert!(w.is_ok(), "serialize failed for {k:?}");
            let read: Result<SymbolKind, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *k),
                Err(e) => assert!(false, "deserialize failed for {k:?}: {e}"),
            }
        }
    }
}
