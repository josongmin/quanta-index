use quanta_index_contract::{
    PublishedSearchBundleDeltaApplyRequest, PublishedSearchBundleDeltaApplyResponse,
    PublishedSearchBundlePrepareRequest, PublishedSearchBundlePrepareResponse,
};

use crate::CoreError;

pub trait PublishedSearchBundlePreparePort {
    fn prepare_bundle(
        &mut self,
        request: PublishedSearchBundlePrepareRequest,
    ) -> Result<PublishedSearchBundlePrepareResponse, CoreError>;
}

pub trait PublishedSearchBundleDeltaApplyPort {
    fn apply_bundle_delta(
        &mut self,
        request: PublishedSearchBundleDeltaApplyRequest,
    ) -> Result<PublishedSearchBundleDeltaApplyResponse, CoreError>;
}
