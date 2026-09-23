use std::marker::PhantomData;
use std::sync::Arc;

use quanta_index_contract::{
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneErrorCodeV2,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse,
};
use quanta_index_ipc::{IpcDispatcher, RequestBudgetV1};
use quanta_index_search_plane::{
    SearchPlaneControlDispatcher, SearchPlaneDispatcher, SearchPlaneIngestDispatcher,
};

pub(super) trait PlaneDispatch<Request, Response>: Send + Sync {
    /// Closed diagnostic projection, not a second wire-route authority.
    fn event_route(request: &Request) -> &'static str;

    /// The owning response enum distinguishes typed refusal from success.
    fn event_error(response: &Response) -> Option<SearchPlaneErrorCodeV2>;

    /// Handle one request. The transport's kernel-derived context is
    /// mandatory: a plane that does not authorize on it must still accept
    /// it explicitly instead of relying on an ambient default.
    fn dispatch(
        &self,
        context: &quanta_index_ipc::DispatchContextV1,
        request: Request,
        budget: &RequestBudgetV1,
    ) -> Response;
}

impl PlaneDispatch<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>
    for SearchPlaneDispatcher
{
    fn event_route(request: &SearchPlaneQueryIpcRequest) -> &'static str {
        match request {
            SearchPlaneQueryIpcRequest::ResolveActiveGeneration(_) => "query.resolve_active",
            SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(_) => "query.resolve_lexical",
            SearchPlaneQueryIpcRequest::Text(_) => "query.text",
            SearchPlaneQueryIpcRequest::Symbol(_) => "query.symbol",
            SearchPlaneQueryIpcRequest::Semantic(_) => "query.semantic",
            SearchPlaneQueryIpcRequest::Hybrid(_) => "query.hybrid",
            SearchPlaneQueryIpcRequest::HybridSeed(_) => "query.hybrid_seed",
            SearchPlaneQueryIpcRequest::History(_) => "query.history",
            SearchPlaneQueryIpcRequest::RuntimeMetadata(_) => "query.runtime_metadata",
            SearchPlaneQueryIpcRequest::Structural(_) => "query.structural",
            SearchPlaneQueryIpcRequest::RepoMapQuery(_) => "query.repo_map",
            SearchPlaneQueryIpcRequest::Explain(_) => "query.explain",
            SearchPlaneQueryIpcRequest::ClusterMembershipRead(_) => "query.cluster_membership",
        }
    }

    fn event_error(response: &SearchPlaneQueryIpcResponse) -> Option<SearchPlaneErrorCodeV2> {
        match response {
            SearchPlaneQueryIpcResponse::Error(error) => Some(error.code),
            SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
            | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
            | SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => None,
        }
    }

    fn dispatch(
        &self,
        _context: &quanta_index_ipc::DispatchContextV1,
        request: SearchPlaneQueryIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        // The query plane reads no principal: it exposes no capability
        // beyond serving a query the admission layer already admitted.
        // It no longer stamps the envelope's request id either (W10-R2):
        // the route builders set the explanation's request id from the
        // budget correlation the server injected, so the adapter passing
        // the dispatcher answer through untouched IS the correlation.
        SearchPlaneDispatcher::dispatch(self, request, budget)
    }
}

impl PlaneDispatch<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>
    for SearchPlaneControlDispatcher
{
    fn event_route(request: &SearchPlaneControlIpcRequest) -> &'static str {
        match request {
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(_) => {
                "control.activate_corpus"
            }
            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(_) => {
                "control.rollback_corpus"
            }
            SearchPlaneControlIpcRequest::RepoMapActivate(_) => "control.activate_repo_map_v1",
            SearchPlaneControlIpcRequest::RepoMapActivateV2(_) => "control.activate_repo_map_v2",
            SearchPlaneControlIpcRequest::CurrentGeneration(_) => "control.current_generation",
            SearchPlaneControlIpcRequest::GenerationStatus(_) => "control.generation_status",
            SearchPlaneControlIpcRequest::MetricsSnapshot(_) => "control.metrics",
            SearchPlaneControlIpcRequest::QuarantineInventory(_) => "control.quarantine_inventory",
            SearchPlaneControlIpcRequest::QuarantineDiscard(_) => "control.quarantine_discard",
            SearchPlaneControlIpcRequest::ProcessReadiness(_) => "control.process_readiness",
        }
    }

    fn event_error(response: &SearchPlaneControlIpcResponse) -> Option<SearchPlaneErrorCodeV2> {
        match response {
            SearchPlaneControlIpcResponse::Error(error) => Some(error.code),
            SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_) => None,
        }
    }

    fn dispatch(
        &self,
        context: &quanta_index_ipc::DispatchContextV1,
        request: SearchPlaneControlIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneControlIpcResponse {
        // The control plane authorizes every request against the
        // kernel-derived peer credential in the context.
        SearchPlaneControlDispatcher::dispatch_authorized(self, context, request, budget)
    }
}

