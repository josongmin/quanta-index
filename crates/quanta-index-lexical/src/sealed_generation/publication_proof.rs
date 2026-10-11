//! Attempt-scoped mutation planning state. Semantic proof is owned by the seal.

#[derive(Default)]
pub(crate) struct PublicationProofs {
    pub(crate) file_plan: crate::file_authority::FileAuthorityPlanCache,
}
