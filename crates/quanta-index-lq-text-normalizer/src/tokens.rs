//! Token boundaries, query-literal lowering and the two match predicates
//! (see the crate doc).

use core::fmt;
use std::borrow::Cow;

use unicode_normalization::char::is_combining_mark;

use crate::case::{BoundedNfcError, CaseMode, apply_case, bounded_nfc_with_checkpoint, nfc};

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

/// One borrowed run in NFC text, before case conversion.
///
/// Its byte span and position have the same meaning as [`Token`]. The run
/// borrows from its cursor, so it must be consumed before advancing again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenRun<'a> {
    pub text: &'a str,
    pub position: usize,
    pub start: usize,
    pub end: usize,
}

impl TokenRun<'_> {
    /// Whether the run is short enough to be an index term.
    #[must_use]
    pub const fn is_indexable(&self) -> bool {
        self.end.saturating_sub(self.start) <= MAX_TOKEN_BYTES
    }
}

/// Incremental run scanner over one NFC-normalized text value.
///
/// NFC normalization may allocate the entire input text. Scanning does not
/// allocate a per-file token vector; each returned run borrows this cursor.
#[derive(Debug)]
pub struct TokenCursor<'a, L = ()> {
    text: Cow<'a, str>,
    offset: usize,
    position: usize,
    // Text backing drops before its grant. Funded cursors cannot deep-clone it.
    memory: Option<L>,
}

impl Clone for TokenCursor<'_> {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            offset: self.offset,
            position: self.position,
            memory: self.memory,
        }
    }
}

impl<'a> TokenCursor<'a> {
    /// Normalize once and prepare to scan runs from the start.
    #[must_use]
    pub fn new(text: &'a str) -> Self {
        Self {
            text: nfc(text),
            offset: 0,
            position: 0,
            memory: None,
        }
    }

    /// Count and admit NFC output before allocating it. The normalization
    /// producer calls `checkpoint` throughout both passes, including long
    /// combining-mark runs.
    pub fn try_new_bounded<E, L>(
        text: &'a str,
        max_output_bytes: usize,
        max_total_bytes: usize,
        mut checkpoint: impl FnMut(u64) -> Result<(), E>,
        reserve: impl FnMut(usize) -> Result<L, E>,
    ) -> Result<TokenCursor<'a, L>, BoundedNfcError<E>> {
        checkpoint(0).map_err(BoundedNfcError::Checkpoint)?;
        // ASCII is already NFC. Scan in bounded chunks so a large unchanged
        // source remains borrowed while cancellation stays observable.
        let mut ascii = true;
        for chunk in text.as_bytes().chunks(4096) {
            checkpoint(
                u64::try_from(chunk.len()).map_err(|_error| BoundedNfcError::OutputExceeded)?,
            )
            .map_err(BoundedNfcError::Checkpoint)?;
            if !chunk.is_ascii() {
                ascii = false;
                break;
            }
        }
        if ascii {
            if text.len() > max_output_bytes {
                return Err(BoundedNfcError::OutputExceeded);
            }
            return Ok(TokenCursor {
                text: Cow::Borrowed(text),
                offset: 0,
                position: 0,
                memory: None,
            });
        }
        let (normalized, memory) = bounded_nfc_with_checkpoint(
            text,
            max_output_bytes,
            max_total_bytes,
            checkpoint,
            reserve,
        )?;
        Ok(TokenCursor {
            text: normalized,
            offset: 0,
            position: 0,
            memory,
        })
    }

    /// Take the uncharged NFC text, copying when normalization borrowed input.
    #[must_use]
    pub fn into_text(self) -> String {
        self.text.into_owned()
    }
}

impl<L> TokenCursor<'_, L> {
    /// The full NFC text whose byte offsets are reported by [`Self::next_run`].
    #[must_use]
    pub fn text(&self) -> &str {
        self.text.as_ref()
    }

    /// Return the next run without materializing later runs.
    pub fn next_run(&mut self) -> Option<TokenRun<'_>> {
        let remaining = self.text.get(self.offset..)?;
        let mut run_start = None;
        for (relative, ch) in remaining.char_indices() {
            let absolute = self.offset.saturating_add(relative);
            if is_token_char(ch) {
                if run_start.is_none() {
                    run_start = Some(absolute);
                }
            } else if let Some(start) = run_start {
                let text = self.text.get(start..absolute)?;
                let position = self.position;
                self.position = self.position.saturating_add(1);
                self.offset = absolute.saturating_add(ch.len_utf8());
                return Some(TokenRun {
                    text,
                    position,
                    start,
                    end: absolute,
                });
            }
        }
        self.offset = self.text.len();
        let start = run_start?;
        let end = self.text.len();
        let text = self.text.get(start..end)?;
        let position = self.position;
        self.position = self.position.saturating_add(1);
        Some(TokenRun {
            text,
            position,
            start,
            end,
        })
    }
}

