//! Structural query route, split by responsibility.
//!
//! Dependency direction: `route` -> { `lowering`, `universe`, `eval`,
//! `lexical_leaves`, `projection` }; `eval` -> `lexical_leaves`; every one of
//! them -> `buckets` (the candidate-bucket algebra) and `read` (the pinned
//! structural snapshot), which depend on nothing here.

mod buckets;
mod eval;
pub(crate) mod lexical_leaves;
mod lowering;
mod projection;
mod read;
mod route;
mod universe;
