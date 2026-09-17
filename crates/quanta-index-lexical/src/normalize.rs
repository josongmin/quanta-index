//! The one text normalization contract of the lexical adapter (QI-BB-011).
//!
//! Every surface that turns text into something comparable — the inverted
//! index analyzer, the phrase position sidecars, the trigram sidecars, the
//! raw-substring and regex verifiers, and the `index:no` manual scan — goes
//! through this module, at build time and at query time. There is no second
//! tokenizer, no second case fold, and no second normalization form anywhere
//! in the crate, so the same DSL query answers identically whichever leaf kind
//! or route executes it.
//!
//! # Contract
//!
//! **Normalization form.** Text is normalized to Unicode NFC before it is
//! indexed and before a query literal is lowered: composed and decomposed
//! spellings of one grapheme (`é` vs `e` + U+0301) are the same text, and
//! canonical singletons collapse (U+212A KELVIN SIGN becomes `K`). NFC only —
//! no compatibility folding, so full-width `ｆｏｏ` stays distinct from `foo`
//! and ligatures are not expanded.
//!
//! **Case folding.** `case:no` compares text after per-character
//! [`char::to_lowercase`] — the full Unicode lowercase mapping, not ASCII.
//! `CAFÉ` and `café` are one token; `Данные` and `данные` are one token. The
//! mapping is context-free and locale-free: a final `Σ` folds to `σ` (never
//! `ς`), `İ` (U+0130) folds to `i` + U+0307 rather than to plain `i`, and `ß`
//! is already lowercase and is never expanded to `ss`. `case:yes` applies no
//! fold at all.
//!
//! **Token boundaries.** A token is a maximal run of characters that are
//! [`char::is_alphanumeric`], `_`, or a combining mark. Everything else —
//! whitespace, punctuation, symbols, emoji — is a boundary and is never part
//! of a token. Hence `foo_bar` is one token, `fooBar` is one token
//! (`foobar` under `case:no`), `foo.bar`, `foo-bar` and `foo bar` are all the
//! sequence `[foo, bar]`, and `ok👍done` is `[ok, done]`. A contiguous CJK run
//! is one token: there is no dictionary segmentation. Combining marks (general
//! category `M`) never break a token, so `नमस्ते` stays whole across its
//! virama. Every run receives a position; runs longer than
//! [`MAX_TOKEN_BYTES`] (measured on the NFC text) keep their position but are
//! never emitted as index terms, and a query literal containing such a run is
//! refused typed rather than answered empty.
//!
//! **Byte semantics vs text semantics.** Keyword and phrase leaves have text
//! semantics: they are tokenized as above and match whole tokens. Raw-string
//! (`'...'`) and regex (`/.../`) leaves have substring semantics over the NFC
//! text of the document: no tokenization, punctuation and whitespace are
//! literal. A raw-string literal is NFC-normalized like any query text, and
//! under `case:no` it is compared after the same fold as the tokenizer against
//! a folded copy of the document. A regex source is NFC-normalized as text
//! (so write `\x{301}` rather than a literal combining mark when the pattern
//! must name a decomposed sequence, which can never match anyway because the
//! document is NFC) and runs with the regex engine's own `(?i)` under
//! `case:no`; its trigram prefilter folds the extracted literal alternatives
//! with [`fold`], which is sound because every alternative is a character-
//! boundary prefix of some match and the fold is per character.
//!
//! **Versioning.** [`TEXT_NORMALIZER_VERSION`] names this contract. Sealed
//! generations record it; a generation built under any other version is
//! refused typed at open rather than served with mismatched semantics. Any
//! change to the rules above — including a Unicode table update that changes
//! a mapping this crate relies on — is a version bump.
//!
//! # Known limits
//!
//! - No Turkic dotted/dotless-I handling, no final-sigma rule, no `ß`→`ss`.
//! - No diacritic stripping (`cafe` does not match `café`) and no width
//!   folding.
//! - No CJK/Thai word segmentation and no `camelCase` / `snake_case`
//!   sub-token expansion.
//! - NFC uses `unicode-normalization`'s tables; case mapping and the
//!   alphanumeric predicate use the Rust standard library's. The two are
//!   pinned by the toolchain and crate versions, not by this contract.

use core::fmt;
use std::borrow::Cow;

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::{IsNormalized, UnicodeNormalization, is_nfc_quick};

