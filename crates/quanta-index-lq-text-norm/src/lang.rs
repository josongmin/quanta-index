//! V1 ship-language set for the lexical normalizer.
//!
//! Per `feature-scope.md` §1.3.4 the Wave-1 ship set is Rust, Python,
//! TypeScript, JavaScript, Go. Anything else resolves to [`LangId::Unknown`]
//! at detection time; callers that require a known lang surface
//! [`crate::LexNormErrorCode::NormalizerUnknownLang`].
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Ship-set language identifier plus `Unknown` fallthrough.
///
/// Wire form is `SCREAMING_SNAKE_CASE` (`RUST`, `PYTHON`, ...). `Unknown` is
/// the only non-ship variant and is the sentinel for "detection ran but did
/// not match the v1 set"; it never silently re-maps to another lang.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LangId {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Unknown,
}

impl LangId {
    /// Stable wire string. `as_code_str(Unknown) == "UNKNOWN"`.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Rust => "RUST",
            Self::Python => "PYTHON",
            Self::TypeScript => "TYPESCRIPT",
            Self::JavaScript => "JAVASCRIPT",
            Self::Go => "GO",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// Inverse of [`LangId::as_code_str`]. Returns `None` for unknown.
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "RUST" => Self::Rust,
            "PYTHON" => Self::Python,
            "TYPESCRIPT" => Self::TypeScript,
            "JAVASCRIPT" => Self::JavaScript,
            "GO" => Self::Go,
            "UNKNOWN" => Self::Unknown,
            _ => return None,
        };
        Some(v)
    }

    /// `true` if this is one of the v1 ship-set entries (i.e. not `Unknown`).
    #[must_use]
    pub const fn is_supported(self) -> bool {
        !matches!(self, Self::Unknown)
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

/// Detect a [`LangId`] from a file path with optional content sniff.
///
/// Extension-first matching is exact: the lowercased suffix after the last
/// `.` must equal a known mapping. If no match is found and `contents_sniff`
/// is `Some`, a minimal shebang sniff applies (`#!/usr/bin/env python` →
/// Python, etc). Anything else returns [`LangId::Unknown`]; callers that
/// require a supported lang must surface
/// [`crate::LexNormErrorCode::NormalizerUnknownLang`] explicitly.
#[must_use]
pub fn detect_lang(path: &str, contents_sniff: Option<&[u8]>) -> LangId {
    if let Some(by_ext) = detect_by_extension(path) {
        return by_ext;
    }
    if let Some(bytes) = contents_sniff
        && let Some(by_sniff) = detect_by_sniff(bytes)
    {
        return by_sniff;
    }
    LangId::Unknown
}

fn detect_by_extension(path: &str) -> Option<LangId> {
    let dot = path.rfind('.')?;
    let ext_start = dot.checked_add(1)?;
    let ext = path.get(ext_start..)?;
    let lower = ext.to_ascii_lowercase();
    let v = match lower.as_str() {
        "rs" => LangId::Rust,
        "py" | "pyi" => LangId::Python,
        "ts" | "tsx" => LangId::TypeScript,
        "js" | "jsx" | "mjs" | "cjs" => LangId::JavaScript,
        "go" => LangId::Go,
        _ => return None,
    };
    Some(v)
}

fn detect_by_sniff(bytes: &[u8]) -> Option<LangId> {
    let head = bytes.get(..bytes.len().min(128))?;
    if !head.starts_with(b"#!") {
        return None;
    }
    if contains_subslice(head, b"python") {
        return Some(LangId::Python);
    }
    if contains_subslice(head, b"node") {
        return Some(LangId::JavaScript);
    }
    None
}

fn contains_subslice(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    let bound = hay.len().saturating_sub(needle.len());
    for i in 0..=bound {
        let Some(slice) = hay.get(i..i.saturating_add(needle.len())) else {
            continue;
        };
        if slice == needle {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{LangId, detect_lang};

    const ALL_LANGS: &[LangId] = &[
        LangId::Rust,
        LangId::Python,
        LangId::TypeScript,
        LangId::JavaScript,
        LangId::Go,
        LangId::Unknown,
    ];

    #[test]
    fn code_strs_are_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for l in ALL_LANGS {
            let s = l.as_code_str();
            assert!(!seen.contains(&s), "duplicate code str: {s}");
            seen.push(s);
        }
        assert_eq!(seen.len(), ALL_LANGS.len());
    }

    #[test]
    fn code_strs_roundtrip() {
        for l in ALL_LANGS {
            let s = l.as_code_str();
            assert_eq!(LangId::from_code_str(s), Some(*l));
        }
    }

    #[test]
    fn rust_extension_detects_rust() {
        assert_eq!(detect_lang("src/lib.rs", None), LangId::Rust);
    }

    #[test]
    fn python_extension_detects_python() {
        assert_eq!(detect_lang("a/b.py", None), LangId::Python);
        assert_eq!(detect_lang("a/b.pyi", None), LangId::Python);
    }

    #[test]
    fn typescript_extension_detects_typescript() {
        assert_eq!(detect_lang("a/b.ts", None), LangId::TypeScript);
        assert_eq!(detect_lang("a/b.tsx", None), LangId::TypeScript);
    }

    #[test]
    fn javascript_extension_detects_javascript() {
        assert_eq!(detect_lang("a/b.js", None), LangId::JavaScript);
        assert_eq!(detect_lang("a/b.mjs", None), LangId::JavaScript);
    }

    #[test]
    fn go_extension_detects_go() {
        assert_eq!(detect_lang("main.go", None), LangId::Go);
    }

    #[test]
    fn unknown_extension_yields_unknown() {
        assert_eq!(detect_lang("README.md", None), LangId::Unknown);
        assert_eq!(detect_lang("no-ext", None), LangId::Unknown);
    }

    #[test]
    fn python_shebang_sniff_detects_python() {
        let head = b"#!/usr/bin/env python3\nprint(1)\n";
        assert_eq!(detect_lang("script", Some(head)), LangId::Python);
    }

    #[test]
    fn node_shebang_sniff_detects_javascript() {
        let head = b"#!/usr/bin/env node\nconsole.log(1);\n";
        assert_eq!(detect_lang("script", Some(head)), LangId::JavaScript);
    }

    #[test]
    fn is_supported_excludes_unknown() {
        assert!(LangId::Rust.is_supported());
        assert!(LangId::Python.is_supported());
        assert!(!LangId::Unknown.is_supported());
    }
}
