#![forbid(unsafe_code)]

//! LEX-00 — Lexical text normalization pipeline for indexed source text.
//!
//! Status: Wave-1 scaffold. Owns the canonical write/read tokenization
//! pipeline that every lexical authority (content, path, symbol) consumes.
//!
//! Surface guarantees:
//!
//! - Wire shapes use hand-rolled `impl serde::Serialize`/`Deserialize` per
//!   D18 (no proc-macro derives, semgrep-enforced).
//! - Every tokenization / fold / language-routing failure returns a typed
//!   [`LexNormError`] with closed [`LexNormErrorCode`]; no silent fallback.
//! - Idempotency invariant: `normalize ∘ normalize == normalize` over the
//!   `Token` stream. Property-tested.
//!
//! Disambiguation: this crate is the *text* normalizer for indexed source
//! chunks. The orthogonal LQ DSL query normalizer (PRE-NORM) lives at
//! `quanta-index-lq-norm` and is not consumed here.
//!
//! `NFC_DEFERRED`: Unicode NFC normalization is currently a no-op at the
//! input boundary. Adding it is a follow-up that pulls in
//! `unicode-normalization` as a workspace dep. Callers must not rely on NFC
//! equivalence yet.

pub mod errors;
pub mod folder;
pub mod lang;
pub mod patterntype;
pub mod tokenizer;

pub use errors::{LexNormError, LexNormErrorCode};
pub use folder::{CaseFold, fold_case};
pub use lang::{LangId, detect_lang};
pub use patterntype::{DEFAULT_PATTERN_TYPE, PatternType};
pub use tokenizer::{Token, TokenKind, tokenize_text};

/// Hexagonal port: any text normalizer the lexical adapter consumes.
///
/// The v1 default normalizer [`DefaultLexicalNormalizer`] composes
/// [`tokenize_text`] and [`fold_case`]. Future swaps (per-language analyzer
/// packs, stemmer dispatch, stop-token filtering) are pure refactors that
/// land as alternate `LexicalNormalizer` implementations.
pub trait LexicalNormalizer: Send + Sync {
    /// Normalize raw text into a `Token` stream. Lang and pattern-type
    /// routing is per-call so the same instance can serve every shard.
    fn normalize(
        &self,
        input: &str,
        lang: LangId,
        pt: PatternType,
    ) -> Result<Vec<Token>, LexNormError>;
}

/// V1 default normalizer. Composes tokenization with case folding.
///
/// Construction is `const`; clone is free. The `case` field is the only
/// configuration knob; per-call lang and pattern-type override no defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DefaultLexicalNormalizer {
    case: CaseFold,
}

impl DefaultLexicalNormalizer {
    /// Construct with a case-fold mode. Use [`CaseFold::Lower`] for the
    /// default `case:no` writer/reader pair.
    #[must_use]
    pub const fn new(case: CaseFold) -> Self {
        Self { case }
    }

    /// Configured case-fold mode.
    #[must_use]
    pub const fn case(&self) -> CaseFold {
        self.case
    }
}

impl LexicalNormalizer for DefaultLexicalNormalizer {
    fn normalize(
        &self,
        input: &str,
        lang: LangId,
        pt: PatternType,
    ) -> Result<Vec<Token>, LexNormError> {
        let toks = tokenize_text(input, lang, pt)?;
        Ok(toks.into_iter().map(|t| fold_case(&t, self.case)).collect())
    }
}

/// Convenience wrapper: tokenize + fold in one call.
///
/// Equivalent to constructing a [`DefaultLexicalNormalizer`] and calling
/// [`LexicalNormalizer::normalize`].
pub fn normalize_text(
    input: &str,
    lang: LangId,
    pt: PatternType,
    case: CaseFold,
) -> Result<Vec<Token>, LexNormError> {
    DefaultLexicalNormalizer::new(case).normalize(input, lang, pt)
}
