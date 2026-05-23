use quanta_index_contract::PublishedSearchGenerationActivateRequest;

use crate::CoreError;

pub struct ActivationPolicy;

impl ActivationPolicy {
    pub fn validate_request(
        request: &PublishedSearchGenerationActivateRequest,
    ) -> Result<(), CoreError> {
        if !request.lexical_ready {
            return Err(CoreError::NotReady(
                "lexical_ready must be true before activation".into(),
            ));
        }

        if !request.semantic_ready {
            return Err(CoreError::NotReady(
                "semantic_ready must be true before activation".into(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{
        GenerationId, ManifestGeneration, PublishedGenerationSet,
        PublishedSearchGenerationActivateRequest, RepoId, RevisionId,
    };

    use super::ActivationPolicy;
    use crate::CoreError;

    #[test]
    fn rejects_lexical_not_ready() {
        let result = ActivationPolicy::validate_request(&sample_request(false, true));
        assert!(
            matches!(result, Err(CoreError::NotReady(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn rejects_semantic_not_ready() {
        let result = ActivationPolicy::validate_request(&sample_request(true, false));
        assert!(
            matches!(result, Err(CoreError::NotReady(_))),
            "unexpected result: {result:?}"
        );
    }

    #[test]
    fn accepts_fully_ready_request() {
        let result = ActivationPolicy::validate_request(&sample_request(true, true));
        assert!(result.is_ok(), "unexpected result: {result:?}");
    }

    fn sample_request(
        lexical_ready: bool,
        semantic_ready: bool,
    ) -> PublishedSearchGenerationActivateRequest {
        PublishedSearchGenerationActivateRequest {
            generation: PublishedGenerationSet {
                repo_id: RepoId::new("repo"),
                revision_id: RevisionId::new("rev"),
                manifest_generation: ManifestGeneration::new(7),
                lexical_generation: GenerationId::new(10),
                symbol_generation: GenerationId::new(11),
                structural_generation: None,
                history_generation: None,
                semantic_generation: None,
                metadata_generation: None,
            },
            lexical_ready,
            semantic_ready,
            active_at_ms: 42,
        }
    }
}
