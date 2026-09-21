//! Foundational types for the STR-01 structural pattern engine.
//!
//! [`ByteSpan`] captures local match offsets, [`MetaVar`] is the pattern-side
//! capture name (e.g. `$X` -> `MetaVar("X")`), and [`LangId`] is the closed
//! v1 ship set from STR-01 §4.10.
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::StructuralError;

/// Hard cap on the number of [`crate::pattern::PatternNode`] entries a
/// single [`crate::pattern::StructuralPattern`] may contain. Exceeding
/// this surfaces `PLAN_LIMIT_EXCEEDED { dimension = NODE_COUNT }`.
pub const MAX_STRUCTURAL_NODES: u32 = 256;

/// Saturating cast `u32 -> usize`.
///
/// On every platform we ship to, `usize` is ≥ 32 bits so this is
/// lossless; the fallback is only there to keep us off the banned
/// `unwrap_or` / `unwrap_or_else` family on hypothetical 16-bit targets.
#[expect(
    clippy::option_if_let_else,
    reason = "the clippy-suggested `.map_or(usize::MAX, |x| x)` then trips `unnecessary_result_map_or`; identity-map is the literal point of this branch"
)]
fn u32_to_usize_saturating(v: u32) -> usize {
    if let Ok(x) = usize::try_from(v) {
        x
    } else {
        usize::MAX
    }
}

/// Hard cap on the nesting depth of a [`crate::pattern::PatternNode`] tree.
/// Exceeding this surfaces `PLAN_LIMIT_EXCEEDED { dimension = DEPTH }`.
pub const MAX_DEPTH: u32 = 16;

/// Hard cap on the number of distinct [`MetaVar`] names per pattern.
/// Exceeding this surfaces `PLAN_LIMIT_EXCEEDED { dimension = METAVAR_COUNT }`.
pub const MAX_METAVARS_PER_PATTERN: u32 = 32;

/// Local byte span within a single source document.
///
/// `start <= end` is an invariant enforced at construction. Bounds are
/// `u32` per the STR-01 / LEX-05 ship contract — files larger than 4 GiB
/// are out of scope for the structural plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteSpan {
    start: u32,
    end: u32,
}

impl ByteSpan {
    /// Construct a `ByteSpan` with `start <= end`. Rejects inverted spans
    /// with `STR_PARSE_FAIL` carrying the offending offsets.
    pub fn new(start: u32, end: u32) -> Result<Self, StructuralError> {
        if start > end {
            return Err(StructuralError::parse_fail(
                u32_to_usize_saturating(start),
                format!("ByteSpan start {start} > end {end}"),
            ));
        }
        Ok(Self { start, end })
    }

    /// Start byte offset (inclusive).
    #[must_use]
    pub const fn start(self) -> u32 {
        self.start
    }

    /// End byte offset (exclusive).
    #[must_use]
    pub const fn end(self) -> u32 {
        self.end
    }

    /// Length in bytes. Always non-negative because of the `new` invariant.
    #[must_use]
    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// `true` if the span has zero bytes.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

impl fmt::Display for ByteSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

impl serde::Serialize for ByteSpan {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeTuple as _;
        let mut t = ser.serialize_tuple(2)?;
        t.serialize_element(&self.start)?;
        t.serialize_element(&self.end)?;
        t.end()
    }
}

impl<'de> serde::Deserialize<'de> for ByteSpan {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = ByteSpan;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ByteSpan 2-tuple of (start: u32, end: u32)")
            }
            fn visit_seq<A: serde::de::SeqAccess<'d>>(
                self,
                mut seq: A,
            ) -> Result<ByteSpan, A::Error> {
                let start: u32 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;
                let end: u32 = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
                if start > end {
                    return Err(serde::de::Error::custom(format!(
                        "ByteSpan start {start} > end {end}"
                    )));
                }
                Ok(ByteSpan { start, end })
            }
        }
        de.deserialize_tuple(2, V)
    }
}