/// Version of the contract in this module's documentation.
///
/// Recorded in every sealed generation and compared at open. Version 1 was
/// the pre-QI-BB-011 state (Tantivy's default analyzer for the index,
/// whitespace + ASCII folding for the sidecars); it is not readable.
pub(crate) const TEXT_NORMALIZER_VERSION: TextNormalizerVersion =
    TextNormalizerVersion { major: 2, minor: 0 };

/// Longest run, in bytes of NFC text, that is emitted as a token.
///
/// Longer runs (minified blobs, base64, hashes past sha-512 hex) keep their
/// position so phrase adjacency stays exact, but they never enter the term
/// dictionary or the position sidecars. The raw-string and regex surfaces
/// still see them.
pub(crate) const MAX_TOKEN_BYTES: usize = 256;

/// `(major, minor)` of the normalization contract a generation was built with.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct TextNormalizerVersion {
    pub(crate) major: u16,
    pub(crate) minor: u16,
}

impl fmt::Display for TextNormalizerVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Whether a surface compares text after the case fold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaseMode {
    /// `case:yes`: no fold.
    Sensitive,
    /// `case:no`: per-character Unicode lowercase on both sides.
    Folded,
}

impl CaseMode {
    #[must_use]
    pub(crate) const fn from_case_sensitive(case_sensitive: bool) -> Self {
        if case_sensitive {
            Self::Sensitive
        } else {
            Self::Folded
        }
    }
}

/// Unicode NFC of `text`, borrowing when the input is already normalized.
#[must_use]
pub(crate) fn nfc(text: &str) -> Cow<'_, str> {
    match is_nfc_quick(text.chars()) {
        IsNormalized::Yes => Cow::Borrowed(text),
        IsNormalized::No | IsNormalized::Maybe => Cow::Owned(text.nfc().collect()),
    }
}

/// The case fold: per-character [`char::to_lowercase`].
#[must_use]
pub(crate) fn fold(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        out.extend(ch.to_lowercase());
    }
    out
}

/// Apply the case mode to already-NFC text.
#[must_use]
pub(crate) fn apply_case(text: &str, case: CaseMode) -> Cow<'_, str> {
    match case {
        CaseMode::Sensitive => Cow::Borrowed(text),
        CaseMode::Folded => Cow::Owned(fold(text)),
    }
}

/// Whether `ch` is part of a token rather than a boundary.
#[must_use]
pub(crate) fn is_token_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || is_combining_mark(ch)
}

/// One run of token characters.
///
/// `position` counts every run in the text, including runs longer than
/// [`MAX_TOKEN_BYTES`], so adjacency is the same whether or not a long run
/// sits between two tokens. `start..end` is the run's byte span in the NFC
/// text, and `text` is that span after the case mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Token {
    pub(crate) text: String,
    pub(crate) position: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Token {
    /// Whether the run is short enough to be an index term.
    #[must_use]
    pub(crate) const fn is_indexable(&self) -> bool {
        self.end.saturating_sub(self.start) <= MAX_TOKEN_BYTES
    }
}

/// NFC text plus every run in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Tokenized {
    pub(crate) text: String,
    pub(crate) tokens: Vec<Token>,
}

impl Tokenized {
    /// The runs that are index terms, in position order.
    pub(crate) fn indexable(&self) -> impl Iterator<Item = &Token> {
        self.tokens.iter().filter(|token| token.is_indexable())
    }
}

/// Normalize `text` to NFC and split it into runs under `case`.
#[must_use]
pub(crate) fn tokenize(text: &str, case: CaseMode) -> Tokenized {
    let text = nfc(text).into_owned();
    let mut tokens = Vec::new();
    let mut position = 0_usize;
    let mut run_start: Option<usize> = None;
    for (offset, ch) in text.char_indices() {
        match (run_start, is_token_char(ch)) {
            (None, true) => run_start = Some(offset),
            (Some(start), false) => {
                push_run(&text, start, offset, position, case, &mut tokens);
                position = position.saturating_add(1);
                run_start = None;
            }
            (None, false) | (Some(_), true) => {}
        }
    }
    if let Some(start) = run_start {
        push_run(&text, start, text.len(), position, case, &mut tokens);
    }
    Tokenized { text, tokens }
}

fn push_run(
    text: &str,
    start: usize,
    end: usize,
    position: usize,
    case: CaseMode,
    tokens: &mut Vec<Token>,
) {
    // `start` and `end` come from `char_indices` over `text` (or are
    // `text.len()`), so they are char boundaries; an absent slice would be a
    // tokenizer bug, and the run is then recorded as empty rather than
    // silently skipped so its position is still consumed.
    let run = text
        .get(start..end)
        .map_or_else(|| Cow::Borrowed(""), |run| apply_case(run, case));
    tokens.push(Token {
        text: run.into_owned(),
        position,
        start,
        end,
    });
}

