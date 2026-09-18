#![forbid(unsafe_code)]

//! The one text normalization contract of the search plane (QI-BB-011).
//!
//! Every surface that turns text into something comparable goes through
//! this crate, at build time and at query time: the lexical adapter's
//! inverted-index analyzer, its phrase position and trigram sidecars, its
//! raw-substring and regex verifiers and its `index:no` manual scan; the
//! history text index that scores commit messages and diff hunks; and the
//! search plane's in-memory text planes that filter history rows and
//! runtime-metadata chunks. The query DSL (`quanta-index-lq-norm`) never
//! folds a literal: it records the `case:` option, and the option selects
//! a [`CaseMode`] here. There is no second tokenizer, no second case fold
//! and no second normalization form anywhere in the pipeline, so the same
//! DSL query answers identically whichever syntax, leaf kind, route or
//! order executes it.
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
//! **Case folding.** [`CaseMode::Folded`] — the DSL default, `case:` absent
//! or `case:no` — compares text after per-character [`char::to_lowercase`]:
//! the full Unicode lowercase mapping, not ASCII. `CAFÉ` and `café` are one
//! token; `Данные` and `данные` are one token. The mapping is context-free
//! and locale-free: a final `Σ` folds to `σ` (never `ς`), `İ` (U+0130) folds
//! to `i` + U+0307 rather than to plain `i`, and `ß` is already lowercase
//! and is never expanded to `ss`. [`CaseMode::Sensitive`] (`case:yes`)
//! applies no fold at all. A query literal is folded exactly once, here,
//! by the surface that executes it — never by the parser — so
//! `str::to_lowercase`'s context-sensitive final-sigma rule can never turn
//! `ΟΔΟΣ` into `οδος` on the query side while the index holds `οδοσ`.
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
//! semantics: they are tokenized as above and match whole tokens, in
//! sequence. Raw-string (`'...'`) and regex (`/.../`) leaves have substring
//! semantics over the NFC text of the document: no tokenization, punctuation
//! and whitespace are literal. A raw-string literal is NFC-normalized like
//! any query text, and under [`CaseMode::Folded`] it is compared after the
//! same fold as the tokenizer against a folded copy of the document. A regex
//! source is NFC-normalized as text (so write `\x{301}` rather than a literal
//! combining mark when the pattern must name a decomposed sequence, which can
//! never match anyway because the document is NFC) and runs with the regex
//! engine's own `(?i)` under `case:no`; a trigram prefilter folds the
//! extracted literal alternatives with [`fold`], which is sound because every
//! alternative is a character-boundary prefix of some match and the fold is
//! per character.
//!
//! **Versioning.** [`TEXT_NORMALIZER_VERSION`] names this contract. Sealed
//! generations and history text epochs record it; one built under any other
//! version is refused typed at open rather than served with mismatched
//! semantics. Any change to the rules above — including a Unicode table
//! update that changes a mapping this crate relies on — is a version bump.
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

mod case;
mod tokens;
mod version;

pub use case::{CaseMode, apply_case, fold, nfc};
pub use tokens::{
    MAX_TOKEN_BYTES, TextQueryError, Token, Tokenized, contains_phrase, contains_substring,
    is_token_char, query_tokens, tokenize,
};
pub use version::{TEXT_NORMALIZER_VERSION, TextNormalizerVersion};
