//! Repo-map query route: policy validation then delegation to the port.

use quanta_index_contract::{RepoMapQueryRequest, RepoMapQueryResponse};
use quanta_index_core::{CoreError, RepoMapPolicy, RequestBudgetV1};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;

impl SearchPlaneDispatcher {
    pub(crate) fn repo_map(
        &self,
        request: RepoMapQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<RepoMapQueryResponse, CoreError> {
        budget.checkpoint("repo-map:entry")?;
        RepoMapPolicy::validate_query(&request)?;
        self.repo_map_query.query(request)
    }
}
