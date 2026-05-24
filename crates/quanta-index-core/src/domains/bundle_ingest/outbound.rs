use quanta_index_contract::{
    PublishedSearchBundleDeltaApplyRequest, PublishedSearchBundleDeltaApplyResponse,
    PublishedSearchBundlePrepareRequest, PublishedSearchBundlePrepareResponse,
};

use crate::CoreError;

/// Driven port: persist prepared bundle outbox rows.
pub trait PublishedSearchBundlePreparePort {
    fn prepare_bundle(
        &mut self,
        request: PublishedSearchBundlePrepareRequest,
    ) -> Result<PublishedSearchBundlePrepareResponse, CoreError>;
}

/// Driven port: apply bundle mutation deltas.
pub trait PublishedSearchBundleDeltaApplyPort {
    fn apply_bundle_delta(
        &mut self,
        request: PublishedSearchBundleDeltaApplyRequest,
    ) -> Result<PublishedSearchBundleDeltaApplyResponse, CoreError>;
}
