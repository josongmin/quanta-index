//! Stable types shared between extractor, registry, and index.
//!
//! [`DocId`] mirrors the LEX-02/03 newtype shape so sibling indices can
//! join on the same id without re-implementing a parallel type.
//! [`ByteSpan`] holds local byte positions only; cross-file resolution
//! belongs to SEM-01 per the spec's reference/def boundary lock.
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::{SymbolError, SymbolErrorCode};
use crate::symbol_kind::SymbolKind;

/// Stable document identifier. Newtype around `u64`; equality and ordering
/// match the wrapped value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub u64);

impl DocId {
    /// Construct from a raw `u64`.
    #[must_use]
    pub const fn new(v: u64) -> Self {
        Self(v)
    }

    /// Borrow the wrapped `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for DocId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<DocId> for u64 {
    fn from(d: DocId) -> Self {
        d.0
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for DocId {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for DocId {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = DocId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("DocId u64")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<DocId, E> {
                Ok(DocId(v))
            }
            fn visit_u32<E: serde::de::Error>(self, v: u32) -> Result<DocId, E> {
                Ok(DocId(u64::from(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<DocId, E> {
                if v < 0 {
                    return Err(E::custom("DocId must be non-negative"));
                }
                let u = u64::try_from(v)
                    .map_err(|err| E::custom(format!("DocId out of u64 range: {err}")))?;
                Ok(DocId(u))
            }
        }
        de.deserialize_u64(V)
    }
}

/// Local byte span within a single source document.
///
/// `start <= end` is an invariant enforced at construction; the typed
/// constructor [`ByteSpan::new`] is the only way to obtain a `ByteSpan`
/// with non-trivial values. Bounds are `u32` per the LEX-05 spec — files
/// larger than 4 GiB are out of scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteSpan {
    start: u32,
    end: u32,
}

impl ByteSpan {
    /// Construct a `ByteSpan` with `start <= end`. Rejects inverted spans
    /// with [`SymbolErrorCode::InvalidDocument`].
    pub fn new(start: u32, end: u32) -> Result<Self, SymbolError> {
        if start > end {
            return Err(SymbolError::new(
                SymbolErrorCode::InvalidDocument,
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

/// Per-language identifier for the v1 ship-set. Wire form is
/// `SCREAMING_SNAKE_CASE`.
///
/// Unknown extensions return `None` from [`LangId::from_extension`];
/// unsupported languages at extractor time surface
/// `SymbolError::lang_unsupported` per the LEX-05 spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LangId {
    Rust,
    Python,
    TypeScript,
    JavaScript,
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

    /// Resolve a `LangId` from a path's file extension (suffix after the
    /// last `.`, case-insensitive). Unknown extensions return `None`;
    /// callers decide whether `None` is an error or a silent skip.
    #[must_use]
    pub fn from_extension(path: &str) -> Option<Self> {
        let dot = path.rfind('.')?;
        let start = dot.checked_add(1)?;
        let ext = path.get(start..)?;
        let lower = ext.to_ascii_lowercase();
        let v = match lower.as_str() {
            "rs" => Self::Rust,
            "py" | "pyi" => Self::Python,
            "ts" | "tsx" => Self::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "go" => Self::Go,
            _ => return None,
        };
        Some(v)
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

/// A single symbol record emitted by an extractor.
///
/// `parent` is `Some(container_name)` for nested symbols (e.g. a Python
/// method inside a class); flat-scope symbols carry `None`. Matches the
/// LEX-05 spec §3 schema with flattened container per Q-LEX05-3.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Symbol {
    pub name: Box<str>,
    pub kind: SymbolKind,
    pub doc_id: DocId,
    pub span: ByteSpan,
    pub lang: LangId,
    pub parent: Option<Box<str>>,
}

impl Symbol {
    /// Construct a new `Symbol`. All fields are owned; `parent` is `None`
    /// for flat-scope symbols.
    #[must_use]
    pub fn new(
        name: impl Into<Box<str>>,
        kind: SymbolKind,
        doc_id: DocId,
        span: ByteSpan,
        lang: LangId,
        parent: Option<Box<str>>,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            doc_id,
            span,
            lang,
            parent,
        }
    }
}

impl serde::Serialize for Symbol {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let n = if self.parent.is_some() { 6 } else { 5 };
        let mut m = ser.serialize_map(Some(n))?;
        m.serialize_entry("name", self.name.as_ref())?;
        m.serialize_entry("kind", &self.kind)?;
        m.serialize_entry("doc_id", &self.doc_id)?;
        m.serialize_entry("span", &self.span)?;
        m.serialize_entry("lang", &self.lang)?;
        if let Some(p) = self.parent.as_ref() {
            m.serialize_entry("parent", p.as_ref())?;
        }
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for Symbol {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = Symbol;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Symbol map (name, kind, doc_id, span, lang, parent?)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<Symbol, M::Error> {
                let mut name: Option<String> = None;
                let mut kind: Option<SymbolKind> = None;
                let mut doc_id: Option<DocId> = None;
                let mut span: Option<ByteSpan> = None;
                let mut lang: Option<LangId> = None;
                let mut parent: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "name" => {
                            if name.is_some() {
                                return Err(serde::de::Error::duplicate_field("name"));
                            }
                            name = Some(map.next_value()?);
                        }
                        "kind" => {
                            if kind.is_some() {
                                return Err(serde::de::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value()?);
                        }
                        "doc_id" => {
                            if doc_id.is_some() {
                                return Err(serde::de::Error::duplicate_field("doc_id"));
                            }
                            doc_id = Some(map.next_value()?);
                        }
                        "span" => {
                            if span.is_some() {
                                return Err(serde::de::Error::duplicate_field("span"));
                            }
                            span = Some(map.next_value()?);
                        }
                        "lang" => {
                            if lang.is_some() {
                                return Err(serde::de::Error::duplicate_field("lang"));
                            }
                            lang = Some(map.next_value()?);
                        }
                        "parent" => {
                            if parent.is_some() {
                                return Err(serde::de::Error::duplicate_field("parent"));
                            }
                            parent = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["name", "kind", "doc_id", "span", "lang", "parent"],
                            ));
                        }
                    }
                }
                let name = name.ok_or_else(|| serde::de::Error::missing_field("name"))?;
                let kind = kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?;
                let doc_id = doc_id.ok_or_else(|| serde::de::Error::missing_field("doc_id"))?;
                let span = span.ok_or_else(|| serde::de::Error::missing_field("span"))?;
                let lang = lang.ok_or_else(|| serde::de::Error::missing_field("lang"))?;
                Ok(Symbol {
                    name: name.into_boxed_str(),
                    kind,
                    doc_id,
                    span,
                    lang,
                    parent: parent.map(String::into_boxed_str),
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{ByteSpan, DocId, LangId, Symbol};
    use crate::errors::SymbolErrorCode;
    use crate::symbol_kind::SymbolKind;

    #[test]
    fn docid_roundtrip_u64() {
        let d = DocId::from(42u64);
        assert_eq!(d.get(), 42);
        let v: u64 = d.into();
        assert_eq!(v, 42);
    }

    #[test]
    fn byte_span_constructor_rejects_inverted() {
        match ByteSpan::new(5, 3) {
            Ok(_) => assert!(false, "must reject inverted span"),
            Err(e) => assert_eq!(e.code, SymbolErrorCode::InvalidDocument),
        }
    }

    #[test]
    fn byte_span_constructor_accepts_equal() {
        let s = match ByteSpan::new(5, 5) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn byte_span_len_and_accessors() {
        let s = match ByteSpan::new(10, 25) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(s.start(), 10);
        assert_eq!(s.end(), 25);
        assert_eq!(s.len(), 15);
        assert!(!s.is_empty());
    }

    #[test]
    fn lang_id_from_extension_rust() {
        assert_eq!(LangId::from_extension("src/lib.rs"), Some(LangId::Rust));
    }

    #[test]
    fn lang_id_from_extension_python() {
        assert_eq!(LangId::from_extension("a/b.py"), Some(LangId::Python));
        assert_eq!(LangId::from_extension("a/b.pyi"), Some(LangId::Python));
    }

    #[test]
    fn lang_id_from_extension_typescript_and_tsx() {
        assert_eq!(LangId::from_extension("a/b.ts"), Some(LangId::TypeScript));
        assert_eq!(LangId::from_extension("a/b.tsx"), Some(LangId::TypeScript));
    }

    #[test]
    fn lang_id_from_extension_javascript_family() {
        assert_eq!(LangId::from_extension("a.js"), Some(LangId::JavaScript));
        assert_eq!(LangId::from_extension("a.mjs"), Some(LangId::JavaScript));
        assert_eq!(LangId::from_extension("a.cjs"), Some(LangId::JavaScript));
        assert_eq!(LangId::from_extension("a.jsx"), Some(LangId::JavaScript));
    }

    #[test]
    fn lang_id_from_extension_go() {
        assert_eq!(LangId::from_extension("main.go"), Some(LangId::Go));
    }

    #[test]
    fn lang_id_unknown_extension_yields_none() {
        assert_eq!(LangId::from_extension("README.md"), None);
        assert_eq!(LangId::from_extension("no-ext"), None);
        assert_eq!(LangId::from_extension(""), None);
    }

    #[test]
    fn lang_id_uppercase_extension_still_matches() {
        assert_eq!(LangId::from_extension("a/B.RS"), Some(LangId::Rust));
    }

    #[test]
    fn lang_id_code_str_roundtrip() {
        for l in LangId::all() {
            assert_eq!(LangId::from_code_str(l.as_code_str()), Some(*l));
        }
    }

    #[test]
    fn symbol_serde_roundtrip_via_ciborium() {
        let span = match ByteSpan::new(0, 10) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let sym = Symbol::new(
            "foo",
            SymbolKind::Function,
            DocId(1),
            span,
            LangId::Rust,
            None,
        );
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&sym, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<Symbol, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, sym),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn symbol_with_parent_roundtrip() {
        let span = match ByteSpan::new(10, 20) {
            Ok(s) => s,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        let sym = Symbol::new(
            "bar",
            SymbolKind::Method,
            DocId(2),
            span,
            LangId::Python,
            Some("Foo".into()),
        );
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&sym, &mut buf) {
            assert!(false, "{e}");
        }
        let got: Result<Symbol, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, sym),
            Err(e) => assert!(false, "{e}"),
        }
    }
}
