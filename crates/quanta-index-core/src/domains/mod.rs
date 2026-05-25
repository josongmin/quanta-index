//! Domain modules.
//!
//! Cross-domain imports are forbidden by
//! `tools/ci/lint/lint-hexagonal-boundaries.py`, with the single exception that
//! `hybrid` may consume the public surfaces of `lexical` and `semantic` for
//! orchestration purposes (RRF, generation-coherence checks).

pub mod channel;
pub mod hybrid;
pub mod lexical;
pub mod repomap;
pub mod semantic;
pub mod structural;