/// Pattern-side metavariable name (the `X` in `$X` or `:[X]`).
///
/// Construction rejects empty strings and non-identifier characters per
/// `dsl.md` §8.1: the first character must be ASCII alphabetic or `_`,
/// subsequent characters must be ASCII alphanumeric or `_`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MetaVar(Box<str>);

impl MetaVar {
    /// Construct from a name string. Fails with `STR_INVALID_METAVAR` on
    /// empty / non-identifier inputs.
    pub fn new(name: impl AsRef<str>) -> Result<Self, StructuralError> {
        let s = name.as_ref();
        if s.is_empty() {
            return Err(StructuralError::invalid_metavar(s));
        }
        let mut chars = s.chars();
        let Some(first) = chars.next() else {
            return Err(StructuralError::invalid_metavar(s));
        };
        if !(first.is_ascii_alphabetic() || first == '_') {
            return Err(StructuralError::invalid_metavar(s));
        }
        for c in chars {
            if !(c.is_ascii_alphanumeric() || c == '_') {
                return Err(StructuralError::invalid_metavar(s));
            }
        }
        Ok(Self(s.to_owned().into_boxed_str()))
    }

    /// Borrow the wrapped name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MetaVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for MetaVar {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for MetaVar {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = MetaVar;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("MetaVar identifier string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<MetaVar, E> {
                MetaVar::new(v).map_err(|err| E::custom(format!("{err}")))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<MetaVar, E> {
                self.visit_str(&v)
            }
        }
        de.deserialize_str(V)
    }
}

/// Per-language identifier for the v1 ship-set. Wire form is
/// `SCREAMING_SNAKE_CASE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LangId {
    /// Rust grammar (`tree-sitter-rust` placeholder; deferred per LEX-05).
    Rust,
    /// Python grammar.
    Python,
    /// TypeScript grammar (TS + TSX subgrammars).
    TypeScript,
    /// JavaScript grammar.
    JavaScript,
    /// Go grammar.
    Go,
}

impl LangId {
    /// Stable `SCREAMING_SNAKE_CASE` wire string.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Rust => "RUST",
            Self::Python => "PYTHON",
            Self::TypeScript => "TYPESCRIPT",
            Self::JavaScript => "JAVASCRIPT",
            Self::Go => "GO",
        }
    }

    /// Inverse of [`LangId::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "RUST" => Self::Rust,
            "PYTHON" => Self::Python,
            "TYPESCRIPT" => Self::TypeScript,
            "JAVASCRIPT" => Self::JavaScript,
            "GO" => Self::Go,
            _ => return None,
        };
        Some(v)
    }

    /// Canonical lowercase producer language code string.
    #[must_use]
    pub const fn as_language_code_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::Go => "go",
        }
    }

    /// Inverse of [`LangId::as_language_code_str`].
    #[must_use]
    pub fn from_language_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "rust" => Self::Rust,
            "python" => Self::Python,
            "typescript" => Self::TypeScript,
            "javascript" => Self::JavaScript,
            "go" => Self::Go,
            _ => return None,
        };
        Some(v)
    }

    /// All v1 supported languages, in declaration order.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Rust,
            Self::Python,
            Self::TypeScript,
            Self::JavaScript,
            Self::Go,
        ]
    }
}

impl fmt::Display for LangId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for LangId {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for LangId {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LangId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LangId SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LangId, E> {
                LangId::from_code_str(v).ok_or_else(|| E::unknown_variant(v, &["<LangId>"]))
            }
        }
        de.deserialize_str(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ByteSpan, LangId, MAX_DEPTH, MAX_METAVARS_PER_PATTERN, MAX_STRUCTURAL_NODES, MetaVar,
    };
    use crate::errors::StructuralErrorCode;

    #[test]
    fn caps_pinned() {
        assert_eq!(MAX_STRUCTURAL_NODES, 256);
        assert_eq!(MAX_DEPTH, 16);
        assert_eq!(MAX_METAVARS_PER_PATTERN, 32);
    }

