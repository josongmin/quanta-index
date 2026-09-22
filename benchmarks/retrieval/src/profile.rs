//! Benchmark embedder selection and provenance, bound to provider-owned identities.

use crate::{BenchError, BenchResult};

pub const DEFAULT_EMBEDDER: &str = "potion-code";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbedderProfile {
    pub selector: &'static str,
    pub model_id: &'static str,
    pub model_revision: &'static str,
}

impl EmbedderProfile {
    /// Only profiles with an independently known identity can produce a
    /// benchmark record. A user-supplied model label is not evidence.
    pub fn resolve(selector: Option<&str>) -> BenchResult<Self> {
        match selector.unwrap_or(DEFAULT_EMBEDDER) {
            DEFAULT_EMBEDDER => Ok(Self {
                selector: DEFAULT_EMBEDDER,
                model_id: quanta_index_embed::POTION_CODE_MODEL_ID,
                model_revision: quanta_index_embed::POTION_CODE_MODEL_REVISION,
            }),
            "hash-dev" => Ok(Self {
                selector: "hash-dev",
                model_id: quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_ID,
                model_revision: quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_REVISION,
            }),
            other => Err(BenchError::Config(format!(
                "embedder {other:?} has no verified benchmark provenance; expected potion-code or hash-dev"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_the_pinned_potion_code_provider() {
        let profile = EmbedderProfile::resolve(None).expect("default profile");
        assert_eq!(profile.selector, "potion-code");
        assert_eq!(profile.model_id, quanta_index_embed::POTION_CODE_MODEL_ID);
        assert_eq!(
            profile.model_revision,
            quanta_index_embed::POTION_CODE_MODEL_REVISION
        );
    }

    #[test]
    fn hash_requires_explicit_development_selector() {
        assert_eq!(
            EmbedderProfile::resolve(Some("hash-dev"))
                .expect("dev profile")
                .selector,
            "hash-dev"
        );
        assert!(EmbedderProfile::resolve(Some("hash")).is_err());
        assert!(EmbedderProfile::resolve(Some("openai")).is_err());
    }
}
