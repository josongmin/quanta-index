//! Token boundaries, query-literal lowering and the two match predicates
//! (see the crate doc).

use core::fmt;
use std::borrow::Cow;

use unicode_normalization::char::is_combining_mark;

use crate::case::{CaseMode, apply_case, nfc};

/// Longest run, in bytes of NFC text, that is emitted as a token.
///
/// Longer runs (minified blobs, base64, hashes past sha-512 hex) keep their
/// position so phrase adjacency stays exact, but they never enter a term
/// dictionary or a position sidecar. The raw-string and regex surfaces
/// still see them.
pub const MAX_TOKEN_BYTES: usize = 256;

/// Whether `ch` is part of a token rather than a boundary.
#[must_use]
pub fn is_token_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || is_combining_mark(ch)
}

/// One run of token characters.
///
/// `position` counts every run in the text, including runs longer than
/// [`MAX_TOKEN_BYTES`], so adjacency is the same whether or not a long run
/// sits between two tokens. `start..end` is the run's byte span in the NFC
/// text, and `text` is that span after the case mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub text: String,
    pub position: usize,
    pub start: usize,
    pub end: usize,
}

impl Token {
    /// Whether the run is short enough to be an index term.
    #[must_use]
    pub const fn is_indexable(&self) -> bool {
        self.end.saturating_sub(self.start) <= MAX_TOKEN_BYTES
    }
}

/// NFC text plus every run in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tokenized {
    pub text: String,
    pub tokens: Vec<Token>,
}

impl Tokenized {
    /// The runs that are index terms, in position order.
    pub fn indexable(&self) -> impl Iterator<Item = &Token> {
        self.tokens.iter().filter(|token| token.is_indexable())
    }
}

/// Normalize `text` to NFC and split it into runs under `case`.
#[must_use]
pub fn tokenize(text: &str, case: CaseMode) -> Tokenized {
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
pub enum TextQueryError {
    /// The literal holds no token characters at all.
    NoTokens,
    /// One run in the literal exceeds [`MAX_TOKEN_BYTES`] and therefore never
    /// exists as an index term.
    TokenTooLong { bytes: usize, max: usize },
}

impl TextQueryError {
    /// The typed wire code every route answers this refusal with.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoTokens => "LEX_TEXT_QUERY_NO_TOKENS",
            Self::TokenTooLong { .. } => "LEX_TEXT_QUERY_TOKEN_TOO_LONG",
        }
    }
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
pub fn query_tokens(text: &str, case: CaseMode) -> Result<Vec<Token>, TextQueryError> {
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
/// exactly as it does in a position sidecar and an inverted index. This is
/// the keyword / phrase contract: a one-token phrase is a keyword.
#[must_use]
pub fn contains_phrase(document: &[Token], phrase: &[Token]) -> bool {
    phrase_ranges(document, phrase).next().is_some()
}

/// NFC byte ranges for the same consecutive-token predicate as
/// [`contains_phrase`].
///
/// Consume only the required bounded number of witnesses.
/// Like that predicate, `document` must contain index terms in position order;
/// dropped long runs are represented by position gaps, never bridged.
pub fn phrase_ranges<'a>(
    document: &'a [Token],
    phrase: &'a [Token],
) -> impl Iterator<Item = core::ops::Range<usize>> + 'a {
    document
        .iter()
        .enumerate()
        .filter(|(_, token)| phrase.first().is_some_and(|first| token.text == first.text))
        .filter_map(|(anchor, token)| {
            let matches = phrase.iter().enumerate().all(|(offset, expected)| {
                document
                    .get(anchor.saturating_add(offset))
                    .is_some_and(|candidate| {
                        candidate.text == expected.text
                            && candidate.position == token.position.saturating_add(offset)
                    })
            });
            if !matches {
                return None;
            }
            let last = document.get(anchor.saturating_add(phrase.len().checked_sub(1)?))?;
            Some(token.start..last.end)
        })
}

