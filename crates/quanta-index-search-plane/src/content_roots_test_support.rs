//! Test doubles for the semantic content roots an activation names (QI-BB-028).
//!
//! Deterministic roots per generation, and a roots port that reports
//! exactly those, so fixtures built with [`roots_for_generation`] validate
//! against [`generation_keyed_content_roots`] and any other roots are
//! refused.

use std::sync::Arc;

use quanta_index_contract::{GenerationSnapshot, SemanticContentRootsV1};
use quanta_index_core::{CoreError, SemanticContentRootsPort};

/// The roots a test generation "sealed": derived from its number alone.
pub(crate) fn roots_for_generation(generation: u64) -> SemanticContentRootsV1 {
    SemanticContentRootsV1 {
        row_root_digest: format!("sha256:{generation:0>64x}"),
        membership_root_digest: format!("sha256:{:0>64x}", generation.saturating_add(0x1000)),
    }
}

/// A roots port that reports [`roots_for_generation`] of the snapshot's
/// generation: the physical truth every fixture agrees with.
struct GenerationKeyedContentRoots;

impl SemanticContentRootsPort for GenerationKeyedContentRoots {
    fn sealed_content_roots(
        &self,
        sealed: &GenerationSnapshot,
    ) -> Result<SemanticContentRootsV1, CoreError> {
        Ok(roots_for_generation(sealed.manifest_generation.get()))
    }
}

pub(crate) fn generation_keyed_content_roots() -> Arc<dyn SemanticContentRootsPort + Send + Sync> {
    Arc::new(GenerationKeyedContentRoots)
}
