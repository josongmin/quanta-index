use core::fmt;

use crate::errors::{LexNormError, LexNormErrorCode};
use crate::lang::LangId;
use crate::nfc::normalize_nfc;
use crate::patterntype::PatternType;

/// Maximum supported chunk size, in bytes. Inputs above this fail with
/// [`LexNormErrorCode::OversizedChunk`].
pub const MAX_CHUNK_BYTES: usize = 64 * 1024;

/// One token emitted by [`tokenize_text`].
///
/// `surface` is the original byte slice copied verbatim. `lowered` is the
/// case-folded form the writer/reader compares against. `byte_start`/
/// `byte_end` index back into the raw input; they are equal for synthetic
/// identifier sub-parts that overlap their parent token's range.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Token {
    pub surface: Box<str>,
    pub lowered: Box<str>,
    pub kind: TokenKind,
    pub byte_start: u32,
    pub byte_end: u32,
}

/// Token kind. Wire form is `SCREAMING_SNAKE_CASE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Word,
    IdentifierPart,
    Number,
    Punct,
    Operator,
    Comment,
    String,
    RawString,
    Whitespace,
}

impl TokenKind {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Word => "WORD",
            Self::IdentifierPart => "IDENTIFIER_PART",
            Self::Number => "NUMBER",
            Self::Punct => "PUNCT",
            Self::Operator => "OPERATOR",
            Self::Comment => "COMMENT",
            Self::String => "STRING",
            Self::RawString => "RAW_STRING",
            Self::Whitespace => "WHITESPACE",
        }
    }

    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "WORD" => Self::Word,
            "IDENTIFIER_PART" => Self::IdentifierPart,
            "NUMBER" => Self::Number,
            "PUNCT" => Self::Punct,
            "OPERATOR" => Self::Operator,
            "COMMENT" => Self::Comment,
            "STRING" => Self::String,
            "RAW_STRING" => Self::RawString,
            "WHITESPACE" => Self::Whitespace,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for TokenKind {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for TokenKind {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = TokenKind;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("TokenKind SCREAMING_SNAKE_CASE string")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<TokenKind, E> {
                TokenKind::from_code_str(v).ok_or_else(|| E::unknown_variant(v, &["<TokenKind>"]))
            }
        }
        de.deserialize_str(V)
    }
}

impl serde::Serialize for Token {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = ser.serialize_struct("Token", 5)?;
        st.serialize_field("surface", &*self.surface)?;
        st.serialize_field("lowered", &*self.lowered)?;
        st.serialize_field("kind", &self.kind)?;
        st.serialize_field("byte_start", &self.byte_start)?;
        st.serialize_field("byte_end", &self.byte_end)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for Token {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Token;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("Token struct with surface/lowered/kind/byte_start/byte_end")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Token, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut surface: Option<String> = None;
                let mut lowered: Option<String> = None;
                let mut kind: Option<TokenKind> = None;
                let mut byte_start: Option<u32> = None;
                let mut byte_end: Option<u32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "surface" => surface = Some(map.next_value()?),
                        "lowered" => lowered = Some(map.next_value()?),
                        "kind" => kind = Some(map.next_value()?),
                        "byte_start" => byte_start = Some(map.next_value()?),
                        "byte_end" => byte_end = Some(map.next_value()?),
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["surface", "lowered", "kind", "byte_start", "byte_end"],
                            ));
                        }
                    }
                }
                Ok(Token {
                    surface: surface
                        .ok_or_else(|| serde::de::Error::missing_field("surface"))?
                        .into_boxed_str(),
                    lowered: lowered
                        .ok_or_else(|| serde::de::Error::missing_field("lowered"))?
                        .into_boxed_str(),
                    kind: kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?,
                    byte_start: byte_start
                        .ok_or_else(|| serde::de::Error::missing_field("byte_start"))?,
                    byte_end: byte_end
                        .ok_or_else(|| serde::de::Error::missing_field("byte_end"))?,
                })
            }
        }
        de.deserialize_map(V)
    }
}

