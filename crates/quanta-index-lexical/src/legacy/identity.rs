use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::CoreError;
pub(crate) fn validate_lexical_sealed_identity(
    observed: &GenerationSnapshot,
    candidate: &GenerationSnapshot,
) -> Result<(), CoreError> {
    if observed != candidate {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
            message: format!(
                "lexical: durable generation identity mismatch for repo={} revision={} generation={}",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation.get(),
            ),
        });
    }
    Ok(())
}