/// Why a query literal cannot be lowered onto the token surfaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TextQueryError {
    /// The literal holds no token characters at all.
    NoTokens,
    /// One run in the literal exceeds [`MAX_TOKEN_BYTES`] and therefore never
    /// exists as an index term.
    TokenTooLong { bytes: usize, max: usize },
}

impl fmt::Display for TextQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTokens => f.write_str(
                "text query holds no searchable token; use a raw string '...' for byte semantics",
            ),
            Self::TokenTooLong { bytes, max } => write!(
                f,
                "text query holds a {bytes}-byte token, longer than the {max}-byte term cap; use a raw string '...' for byte semantics"
            ),
        }
    }
}

impl std::error::Error for TextQueryError {}

/// Tokenize a keyword or phrase literal for lowering.
///
/// Every run must be an index term: a run over [`MAX_TOKEN_BYTES`] is refused
/// with [`TextQueryError::TokenTooLong`] instead of leaving a gap the phrase
/// engine cannot express, and a literal with no run at all is refused with
/// [`TextQueryError::NoTokens`] instead of matching nothing. The returned
/// tokens therefore carry consecutive positions `0..n`.
pub(crate) fn query_tokens(text: &str, case: CaseMode) -> Result<Vec<Token>, TextQueryError> {
    let tokenized = tokenize(text, case);
    if tokenized.tokens.is_empty() {
        return Err(TextQueryError::NoTokens);
    }
    if let Some(long) = tokenized.tokens.iter().find(|token| !token.is_indexable()) {
        return Err(TextQueryError::TokenTooLong {
            bytes: long.end.saturating_sub(long.start),
            max: MAX_TOKEN_BYTES,
        });
    }
    Ok(tokenized.tokens)
}

/// Whether `document` contains `phrase` as consecutive index terms.
///
/// `document` is any token slice in position order (typically
/// [`Tokenized::indexable`] collected); `phrase` is a [`query_tokens`]
/// result. A dropped over-long run inside the document breaks adjacency
/// exactly as it does in the position sidecar and the inverted index.
#[must_use]
pub(crate) fn contains_phrase(document: &[Token], phrase: &[Token]) -> bool {
    let Some(first) = phrase.first() else {
        return false;
    };
    document
        .iter()
        .enumerate()
        .filter(|(_, token)| token.text == first.text)
        .any(|(anchor, token)| {
            phrase.iter().enumerate().all(|(offset, expected)| {
                document
                    .get(anchor.saturating_add(offset))
                    .is_some_and(|candidate| {
                        candidate.text == expected.text
                            && candidate.position == token.position.saturating_add(offset)
                    })
            })
        })
}

/// Whether `haystack` (NFC text) contains `needle` as a substring under `case`.
///
/// This is the raw-string contract: `needle` is NFC-normalized here, both
/// sides are folded under [`CaseMode::Folded`], and nothing is tokenized.
#[must_use]
pub(crate) fn contains_substring(haystack: &str, needle: &str, case: CaseMode) -> bool {
    let needle = nfc(needle);
    if needle.is_empty() {
        return false;
    }
    apply_case(haystack, case)
        .as_ref()
        .contains(apply_case(needle.as_ref(), case).as_ref())
}

#[cfg(test)]
mod tests {
    use super::{
        CaseMode, MAX_TOKEN_BYTES, TextQueryError, Token, contains_phrase, contains_substring,
        fold, nfc, query_tokens, tokenize,
    };

    fn texts(text: &str, case: CaseMode) -> Vec<String> {
        tokenize(text, case)
            .tokens
            .into_iter()
            .map(|token| token.text)
            .collect()
    }

    #[test]
    fn boundaries_are_punctuation_whitespace_and_symbols() {
        assert_eq!(
            texts("foo.bar foo-bar", CaseMode::Sensitive),
            ["foo", "bar", "foo", "bar"]
        );
        assert_eq!(
            texts("foo_bar fooBar", CaseMode::Sensitive),
            ["foo_bar", "fooBar"]
        );
        assert_eq!(texts("ok👍done", CaseMode::Sensitive), ["ok", "done"]);
        assert_eq!(texts("👍👍", CaseMode::Sensitive), Vec::<String>::new());
        assert_eq!(
            texts("全文検索エンジン", CaseMode::Sensitive),
            ["全文検索エンジン"]
        );
        assert_eq!(texts("नमस्ते दुनिया", CaseMode::Sensitive), ["नमस्ते", "दुनिया"]);
    }

