//! Code-aware tokenizer.
//!
//! Branches per [`PatternType`](crate::PatternType):
//!
//! - `Literal` — whitespace-delimited slices emit as a single
//!   [`TokenKind::Word`] each. No identifier split. Mirrors `dsl.md` §4
//!   literal-mode pin.
//! - `Keyword` / `Standard` — identifier-aware split on camelCase,
//!   `snake_case`, kebab-case, and digit boundaries. Sub-parts emit as
//!   [`TokenKind::IdentifierPart`] tokens; the un-split surface stays
//!   addressable via the original token's `byte_start..byte_end`.
//! - `Regexp` / `Structural` — return
//!   [`crate::errors::LexNormErrorCode::UnknownPatternType`]. Real
//!   implementations live in LEX-04 (regex) and STR-01 (structural) per
//!   the LEX-00 ticket §3 deferral pin.
//!
//! NFC contract: when `input` contains any non-ASCII byte, the tokenizer
//! first applies Unicode NFC via [`crate::nfc::normalize_nfc`] and then
//! tokenizes the normalized form. Emitted [`Token`] byte offsets index into
//! the NFC-normalized form, not the raw caller-supplied bytes.

mod implementation;

pub use implementation::{MAX_CHUNK_BYTES, Token, TokenKind, tokenize_text};
