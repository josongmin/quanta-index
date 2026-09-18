//! Domain modules.
//!
//! Cross-domain imports are forbidden by
//! `tools/ci/lint/lint-hexagonal-boundaries.py`, with the single exception that
//! `hybrid` may consume the public surfaces of `lexical` and `semantic` for
//! orchestration purposes (RRF, generation-coherence checks).

pub mod auxiliary;
pub mod generation;
pub mod hybrid;
pub mod idempotency;
pub mod integrity;
pub mod lexical;
pub mod observability;
pub mod read_view;
pub mod repomap;
pub mod semantic;
pub mod structural;