    #[test]
    fn fold_is_unicode_lowercase_per_char() {
        assert_eq!(fold("CAFÉ"), "café");
        assert_eq!(fold("Данные"), "данные");
        assert_eq!(fold("ΣΊΣΥΦΟΣ"), "σίσυφοσ");
        assert_eq!(fold("Straße"), "straße");
        assert_eq!(fold("İ"), "i\u{307}");
        assert_eq!(texts("FooBar", CaseMode::Folded), ["foobar"]);
        assert_eq!(texts("FooBar", CaseMode::Sensitive), ["FooBar"]);
    }

    #[test]
    fn nfc_composes_and_maps_singletons() {
        assert_eq!(nfc("cafe\u{301}"), "café");
        assert_eq!(nfc("\u{212A}elvin"), "Kelvin");
        assert_eq!(nfc("ｆｏｏ"), "ｆｏｏ");
        assert!(matches!(nfc("plain"), std::borrow::Cow::Borrowed(_)));
        assert_eq!(texts("cafe\u{301}", CaseMode::Folded), ["café"]);
    }

    #[test]
    fn positions_count_every_run_and_spans_index_the_nfc_text() {
        let long = "a".repeat(MAX_TOKEN_BYTES.saturating_add(1));
        let text = format!("x {long} y");
        let tokenized = tokenize(&text, CaseMode::Folded);
        let positions: Vec<(usize, bool)> = tokenized
            .tokens
            .iter()
            .map(|token| (token.position, token.is_indexable()))
            .collect();
        assert_eq!(positions, [(0, true), (1, false), (2, true)]);
        let indexable: Vec<usize> = tokenized.indexable().map(|token| token.position).collect();
        assert_eq!(indexable, [0, 2]);
        for token in &tokenized.tokens {
            assert_eq!(
                tokenized.text.get(token.start..token.end),
                Some(token.text.as_str())
            );
        }
    }

    #[test]
    fn query_tokens_refuse_empty_and_over_long() {
        assert_eq!(
            query_tokens("👍", CaseMode::Folded),
            Err(TextQueryError::NoTokens)
        );
        assert_eq!(
            query_tokens("  ", CaseMode::Folded),
            Err(TextQueryError::NoTokens)
        );
        let long = "b".repeat(MAX_TOKEN_BYTES.saturating_add(1));
        assert_eq!(
            query_tokens(&format!("ok {long}"), CaseMode::Folded),
            Err(TextQueryError::TokenTooLong {
                bytes: MAX_TOKEN_BYTES.saturating_add(1),
                max: MAX_TOKEN_BYTES,
            })
        );
        let ok = query_tokens("Foo.Bar", CaseMode::Folded);
        let positions: Vec<(String, usize)> = ok
            .into_iter()
            .flatten()
            .map(|token| (token.text, token.position))
            .collect();
        assert_eq!(positions, [("foo".to_string(), 0), ("bar".to_string(), 1)]);
    }

    fn doc(text: &str) -> Vec<Token> {
        tokenize(text, CaseMode::Folded)
            .indexable()
            .cloned()
            .collect()
    }

    #[test]
    fn phrase_containment_needs_consecutive_positions() {
        let phrase = query_tokens("foo bar", CaseMode::Folded)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        assert!(contains_phrase(&doc("x foo.bar y"), &phrase));
        assert!(contains_phrase(&doc("FOO-BAR"), &phrase));
        assert!(!contains_phrase(&doc("foo_bar"), &phrase));
        assert!(!contains_phrase(&doc("foo x bar"), &phrase));
        assert!(!contains_phrase(&doc("bar foo"), &phrase));
        let long = "c".repeat(MAX_TOKEN_BYTES.saturating_add(1));
        assert!(!contains_phrase(&doc(&format!("foo {long} bar")), &phrase));
        assert!(!contains_phrase(&doc("foo bar"), &[]));
    }

    #[test]
    fn substring_containment_is_nfc_and_folded() {
        assert!(contains_substring("café au lait", "CAFÉ", CaseMode::Folded));
        assert!(contains_substring(
            "café au lait",
            "cafe\u{301}",
            CaseMode::Folded
        ));
        assert!(!contains_substring(
            "café au lait",
            "CAFÉ",
            CaseMode::Sensitive
        ));
        assert!(contains_substring("foo_bar", "o_b", CaseMode::Sensitive));
        assert!(!contains_substring("foo", "", CaseMode::Folded));
    }
}
