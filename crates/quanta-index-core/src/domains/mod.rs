//! Bounded contexts for the search-plane application core.
//!
//! Domains must not import sibling domains. Cross-domain orchestration belongs in
//! `quanta-index-searchd::app` only.

pub mod bundle_ingest;
pub mod generation;
pub mod materialization;
pub mod query;