/// Normalize `text` to NFC and split it into runs under `case`.
#[must_use]
pub fn tokenize(text: &str, case: CaseMode) -> Tokenized {
    let mut cursor = TokenCursor::new(text);
    let mut tokens = Vec::new();
    while let Some(run) = cursor.next_run() {
        tokens.push(Token {
            text: apply_case(run.text, case).into_owned(),
            position: run.position,
            start: run.start,
            end: run.end,
        });
    }
    Tokenized {
        text: cursor.into_text(),
        tokens,
    }
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
        CaseMode, MAX_TOKEN_BYTES, TextQueryError, Token, TokenCursor, TokenRun, contains_phrase,
        contains_substring, query_tokens, tokenize,
    };

    #[test]
    fn cursor_reports_fixed_nfc_runs_and_offsets() {
        let mut cursor = TokenCursor::new("cafe\u{301}!नमस्ते");
        assert_eq!(cursor.text(), "café!नमस्ते");
        assert_eq!(
            cursor.next_run(),
            Some(TokenRun {
                text: "café",
                position: 0,
                start: 0,
                end: 5,
            })
        );
        assert_eq!(
            cursor.next_run(),
            Some(TokenRun {
                text: "नमस्ते",
                position: 1,
                start: 6,
                end: 24,
            })
        );
        assert_eq!(cursor.next_run(), None);
        assert_eq!(cursor.into_text(), "café!नमस्ते");
    }

    #[test]
    fn bounded_cursor_matches_fixed_nfc_offsets_and_rejects_expansion() -> Result<(), String> {
        let source = "cafe\u{301}!नमस्ते";
        let mut cursor = TokenCursor::try_new_bounded(
            source,
            24,
            4096,
            |_units| Ok::<_, ()>(()),
            |_bytes| Ok(()),
        )
        .map_err(|error| format!("exact NFC fixture: {error:?}"))?;
        if cursor.text() != "café!नमस्ते"
            || cursor
                .next_run()
                .map(|run| (run.position, run.start, run.end))
                != Some((0, 0, 5))
            || cursor
                .next_run()
                .map(|run| (run.position, run.start, run.end))
                != Some((1, 6, 24))
            || cursor.next_run().is_some()
        {
            return Err("fixed NFC text, positions or byte offsets differ".into());
        }
        let expanded = "\u{1D160}".repeat(8);
        let expected = "\u{1D158}\u{1D165}\u{1D16E}".repeat(8);
        if crate::case::nfc(&expanded) != expected || expected.len() != 96 {
            return Err("fixed canonical expansion differs".into());
        }
        if TokenCursor::try_new_bounded(
            &expanded,
            expanded.len(),
            4096,
            |_units| Ok::<_, ()>(()),
            |_bytes| Ok(()),
        )
        .is_ok()
        {
            return Err("expanded output exceeded its admitted bound".into());
        }
        let admitted = TokenCursor::try_new_bounded(
            &expanded,
            96,
            4096,
            |_units| Ok::<_, ()>(()),
            |_bytes| Ok(()),
        )
        .map_err(|error| format!("exact expansion fixture: {error:?}"))?;
        if admitted.text() != expected {
            return Err("fixed expanded NFC text differs".into());
        }
        Ok(())
    }

    struct TestGrant {
        live: std::rc::Rc<std::cell::Cell<usize>>,
        bytes: usize,
    }

    impl Drop for TestGrant {
        fn drop(&mut self) {
            self.live.set(
                self.live
                    .get()
                    .checked_sub(self.bytes)
                    .expect("live grant underflow"),
            );
        }
    }

    fn grant(
        live: &std::rc::Rc<std::cell::Cell<usize>>,
        bytes: usize,
    ) -> Result<TestGrant, &'static str> {
        live.set(live.get().checked_add(bytes).ok_or("live grant overflow")?);
        Ok(TestGrant {
            live: live.clone(),
            bytes,
        })
    }

    #[test]
    fn funded_cursor_keeps_only_output_grant_until_cursor_drop() -> Result<(), String> {
        let live = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut cursor = TokenCursor::try_new_bounded(
            "e\u{301}!",
            3,
            4096,
            |_units| Ok(()),
            |bytes| grant(&live, bytes),
        )
        .map_err(|error| format!("funded NFC: {error:?}"))?;
        if cursor.text() != "é!"
            || live.get() != 3
            || cursor.next_run().map(|run| (run.text, run.position)) != Some(("é", 0))
        {
            return Err("owned output or retained output grant differs".into());
        }
        drop(cursor);
        if live.get() != 0 {
            return Err("cursor drop did not release output grant".into());
        }
        Ok(())
    }

    #[test]
    fn funded_ascii_cursor_never_requests_a_memory_grant() -> Result<(), String> {
        let source = "ascii ".repeat(2048);
        let cursor = TokenCursor::try_new_bounded(
            &source,
            source.len(),
            0,
            |_units| Ok(()),
            |_bytes| Err::<TestGrant, _>("unexpected grant"),
        )
        .map_err(|error| format!("funded ASCII: {error:?}"))?;
        if cursor.text().as_ptr() != source.as_ptr() {
            return Err("ASCII lost its source borrow".into());
        }
        Ok(())
    }

    #[test]
    fn funded_output_denial_preserves_original_refusal() -> Result<(), String> {
        let result = TokenCursor::try_new_bounded(
            "e\u{301}",
            2,
            4096,
            |_units| Ok(()),
            |_bytes| Err::<TestGrant, _>("output denied"),
        );
        let Err(error) = result else {
            return Err("denied output was allocated".into());
        };
        if !matches!(error.into_checkpoint(), Ok("output denied")) {
            return Err("memory refusal was changed".into());
        }
        Ok(())
    }

    #[test]
    fn second_pass_denial_releases_output_and_census_scratch_grants() -> Result<(), String> {
        let source = format!("e\u{301}{}", "\u{315}".repeat(32));
        let expected = format!("é{}", "\u{315}".repeat(32));
        let live = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut output_funded = false;
        let result = TokenCursor::try_new_bounded(
            &source,
            expected.len(),
            4096,
            |_units| Ok(()),
            |bytes| {
                if output_funded {
                    return Err("second pass denied");
                }
                if bytes == expected.len() {
                    output_funded = true;
                }
                grant(&live, bytes)
            },
        );
        let Err(error) = result else {
            return Err("denied second-pass allocation succeeded".into());
        };
        if !output_funded
            || live.get() != 0
            || !matches!(error.into_checkpoint(), Ok("second pass denied"))
        {
            return Err("second-pass refusal or grant cleanup differs".into());
        }
        Ok(())
    }

    #[test]
    fn scratch_growth_reserves_old_and_new_backing_together() -> Result<(), String> {
        let source = format!("e\u{301}{}", "\u{315}".repeat(32));
        let expected = format!("é{}", "\u{315}".repeat(32));
        let live = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut overlap = false;
        let cursor = TokenCursor::try_new_bounded(
            &source,
            expected.len(),
            4096,
            |_units| Ok(()),
            |bytes| {
                if live.get() > 0 && bytes != expected.len() {
                    overlap = true;
                }
                grant(&live, bytes)
            },
        )
        .map_err(|error| format!("funded growth: {error:?}"))?;
        if !overlap || cursor.text() != expected || live.get() != expected.len() {
            return Err("scratch overlap or completed output grant differs".into());
        }
        drop(cursor);
        if live.get() != 0 {
            return Err("scratch/output grants leaked".into());
        }
        Ok(())
    }

    #[test]
    fn cancellation_after_scratch_birth_releases_every_grant() -> Result<(), String> {
        let source = format!("e\u{301}{}", "\u{315}".repeat(2048));
        let live = std::rc::Rc::new(std::cell::Cell::new(0));
        let result = TokenCursor::try_new_bounded(
            &source,
            source.len(),
            128 * 1024,
            |_units| {
                if live.get() > 0 {
                    Err("cancelled")
                } else {
                    Ok(())
                }
            },
            |bytes| grant(&live, bytes),
        );
        let Err(error) = result else {
            return Err("cancellation after birth succeeded".into());
        };
        if !matches!(error.into_checkpoint(), Ok("cancelled")) || live.get() != 0 {
            return Err("cancellation cause or scratch cleanup differs".into());
        }
        Ok(())
    }

    #[test]
    fn panic_after_scratch_birth_releases_every_grant() {
        let source = format!("e\u{301}{}", "\u{315}".repeat(2048));
        let live = std::rc::Rc::new(std::cell::Cell::new(0));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            TokenCursor::try_new_bounded(
                &source,
                source.len(),
                128 * 1024,
                |_units| {
                    assert_eq!(live.get(), 0, "injected panic after native birth");
                    Ok(())
                },
                |bytes| grant(&live, bytes),
            )
        }));
        assert!(result.is_err(), "native scratch birth was not reached");
        assert_eq!(live.get(), 0, "scratch grant leaked across panic");
    }

    #[test]
    fn bounded_ascii_cursor_borrows_and_checks_every_chunk() -> Result<(), String> {
        let source = "word ".repeat(4096);
        let mut calls = 0u64;
        let mut cursor = TokenCursor::try_new_bounded(
            &source,
            source.len(),
            0,
            |_units| {
                calls = calls.checked_add(1).ok_or("checkpoint count overflow")?;
                Ok::<_, &str>(())
            },
            |_bytes| Ok(()),
        )
        .map_err(|error| format!("borrowed ASCII fixture: {error:?}"))?;
        if cursor.text().as_ptr() != source.as_ptr()
            || calls < 6
            || cursor
                .next_run()
                .map(|run| (run.text, run.position, run.start, run.end))
                != Some(("word", 0, 0, 4))
        {
            return Err("ASCII borrow, checkpoint cadence or first token differs".into());
        }
        if TokenCursor::try_new_bounded(
            &source,
            source
                .len()
                .checked_sub(1)
                .ok_or("ASCII fixture has no bytes")?,
            0,
            |_units| Ok::<_, ()>(()),
            |_bytes| Ok(()),
        )
        .is_ok()
        {
            return Err("oversized ASCII source was admitted".into());
        }
        Ok(())
    }

    #[test]
    fn bounded_canonical_unicode_cursor_borrows_after_controlled_census() -> Result<(), String> {
        let source = "한글 café 데이터";
        let mut cursor = TokenCursor::try_new_bounded(
            source,
            source.len(),
            4096,
            |_units| Ok::<_, ()>(()),
            |_bytes| Ok(()),
        )
        .map_err(|error| format!("canonical Unicode fixture: {error:?}"))?;
        if cursor.text().as_ptr() != source.as_ptr()
            || cursor.next_run().map(|run| (run.text, run.position)) != Some(("한글", 0))
            || cursor.next_run().map(|run| (run.text, run.position)) != Some(("café", 1))
            || cursor.next_run().map(|run| (run.text, run.position)) != Some(("데이터", 2))
            || cursor.next_run().is_some()
        {
            return Err("canonical Unicode borrow or token sequence differs".into());
        }
        Ok(())
    }

    #[test]
    fn bounded_cursor_checks_long_combining_run_before_emitting_prefix() -> Result<(), String> {
        let source = format!("a{}b", "\u{315}".repeat(4096));
        let mut visits = 0u64;
        let result = TokenCursor::try_new_bounded(
            &source,
            source.len(),
            32 * 1024,
            |_units| {
                visits = visits.checked_add(1).ok_or("checkpoint count overflow")?;
                if visits > 1 { Err("cancelled") } else { Ok(()) }
            },
            |_bytes| Ok(()),
        );
        let Err(failure) = result else {
            return Err("long combining run ignored cancellation".into());
        };
        if !matches!(failure.into_checkpoint(), Ok("cancelled")) || visits > 4 {
            return Err("exact cancellation or checkpoint bound differs".into());
        }
        Ok(())
    }

    #[test]
    fn cursor_counts_long_run_without_indexing_it() {
        let long = "a".repeat(MAX_TOKEN_BYTES.saturating_add(1));
        let text = format!("x {long} y");
        let mut cursor = TokenCursor::new(&text);
        assert_eq!(
            cursor.next_run().map(|run| (run.text, run.position)),
            Some(("x", 0))
        );
        let long_run = cursor.next_run();
        assert_eq!(
            long_run.map(|run| (
                run.text,
                run.position,
                run.start,
                run.end,
                run.is_indexable()
            )),
            Some((long.as_str(), 1, 2, 259, false))
        );
        assert_eq!(
            cursor.next_run(),
            Some(TokenRun {
                text: "y",
                position: 2,
                start: 260,
                end: 261,
            })
        );
        assert_eq!(cursor.next_run(), None);
    }

    #[test]
    fn cursor_empty_punctuation_case_and_early_stop() {
        let mut empty = TokenCursor::new("😄!?  ");
        assert_eq!(empty.next_run(), None);
        assert_eq!(empty.into_text(), "😄!?  ");

        let text = format!("MiXeD.{}", "later!".repeat(10_000));
        let mut cursor = TokenCursor::new(&text);
        assert_eq!(
            cursor
                .next_run()
                .map(|run| (run.text, run.position, run.start, run.end)),
            Some(("MiXeD", 0, 0, 5))
        );
        assert_eq!(cursor.text(), text);
        assert_eq!(
            tokenize("MiXeD.foo", CaseMode::Sensitive).tokens,
            [
                Token {
                    text: "MiXeD".into(),
                    position: 0,
                    start: 0,
                    end: 5
                },
                Token {
                    text: "foo".into(),
                    position: 1,
                    start: 6,
                    end: 9
                },
            ]
        );
        assert_eq!(
            tokenize("MiXeD.foo", CaseMode::Folded).tokens,
            [
                Token {
                    text: "mixed".into(),
                    position: 0,
                    start: 0,
                    end: 5
                },
                Token {
                    text: "foo".into(),
                    position: 1,
                    start: 6,
                    end: 9
                },
            ]
        );
    }

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
