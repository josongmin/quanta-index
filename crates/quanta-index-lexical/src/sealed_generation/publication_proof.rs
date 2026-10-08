//! Opaque, bounded proof custody for a single publication attempt.

use quanta_index_contract::GenerationSnapshot;
use quanta_index_core::SealedArtifactCommitmentV1;

use crate::file_authority::ValidatedFileAuthorityProof;

/// One F15 root commitment, one text-manifest commitment, and one mutation plan.
///
/// No decoded source bodies, normalized strings, posting maps, or query handles survive
/// a validation call. Changing identity/manifest/policy requires a new proof.
#[derive(Default)]
pub(crate) struct PublicationProofs {
    pub(crate) file: Option<ValidatedFileAuthorityProof>,
    pub(crate) text: Option<ValidatedTextAuthorityProof>,
    pub(crate) file_plan: crate::file_authority::FileAuthorityPlanCache,
}

pub(crate) struct ValidatedTextAuthorityProof {
    identity: GenerationSnapshot,
    manifest: SealedArtifactCommitmentV1,
}

impl ValidatedTextAuthorityProof {
    /// Called only after all committed shards have been decoded and checked.
    pub(super) fn after_full_verification(
        identity: &GenerationSnapshot,
        manifest: &SealedArtifactCommitmentV1,
    ) -> Self {
        Self {
            identity: identity.clone(),
            manifest: manifest.clone(),
        }
    }

    pub(super) fn matches(
        &self,
        identity: &GenerationSnapshot,
        manifest: &SealedArtifactCommitmentV1,
    ) -> bool {
        self.identity == *identity && self.manifest == *manifest
    }
}
