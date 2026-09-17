//! Structural query route, split by responsibility.
//!
//! Dependency direction: `route` -> { `lowering`, `universe`, `eval`,
//! `lexical_leaves`, `projection` }; `eval` -> `lexical_leaves`; every one of
//! them -> `buckets` (the candidate-bucket algebra), which depends on nothing
//! here.

mod buckets;
mod eval;
pub(crate) mod lexical_leaves;
mod lowering;
mod projection;
mod route;
mod universe;