    #[test]
    fn byte_span_rejects_inverted() {
        match ByteSpan::new(5, 3) {
            Ok(_) => assert!(false, "must reject inverted span"),
            Err(e) => assert_eq!(e.code, StructuralErrorCode::StrParseFail),
        }
    }

    #[test]
    fn byte_span_accepts_equal() {
        let Ok(s) = ByteSpan::new(5, 5) else {
            assert!(false, "must accept equal");
            return;
        };
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn byte_span_accessors() {
        let Ok(s) = ByteSpan::new(10, 25) else {
            assert!(false, "must accept ordered");
            return;
        };
        assert_eq!(s.start(), 10);
        assert_eq!(s.end(), 25);
        assert_eq!(s.len(), 15);
        assert!(!s.is_empty());
    }

    #[test]
    fn metavar_rejects_empty() {
        match MetaVar::new("") {
            Ok(_) => assert!(false, "must reject empty"),
            Err(e) => assert_eq!(e.code, StructuralErrorCode::StrInvalidMetavar),
        }
    }

    #[test]
    fn metavar_rejects_leading_digit() {
        match MetaVar::new("1ABC") {
            Ok(_) => assert!(false, "must reject leading digit"),
            Err(e) => assert_eq!(e.code, StructuralErrorCode::StrInvalidMetavar),
        }
    }

    #[test]
    fn metavar_rejects_dash() {
        match MetaVar::new("A-B") {
            Ok(_) => assert!(false, "must reject dash"),
            Err(e) => assert_eq!(e.code, StructuralErrorCode::StrInvalidMetavar),
        }
    }

    #[test]
    fn metavar_accepts_underscore_first() {
        let Ok(v) = MetaVar::new("_x1") else {
            assert!(false, "underscore first should be accepted");
            return;
        };
        assert_eq!(v.as_str(), "_x1");
    }

    #[test]
    fn metavar_accepts_alpha_only() {
        let Ok(v) = MetaVar::new("X") else {
            assert!(false, "single alpha should be accepted");
            return;
        };
        assert_eq!(v.as_str(), "X");
    }

    #[test]
    fn lang_id_code_str_roundtrip() {
        for l in LangId::all() {
            assert_eq!(LangId::from_code_str(l.as_code_str()), Some(*l));
        }
    }

    #[test]
    fn lang_id_language_code_roundtrip() {
        for l in LangId::all() {
            assert_eq!(LangId::from_language_code_str(l.as_language_code_str()), Some(*l));
        }
    }

    #[test]
    fn lang_id_unknown_is_none() {
        assert_eq!(LangId::from_code_str("CPP"), None);
    }

    #[test]
    fn lang_id_serde_roundtrip_via_ciborium() {
        for l in LangId::all() {
            let mut buf: Vec<u8> = Vec::new();
            if let Err(e) = ciborium::ser::into_writer(l, &mut buf) {
                assert!(false, "serialize {l:?}: {e}");
            }
            let got: Result<LangId, _> = ciborium::de::from_reader(buf.as_slice());
            match got {
                Ok(v) => assert_eq!(v, *l),
                Err(e) => assert!(false, "deserialize {l:?}: {e}"),
            }
        }
    }

    #[test]
    fn metavar_serde_roundtrip_via_ciborium() {
        let Ok(m) = MetaVar::new("X") else {
            assert!(false, "construct MetaVar");
            return;
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&m, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<MetaVar, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, m),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn byte_span_serde_roundtrip() {
        let Ok(s) = ByteSpan::new(3, 7) else {
            assert!(false, "construct ByteSpan");
            return;
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&s, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<ByteSpan, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, s),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn byte_span_serde_rejects_inverted() {
        // Manually craft a 2-tuple with inverted bounds: ciborium will
        // re-enter the visitor and the visitor must reject.
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&(9u32, 4u32), &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<ByteSpan, _> = ciborium::de::from_reader(buf.as_slice());
        assert!(got.is_err(), "deserialization must reject inverted span");
    }
}
