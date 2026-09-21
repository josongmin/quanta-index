//! The inverted index's analyzer, as a thin binding over [`crate::normalize`].
//!
//! Tantivy sees exactly the token stream [`normalize::tokenize`] produces:
//! the same NFC text, the same boundaries, the same fold, the same positions
//! (over-long runs are skipped but still consume their position). Nothing
//! here decides what a token is; that contract lives in one place.

use tantivy::Index;
use tantivy::tokenizer::{TextAnalyzer, Token as IndexToken, TokenStream, Tokenizer};

use crate::normalize::{self, CaseMode, Token};

/// Registered name of the `case:no` analyzer (folded terms).
pub(crate) const FOLDED_TOKENIZER_NAME: &str = "qi_text_folded";
/// Registered name of the `case:yes` analyzer (terms as written, after NFC).
pub(crate) const CASE_SENSITIVE_TOKENIZER_NAME: &str = "qi_text_case";

/// Registered analyzer name for a case mode.
#[must_use]
pub(crate) const fn tokenizer_name(case: CaseMode) -> &'static str {
    match case {
        CaseMode::Folded => FOLDED_TOKENIZER_NAME,
        CaseMode::Sensitive => CASE_SENSITIVE_TOKENIZER_NAME,
    }
}

/// A Tantivy tokenizer that delegates to the shared normalizer.
#[derive(Clone)]
pub(crate) struct NormalizingTokenizer {
    case: CaseMode,
    token: IndexToken,
    runs: Vec<Token>,
}

impl NormalizingTokenizer {
    #[must_use]
    pub(crate) fn new(case: CaseMode) -> Self {
        Self {
            case,
            token: IndexToken::default(),
            runs: Vec::new(),
        }
    }
}

impl Tokenizer for NormalizingTokenizer {
    type TokenStream<'a> = NormalizedTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        self.runs = normalize::tokenize(text, self.case).tokens;
        self.token = IndexToken::default();
        let Self { token, runs, .. } = self;
        NormalizedTokenStream {
            runs,
            next: 0,
            token,
        }
    }
}

/// Iterates the indexable runs of one text.
pub(crate) struct NormalizedTokenStream<'a> {
    runs: &'a [Token],
    next: usize,
    token: &'a mut IndexToken,
}

impl TokenStream for NormalizedTokenStream<'_> {
    fn advance(&mut self) -> bool {
        while let Some(run) = self.runs.get(self.next) {
            self.next = self.next.saturating_add(1);
            if !run.is_indexable() {
                continue;
            }
            self.token.offset_from = run.start;
            self.token.offset_to = run.end;
            self.token.position = run.position;
            self.token.position_length = 1;
            self.token.text.clear();
            self.token.text.push_str(&run.text);
            return true;
        }
        false
    }

    fn token(&self) -> &IndexToken {
        self.token
    }

    fn token_mut(&mut self) -> &mut IndexToken {
        self.token
    }
}

/// Register the two analyzers every text field names, on the corpus index
/// and on the history text index alike.
///
/// Both are the shared normalizer; they differ only in case mode. No
/// filter is chained after it: the normalizer already owns boundaries,
/// folding, and the term-length cap.
pub(crate) fn register_analyzers(index: &Index) {
    for case in [CaseMode::Folded, CaseMode::Sensitive] {
        index
            .tokenizers()
            .register(tokenizer_name(case), TextAnalyzer::from(NormalizingTokenizer::new(case)));
    }
}

#[cfg(test)]
mod tests {
    use tantivy::tokenizer::{TokenStream as _, Tokenizer as _};

    use super::NormalizingTokenizer;
    use crate::normalize::{CaseMode, MAX_TOKEN_BYTES};

    fn stream(text: &str, case: CaseMode) -> Vec<(String, usize, usize, usize)> {
        let mut tokenizer = NormalizingTokenizer::new(case);
        let mut stream = tokenizer.token_stream(text);
        let mut out = Vec::new();
        while stream.advance() {
            let token = stream.token();
            out.push((token.text.clone(), token.position, token.offset_from, token.offset_to));
        }
        out
    }

    #[test]
    fn index_stream_is_the_normalizer_stream() {
        assert_eq!(
            stream("Foo.Bar foo_bar", CaseMode::Folded),
            [
                ("foo".to_string(), 0, 0, 3),
                ("bar".to_string(), 1, 4, 7),
                ("foo_bar".to_string(), 2, 8, 15),
            ]
        );
        assert_eq!(stream("CAFÉ", CaseMode::Sensitive), [("CAFÉ".to_string(), 0, 0, 5)]);
        assert_eq!(stream("cafe\u{301}", CaseMode::Folded), [("café".to_string(), 0, 0, 5)]);
    }

    #[test]
    fn over_long_runs_are_skipped_but_keep_their_position() {
        let long = "z".repeat(MAX_TOKEN_BYTES.saturating_add(1));
        let positions: Vec<usize> = stream(&format!("a {long} b"), CaseMode::Folded)
            .into_iter()
            .map(|(_, position, _, _)| position)
            .collect();
        assert_eq!(positions, [0, 2]);
    }

    #[test]
    fn a_tokenizer_is_reusable_across_texts() {
        let mut tokenizer = NormalizingTokenizer::new(CaseMode::Folded);
        let first: Vec<String> = {
            let mut stream = tokenizer.token_stream("one two");
            let mut out = Vec::new();
            while stream.advance() {
                out.push(stream.token().text.clone());
            }
            out
        };
        let second: Vec<String> = {
            let mut stream = tokenizer.token_stream("three");
            let mut out = Vec::new();
            while stream.advance() {
                out.push(stream.token().text.clone());
            }
            out
        };
        assert_eq!(first, ["one", "two"]);
        assert_eq!(second, ["three"]);
    }
}
