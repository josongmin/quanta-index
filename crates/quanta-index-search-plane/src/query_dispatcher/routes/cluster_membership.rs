//! Cluster-membership batch read route over the semantic searcher.

use quanta_index_contract::{
    ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1,
};
use quanta_index_core::{CoreError, QueryRouteV1, RequestBudgetV1};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::read_view::ReadViewRequestV1;

impl SearchPlaneDispatcher {
    pub fn cluster_membership_batch_read(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
        budget: &RequestBudgetV1,
    ) -> Result<ClusterMembershipBatchReadResponseV1, CoreError> {
        budget.checkpoint("cluster-membership:entry")?;
        request
            .validate_v1()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        let view = self.acquire_read_view(&ReadViewRequestV1::declare(
            "cluster membership read",
            QueryRouteV1::ClusterMembershipRead,
            None,
            &request.generation,
        ))?;
        let searcher = view.semantic()?;
        budget.checkpoint("cluster-membership:read")?;
        let outcome = searcher.cluster_membership_batch_read(request)?;
        outcome.validate_against_v1(request).map_err(|failure| {
            CoreError::InvalidContract(format!(
                "cluster membership batch read: searcher returned invalid authority: {failure}"
            ))
        })?;
        Ok(outcome)
    }
}
