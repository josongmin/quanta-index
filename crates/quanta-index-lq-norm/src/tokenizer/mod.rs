//! LQ tokenizer — pure lex over a `&str` input.
//!
//! The lexer is byte-indexed against the original input; every token carries
//! an [`LqSpan`](crate::errors::LqSpan) with `[start, end)` byte offsets so
//! the parser and error envelopes can attribute every AST node to a source
//! range.
//!
//! Tokenizer recognizes a subset of the dsl.md §1.7 token kinds large enough
//! to drive the canonical parse → normalize → hash pipeline that PRE-CONF
//! consumes; structural sub-grammar, predicate calls, and bridge directives
//! beyond `into:codeql` are deferred to a follow-up ticket (see PRE-NORM
//! ticket §4.1 entries for `predicates.rs` and `structural.rs`).

mod implementation;

pub use implementation::{LqToken, LqTokenKind, tokenize};