impl PlaneDispatch<SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse>
    for SearchPlaneIngestDispatcher
{
    fn event_route(request: &SearchPlaneIngestIpcRequest) -> &'static str {
        match request {
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_) => "ingest.search_corpus",
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(_) => "ingest.history",
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(_) => {
                "ingest.repo_commit_recency"
            }
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(_) => "ingest.repo_topic",
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(_) => "ingest.file_ownership",
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(_) => {
                "ingest.file_contributor"
            }
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(_) => "ingest.dirty",
            SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(_) => "ingest.runtime_catalog",
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(_) => "ingest.structural",
            SearchPlaneIngestIpcRequest::PublishRepoMapBundle(_) => "ingest.repo_map_v1",
            SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(_) => "ingest.repo_map_v2",
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(_) => "ingest.repo_meta",
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(_) => {
                "ingest.repo_description"
            }
        }
    }

    fn event_error(response: &SearchPlaneIngestIpcResponse) -> Option<SearchPlaneErrorCodeV2> {
        match response {
            SearchPlaneIngestIpcResponse::Error(error) => Some(error.code),
            SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => None,
        }
    }

    fn dispatch(
        &self,
        _context: &quanta_index_ipc::DispatchContextV1,
        request: SearchPlaneIngestIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneIngestIpcResponse {
        // Ingest authorization is the socket access policy's; the payload
        // binding is the digest stamp, not a principal.
        SearchPlaneIngestDispatcher::dispatch(self, request, budget)
    }
}

/// Transport adapter that projects application-layer dispatchers onto the IPC
/// server trait expected by `quanta-index-ipc`.
pub(super) struct SearchPlaneIpcDispatcher<Request, Response, D>
where
    D: PlaneDispatch<Request, Response>,
{
    inner: Arc<D>,
    marker: PhantomData<fn(Request) -> Response>,
}

pub(super) type SearchPlaneQueryIpcAdapter = SearchPlaneIpcDispatcher<
    SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse,
    SearchPlaneDispatcher,
>;

pub(super) type SearchPlaneControlIpcAdapter = SearchPlaneIpcDispatcher<
    SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponse,
    SearchPlaneControlDispatcher,
>;

pub(super) type SearchPlaneIngestIpcAdapter = SearchPlaneIpcDispatcher<
    SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse,
    SearchPlaneIngestDispatcher,
>;

impl<Request, Response, D> SearchPlaneIpcDispatcher<Request, Response, D>
where
    D: PlaneDispatch<Request, Response>,
{
    #[must_use]
    pub(super) fn new(inner: Arc<D>) -> Self {
        Self {
            inner,
            marker: PhantomData,
        }
    }
}

impl<Request, Response, D> IpcDispatcher<Request, Response>
    for SearchPlaneIpcDispatcher<Request, Response, D>