/// Tokenize `input` per `lang` and `pt`. See module-level docs for the
/// per-`PatternType` branching contract.
pub fn tokenize_text(
    input: &str,
    lang: LangId,
    pt: PatternType,
) -> Result<Vec<Token>, LexNormError> {
    if input.len() > MAX_CHUNK_BYTES {
        return Err(LexNormError::new(
            LexNormErrorCode::OversizedChunk,
            u32_from_usize_clamped(MAX_CHUNK_BYTES),
            "input exceeds 64 KiB cap",
        ));
    }
    if !lang.is_supported() {
        return Err(LexNormError::new(
            LexNormErrorCode::NormalizerUnknownLang,
            0,
            "lang not in v1 ship set",
        ));
    }
    let normalized: String;
    let work: &str = if input.is_ascii() {
        input
    } else {
        normalized = normalize_nfc(input);
        &normalized
    };
    if work.len() > MAX_CHUNK_BYTES {
        return Err(LexNormError::new(
            LexNormErrorCode::OversizedChunk,
            u32_from_usize_clamped(MAX_CHUNK_BYTES),
            "NFC-normalized input exceeds 64 KiB cap",
        ));
    }
    match pt {
        PatternType::Literal => tokenize_literal(work),
        PatternType::Keyword | PatternType::Standard => tokenize_identifier_aware(work),
        PatternType::Regexp => Err(LexNormError::new(
            LexNormErrorCode::UnknownPatternType,
            0,
            "regexp tokenization deferred to LEX-04",
        )),
        PatternType::Structural => Err(LexNormError::new(
            LexNormErrorCode::UnknownPatternType,
            0,
            "structural tokenization deferred to STR-01",
        )),
    }
}

fn tokenize_literal(input: &str) -> Result<Vec<Token>, LexNormError> {
    let mut out: Vec<Token> = Vec::new();
    let bytes = input.as_bytes();
    let mut i: usize = 0;
    while i < bytes.len() {
        let Some(b) = bytes.get(i) else {
            break;
        };
        if is_ascii_ws(*b) {
            i = i.saturating_add(1);
            continue;
        }
        let start = i;
        while let Some(b2) = bytes.get(i) {
            if is_ascii_ws(*b2) {
                break;
            }
            i = i.saturating_add(1);
        }
        let end = i;
        let slice = input
            .get(start..end)
            .ok_or_else(|| invalid_slice_err(start))?;
        out.push(Token {
            surface: slice.to_owned().into_boxed_str(),
            lowered: slice.to_ascii_lowercase().into_boxed_str(),
            kind: TokenKind::Word,
            byte_start: u32_from_usize_clamped(start),
            byte_end: u32_from_usize_clamped(end),
        });
    }
    Ok(out)
}

fn tokenize_identifier_aware(input: &str) -> Result<Vec<Token>, LexNormError> {
    let mut out: Vec<Token> = Vec::new();
    let bytes = input.as_bytes();
    let mut i: usize = 0;
    while i < bytes.len() {
        let Some(b) = bytes.get(i) else {
            break;
        };
        if is_ascii_ws(*b) {
            i = i.saturating_add(1);
            continue;
        }
        let start = i;
        let is_word = is_word_byte(*b);
        if is_word {
            while let Some(b2) = bytes.get(i) {
                if !is_word_byte(*b2) {
                    break;
                }
                i = i.saturating_add(1);
            }
        } else {
            i = i.saturating_add(1);
        }
        let end = i;
        let slice = input
            .get(start..end)
            .ok_or_else(|| invalid_slice_err(start))?;
        if is_word {
            push_word_and_parts(&mut out, slice, start, end);
        } else {
            out.push(Token {
                surface: slice.to_owned().into_boxed_str(),
                lowered: slice.to_ascii_lowercase().into_boxed_str(),
                kind: TokenKind::Punct,
                byte_start: u32_from_usize_clamped(start),
                byte_end: u32_from_usize_clamped(end),
            });
        }
    }
    Ok(out)
}

fn push_word_and_parts(out: &mut Vec<Token>, slice: &str, start: usize, end: usize) {
    let kind = if slice.bytes().all(|b| b.is_ascii_digit()) {
        TokenKind::Number
    } else {
        TokenKind::Word
    };
    out.push(Token {
        surface: slice.to_owned().into_boxed_str(),
        lowered: slice.to_ascii_lowercase().into_boxed_str(),
        kind,
        byte_start: u32_from_usize_clamped(start),
        byte_end: u32_from_usize_clamped(end),
    });
    let parts = split_identifier(slice);
    if parts.len() <= 1 {
        return;
    }
    let mut local_off: usize = 0;
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let plen = part.len();
        let pstart = start.saturating_add(local_off);
        let pend = pstart.saturating_add(plen);
        local_off = local_off.saturating_add(plen);
        if pstart == start && pend == end {
            continue;
        }
        let pkind = if part.bytes().all(|b| b.is_ascii_digit()) {
            TokenKind::Number
        } else {
            TokenKind::IdentifierPart
        };
        out.push(Token {
            surface: part.to_owned().into_boxed_str(),
            lowered: part.to_ascii_lowercase().into_boxed_str(),
            kind: pkind,
            byte_start: u32_from_usize_clamped(pstart),
            byte_end: u32_from_usize_clamped(pend),
        });
    }
}

