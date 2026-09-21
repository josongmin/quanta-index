//! Repo-map query route: policy validation, then the pinned snapshot the
//! read view acquired.
//!
//! The `RepoMap` domain is acquired once by the view as a real immutable
//! handle; the route executes against `view.repo_map()` only — there is
//! no ambient store or registry left to consult after acquisition.

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
        if !view.identity().evidence_is_exact() {
            return Err(CoreError::Storage(format!(
                "repo map: the read view evidence is not exact for {}",
                view.domains()
            )));
        }
        let snapshot = view.repo_map()?;
        snapshot.query(request)
    }
}
