//! Repo-map query route: policy validation then delegation to the port.
//!
//! The `RepoMap` domain is served by the port the composition root wired;
//! the route's read view declares that domain and nothing else, so no
//! track handle or auxiliary snapshot is opened for a `RepoMap` query.

use quanta_index_contract::{GenerationPin, RepoMapQueryRequest, RepoMapQueryResponse};
use quanta_index_core::{
    CoreError, QueryRouteV1, ReadDomainV1, RepoMapPolicy, RequestBudgetV1, RequiredDomainsV1,
};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::read_view::ReadViewRequestV1;

impl SearchPlaneDispatcher {
    pub(crate) fn repo_map(
        &self,
        request: RepoMapQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<RepoMapQueryResponse, CoreError> {
        budget.checkpoint("repo-map:entry")?;
        RepoMapPolicy::validate_query(&request)?;
        let pin = GenerationPin::new(
            request.repo_id.clone(),
            request.revision_id.clone(),
            request.manifest_generation,
        );
        let view = self.acquire_read_view(
            &ReadViewRequestV1::declare("repo map", QueryRouteV1::RepoMap, None, &pin),
            budget,
        )?;
        if view.domains() != RequiredDomainsV1::of(ReadDomainV1::RepoMap) {
            return Err(CoreError::Storage(format!(
                "repo map: the read view declared {} for a route that reads the RepoMap store only",
                view.domains()
            )));
        }
        self.repo_map_query.query(request)
    }
}