where
    D: PlaneDispatch<Request, Response>,
{
    fn dispatch(
        &self,
        context: &quanta_index_ipc::DispatchContextV1,
        request: Request,
        budget: &RequestBudgetV1,
    ) -> Response {
        let route = D::event_route(&request);
        context.record_event_v1(quanta_index_ipc::RequestEventStageV1::BackendStarted);
        let response = self.inner.dispatch(context, request, budget);
        context.record_event_v1(quanta_index_ipc::RequestEventStageV1::BackendOutcome {
            route,
            error: D::event_error(&response),
        });
        context.record_event_v1(quanta_index_ipc::RequestEventStageV1::BackendReturned);
        response
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use quanta_index_contract::{
        MetricsSnapshotRequest, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
        SearchPlaneIngestIpcResponse, SearchPlaneIpcError, SearchPlaneQueryIpcResponse,
    };
    use quanta_index_ipc::{
        DispatchContextV1, IpcDispatcher, IpcPlane, IpcServerCounters, RequestBudgetV1,
        RequestEventSinkV1, RequestEventStageV1,
    };

    use super::{
        PlaneDispatch, SearchPlaneControlDispatcher, SearchPlaneDispatcher,
        SearchPlaneIngestDispatcher, SearchPlaneIpcDispatcher,
    };

    struct Echo;

    impl PlaneDispatch<u64, u64> for Echo {
        fn event_route(_request: &u64) -> &'static str {
            "test.echo"
        }

        fn event_error(_response: &u64) -> Option<quanta_index_contract::SearchPlaneErrorCodeV2> {
            None
        }

        fn dispatch(
            &self,
            context: &DispatchContextV1,
            request: u64,
            _budget: &RequestBudgetV1,
        ) -> u64 {
            assert_eq!(context.request_id.get(), 9);
            request + 1
        }
    }

    #[test]
    fn backend_adapter_uses_transport_identity_and_one_event_sink() {
        let counters = Arc::new(IpcServerCounters::for_plane("query"));
        let events: Arc<dyn RequestEventSinkV1> = counters.clone();
        let budget = RequestBudgetV1::unbounded();
        let context = DispatchContextV1 {
            request_id: NonZeroU64::new(9).expect("nonzero fixture ID"),
            plane: IpcPlane::Query,
            principal: None,
            owner_uid: 0,
            connection_id: 17,
            deadline: Instant::now() + Duration::from_secs(1),
            cancellation: budget.cancel_handle(),
            events,
            request_started: Instant::now(),
        };
        let adapter = SearchPlaneIpcDispatcher::<u64, u64, Echo>::new(Arc::new(Echo));
        assert_eq!(IpcDispatcher::dispatch(&adapter, &context, 4, &budget), 5);
        let tail = counters.recent_request_events_v1().expect("event tail");
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].stage, RequestEventStageV1::BackendStarted);
        assert_eq!(
            tail[1].stage,
            RequestEventStageV1::BackendOutcome {
                route: "test.echo",
                error: None,
            }
        );
        assert_eq!(tail[2].stage, RequestEventStageV1::BackendReturned);
        assert!(
            tail.iter()
                .all(|event| event.request_id.get() == 9 && event.connection_id == 17)
        );
    }

    #[test]
    fn route_and_typed_error_projection_uses_response_variants() {
        let request = SearchPlaneControlIpcRequest::MetricsSnapshot(MetricsSnapshotRequest);
        assert_eq!(
            <SearchPlaneControlDispatcher as PlaneDispatch<
                SearchPlaneControlIpcRequest,
                SearchPlaneControlIpcResponse,
            >>::event_route(&request),
            "control.metrics"
        );
        let error = SearchPlaneIpcError::overloaded(Duration::ZERO, 1);
        let code = error.code;
        assert_eq!(
            <SearchPlaneControlDispatcher as PlaneDispatch<
                SearchPlaneControlIpcRequest,
                SearchPlaneControlIpcResponse,
            >>::event_error(&SearchPlaneControlIpcResponse::Error(error.clone())),
            Some(code)
        );
        assert_eq!(
            <SearchPlaneDispatcher as PlaneDispatch<
                quanta_index_contract::SearchPlaneQueryIpcRequest,
                SearchPlaneQueryIpcResponse,
            >>::event_error(&SearchPlaneQueryIpcResponse::Error(error.clone())),
            Some(code)
        );
        assert_eq!(
            <SearchPlaneIngestDispatcher as PlaneDispatch<
                quanta_index_contract::SearchPlaneIngestIpcRequest,
                SearchPlaneIngestIpcResponse,
            >>::event_error(&SearchPlaneIngestIpcResponse::Error(error)),
            Some(code)
        );
    }
}