/// Whether `haystack` (NFC text) contains `needle` as a substring under `case`.
///
/// This is the raw-string contract: `needle` is NFC-normalized here, both
/// sides are folded under [`CaseMode::Folded`], and nothing is tokenized.
#[must_use]
pub fn contains_substring(haystack: &str, needle: &str, case: CaseMode) -> bool {
    if needle.is_empty() {
        return false;
    }
    substring_range_in_case_text(apply_case(haystack, case).as_ref(), needle, case).is_some()
}

/// Locate in text already transformed by `apply_case`; coordinates stay in
/// that transformed text. Shared by substring truth and provenance witnesses.
pub(crate) fn substring_range_in_case_text(
    haystack: &str,
    needle: &str,
    case: CaseMode,
) -> Option<core::ops::Range<usize>> {
    substring_ranges_in_case_text(haystack, needle, case).next()
}

/// Enumerate every raw-substring occurrence, including overlaps.
///
/// Reuse the truth transformation and advance one UTF-8 scalar from the
/// previous start so a later occurrence is not hidden by an earlier match.
pub(crate) fn substring_ranges_in_case_text<'a>(
    haystack: &'a str,
    needle: &str,
    case: CaseMode,
) -> impl Iterator<Item = core::ops::Range<usize>> + 'a {
    let needle = nfc(needle);
    let needle = apply_case(needle.as_ref(), case).into_owned();
    let mut cursor = 0;
    std::iter::from_fn(move || {
        if needle.is_empty() {
            return None;
        }
        let remaining = haystack.get(cursor..)?;
        let relative = remaining.find(needle.as_str())?;
        let start = cursor.saturating_add(relative);
        let end = start.saturating_add(needle.len());
        let first = haystack.get(start..)?.chars().next()?;
        cursor = start.saturating_add(first.len_utf8());
        Some(start..end)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CaseMode, MAX_TOKEN_BYTES, TextQueryError, Token, contains_phrase, contains_substring,
        query_tokens, tokenize,
    };

    fn texts(text: &str, case: CaseMode) -> Vec<String> {
        tokenize(text, case)
            .tokens
            .into_iter()
            .map(|token| token.text)
            .collect()
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "byte-exact phrase fixture assertions; query setup errors propagate"
    )]
    fn l4_phrase_ranges_share_token_truth_and_nfc_offsets() -> Result<(), TextQueryError> {
        let phrase = query_tokens("blue whale", CaseMode::Folded)?;
        let document = tokenize("BLUE\r\nwhale", CaseMode::Folded);
        let ranges: Vec<_> = super::phrase_ranges(&document.tokens, &phrase).collect();
        assert_eq!(ranges, vec![0..11]);
        assert!(contains_phrase(&document.tokens, &phrase));
        let needle = query_tokens("needle", CaseMode::Folded)?;
        let document = tokenize("needlework NEEDLE", CaseMode::Folded);
        assert_eq!(
            super::phrase_ranges(&document.tokens, &needle).collect::<Vec<_>>(),
            vec![11..17]
        );
        let document = tokenize(
            &format!(
                "blue {} whale",
                "x".repeat(MAX_TOKEN_BYTES.saturating_add(1))
            ),
            CaseMode::Folded,
        );
        let present: Vec<_> = document.indexable().cloned().collect();
        assert!(super::phrase_ranges(&present, &phrase).next().is_none());
        assert!(!contains_phrase(&present, &phrase));
        assert!(super::phrase_ranges(&present, &[]).next().is_none());
        Ok(())
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
        assert_eq!(texts("FooBar", CaseMode::Folded), ["foobar"]);
        assert_eq!(texts("FooBar", CaseMode::Sensitive), ["FooBar"]);
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
        assert_eq!(TextQueryError::NoTokens.code(), "LEX_TEXT_QUERY_NO_TOKENS");
        assert_eq!(
            TextQueryError::TokenTooLong { bytes: 1, max: 0 }.code(),
            "LEX_TEXT_QUERY_TOKEN_TOO_LONG"
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
        // A one-token phrase is a whole-token keyword: never a substring.
        let keyword = query_tokens("fix", CaseMode::Folded)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        assert!(contains_phrase(&doc("Fix typo"), &keyword));
        assert!(!contains_phrase(&doc("prefix"), &keyword));
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
