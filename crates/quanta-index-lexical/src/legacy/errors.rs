//! Canonical persisted normalizer refusal, independent of the engine.
use crate::normalize::{TEXT_NORMALIZER_VERSION, TextNormalizerVersion};
use quanta_index_core::CoreError;
use std::path::Path;
pub(crate) fn normalizer_unsupported(path: &Path, built_with: TextNormalizerVersion) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationNormalizerUnsupported,
        message: format!(
            "lexical: sealed generation {} was built under text normalizer {built_with} (this build runs {TEXT_NORMALIZER_VERSION}); it must be rebuilt, never served with mismatched text semantics",
            path.display()
        ),
    }
}

pub(crate) fn sidecar_corrupt(generation_dir: &Path, name: &str, reason: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
        message: format!(
            "lexical: generation {} does not match its manifest: {name}: {reason}",
            generation_dir.display()
        ),
    }
}
