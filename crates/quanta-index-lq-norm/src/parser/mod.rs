//! LQ recursive-descent parser.
//!
//! Consumes the `LqToken` sequence emitted by [`crate::tokenizer::tokenize`]
//! and produces an [`LqNormalizedQuery`](crate::ast::LqNormalizedQuery)
//! (pre-normalize — the normalizer pass is a separate step). Boolean
//! precedence is `NOT > AND > OR` per dsl.md §3.2; explicit groups via
//! `(...)`. AST depth tracker is checked at every recursion against
//! [`crate::limits::MAX_AST_DEPTH`] before the recursive call descends.

mod implementation;

pub use implementation::parse;