fn split_identifier(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut parts: Vec<&str> = Vec::new();
    let mut start: usize = 0;
    let mut i: usize = 0;
    while i < bytes.len() {
        let Some(b) = bytes.get(i) else {
            break;
        };
        if *b == b'_' || *b == b'-' {
            if start < i
                && let Some(p) = s.get(start..i)
            {
                parts.push(p);
            }
            i = i.saturating_add(1);
            start = i;
            continue;
        }
        let is_upper = b.is_ascii_uppercase();
        let is_digit = b.is_ascii_digit();
        let prev = if i == 0 {
            None
        } else {
            bytes.get(i.saturating_sub(1))
        };
        let next = bytes.get(i.saturating_add(1));
        let cut_here = prev.is_some_and(|pb| {
            let prev_lower = pb.is_ascii_lowercase();
            let prev_upper = pb.is_ascii_uppercase();
            let prev_digit = pb.is_ascii_digit();
            let lower_to_upper = prev_lower && is_upper;
            let acronym_break =
                prev_upper && is_upper && matches!(next, Some(nb) if nb.is_ascii_lowercase());
            let digit_boundary = (prev_digit && !is_digit) || (!prev_digit && is_digit);
            lower_to_upper || acronym_break || digit_boundary
        });
        if cut_here
            && start < i
            && let Some(p) = s.get(start..i)
        {
            parts.push(p);
            start = i;
        }
        i = i.saturating_add(1);
    }
    if start < bytes.len()
        && let Some(p) = s.get(start..bytes.len())
    {
        parts.push(p);
    }
    parts
}

const fn is_ascii_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

const fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || (b >= 0x80)
}

fn invalid_slice_err(at: usize) -> LexNormError {
    LexNormError::new(
        LexNormErrorCode::InvalidUtf8,
        u32_from_usize_clamped(at),
        "slice does not align to a UTF-8 boundary",
    )
}

