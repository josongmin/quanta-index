//! LQ canonical normalizer.
//!
//! Idempotent: `normalize(normalize(x)) == normalize(x)` (byte-identical).
//!
//! Transforms per dsl.md §10:
//! 1. Strip a leading `(?i)` from regex leaves and record it as `case:no`
//!    on the options; leaf text is never folded here (the text normalizer
//!    folds once, on the executing surface — QI-BB-011).
//! 2. Flatten nested `LqExpr::All`/`LqExpr::Any` of the same kind (one level
//!    deep — fixed-point after the recursive `normalize_expr` returns).
//! 3. Sort commutative children (`All`, `Any`) by canonical key.
//! 4. Dedup adjacent identical children.
//! 5. Sort filters and directives by canonical key.
//! 6. Collapse trivial single-child `All`/`Any` to the child.
//!
//! The canonical key used for ordering is a stable string derived from the
//! structural form of each sub-tree. It is not the hash output; the hash
//! is computed by [`crate::hasher`] after normalize completes.

mod implementation;

pub use implementation::{NormalizedOptions, normalize};