fn u32_from_usize_clamped(v: usize) -> u32 {
    u32::try_from(v).map_or(u32::MAX, |n| n)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_CHUNK_BYTES, Token, TokenKind, split_identifier, tokenize_literal, tokenize_text,
    };
    use crate::errors::{LexNormError, LexNormErrorCode};
    use crate::lang::LangId;
    use crate::patterntype::PatternType;

    fn must_ok(label: &str, r: Result<Vec<Token>, LexNormError>) -> Vec<Token> {
        match r {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{label}: expected Ok, got Err({e})");
                Vec::new()
            }
        }
    }

    fn must_err(label: &str, r: Result<Vec<Token>, LexNormError>) -> LexNormErrorCode {
        match r {
            Ok(v) => {
                assert!(false, "{label}: expected Err, got {} tokens", v.len());
                LexNormErrorCode::EmptyTokenStream
            }
            Err(e) => e.code,
        }
    }

    #[test]
    fn empty_input_yields_empty_token_stream() {
        let v = must_ok(
            "empty",
            tokenize_text("", LangId::Rust, PatternType::Standard),
        );
        assert!(v.is_empty());
    }

    #[test]
    fn literal_does_not_split_camel_case() {
        let toks = must_ok(
            "literal-camel",
            tokenize_text("getUserName", LangId::Rust, PatternType::Literal),
        );
        assert_eq!(toks.len(), 1);
        let Some(t) = toks.first() else {
            assert!(false, "no token");
            return;
        };
        assert_eq!(&*t.surface, "getUserName");
        assert_eq!(&*t.lowered, "getusername");
        assert_eq!(t.kind, TokenKind::Word);
    }

    #[test]
    fn literal_splits_only_on_whitespace() {
        let toks = match tokenize_literal("foo bar  baz") {
            Ok(v) => v,
            Err(e) => {
                assert!(false, "{e}");
                return;
            }
        };
        assert_eq!(toks.len(), 3);
        let surfaces: Vec<&str> = toks.iter().map(|t| &*t.surface).collect();
        assert_eq!(surfaces, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn standard_splits_camel_case() {
        let toks = must_ok(
            "standard-camel",
            tokenize_text("getUserName", LangId::Rust, PatternType::Standard),
        );
        let surfaces: Vec<&str> = toks.iter().map(|t| &*t.surface).collect();
        assert!(surfaces.contains(&"getUserName"));
        assert!(surfaces.contains(&"get"));
        assert!(surfaces.contains(&"User"));
        assert!(surfaces.contains(&"Name"));
    }

    #[test]
    fn standard_splits_snake_case() {
        let toks = must_ok(
            "standard-snake",
            tokenize_text("parse_query_v2", LangId::Rust, PatternType::Standard),
        );
        let surfaces: Vec<&str> = toks.iter().map(|t| &*t.surface).collect();
        assert!(surfaces.contains(&"parse_query_v2"));
        assert!(surfaces.contains(&"parse"));
        assert!(surfaces.contains(&"query"));
        assert!(surfaces.contains(&"v"));
        assert!(surfaces.contains(&"2"));
    }

    #[test]
    fn standard_splits_acronym_then_word() {
        let parts = split_identifier("HTTPServer");
        assert_eq!(parts, vec!["HTTP", "Server"]);
    }

    #[test]
    fn standard_splits_digit_boundaries() {
        let parts = split_identifier("foo123bar");
        assert_eq!(parts, vec!["foo", "123", "bar"]);
    }

    #[test]
    fn standard_splits_kebab_case() {
        let parts = split_identifier("foo-bar-baz");
        assert_eq!(parts, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn regexp_pattern_type_returns_typed_error() {
        let code = must_err(
            "regexp",
            tokenize_text("foo", LangId::Rust, PatternType::Regexp),
        );
        assert_eq!(code, LexNormErrorCode::UnknownPatternType);
    }

    #[test]
    fn structural_pattern_type_returns_typed_error() {
        let code = must_err(
            "structural",
            tokenize_text("foo", LangId::Rust, PatternType::Structural),
        );
        assert_eq!(code, LexNormErrorCode::UnknownPatternType);
    }

    #[test]
    fn unknown_lang_returns_typed_error() {
        let code = must_err(
            "unknown-lang",
            tokenize_text("foo", LangId::Unknown, PatternType::Standard),
        );
        assert_eq!(code, LexNormErrorCode::NormalizerUnknownLang);
    }

    #[test]
    fn oversized_chunk_returns_typed_error() {
        let big = "a".repeat(MAX_CHUNK_BYTES.saturating_add(1));
        let code = must_err(
            "oversized",
            tokenize_text(&big, LangId::Rust, PatternType::Standard),
        );
        assert_eq!(code, LexNormErrorCode::OversizedChunk);
    }

    #[test]
    fn byte_offsets_index_into_input() {
        let toks = must_ok(
            "hello-world",
            tokenize_text("hello world", LangId::Rust, PatternType::Standard),
        );
        let Some(first) = toks.first() else {
            assert!(false, "no token");
            return;
        };
        assert_eq!(first.byte_start, 0);
        assert_eq!(first.byte_end, 5);
    }

    #[test]
    fn single_identifier_emits_no_extra_parts() {
        let toks = must_ok(
            "single-foo",
            tokenize_text("foo", LangId::Rust, PatternType::Standard),
        );
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn ascii_fast_path_offsets_match_raw_input() {
        let raw = "hello world";
        let toks = must_ok(
            "ascii-fast-path",
            tokenize_text(raw, LangId::Rust, PatternType::Standard),
        );
        assert_eq!(toks.len(), 2);
        let Some(t0) = toks.first() else {
            assert!(false, "no first token");
            return;
        };
        assert_eq!(&*t0.surface, "hello");
        assert_eq!(t0.byte_start, 0);
        assert_eq!(t0.byte_end, 5);
    }

    #[test]
    fn empty_input_after_nfc_path() {
        let toks = must_ok(
            "empty-literal",
            tokenize_text("", LangId::Rust, PatternType::Literal),
        );
        assert!(toks.is_empty());
    }

    #[test]
    fn nfc_equivalence_precomposed_vs_decomposed() {
        let precomposed = "caf\u{00E9}";
        let decomposed = "caf\u{0065}\u{0301}";
        let a = must_ok(
            "nfc-precomposed",
            tokenize_text(precomposed, LangId::Rust, PatternType::Standard),
        );
        let b = must_ok(
            "nfc-decomposed",
            tokenize_text(decomposed, LangId::Rust, PatternType::Standard),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn token_offsets_index_into_normalized_form() {
        let decomposed = "caf\u{0065}\u{0301}";
        assert_eq!(decomposed.len(), 6);
        let toks = must_ok(
            "nfc-offsets",
            tokenize_text(decomposed, LangId::Rust, PatternType::Standard),
        );
        assert_eq!(toks.len(), 1);
        let Some(t0) = toks.first() else {
            assert!(false, "no token");
            return;
        };
        assert_eq!(t0.byte_start, 0);
        assert_eq!(t0.byte_end, 5);
    }
}
