use std::marker::PhantomData;
use std::sync::Arc;

use quanta_index_contract::{
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
};
use quanta_index_ipc::{IpcDispatcher, RequestBudgetV1};
use quanta_index_search_plane::{
    SearchPlaneControlDispatcher, SearchPlaneDispatcher, SearchPlaneIngestDispatcher,
};

pub(super) trait PlaneDispatch<Request, Response>: Send + Sync {
    fn dispatch(&self, request: Request, budget: &RequestBudgetV1) -> Response;
}

impl PlaneDispatch<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>
    for SearchPlaneDispatcher
{
    fn dispatch(
        &self,
        request: SearchPlaneQueryIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        SearchPlaneDispatcher::dispatch(self, request, budget)
    }
}

impl PlaneDispatch<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>
    for SearchPlaneControlDispatcher
{
    fn dispatch(
        &self,
        request: SearchPlaneControlIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneControlIpcResponse {
        SearchPlaneControlDispatcher::dispatch(self, request, budget)
    }
}

impl PlaneDispatch<SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse>
    for SearchPlaneIngestDispatcher
{
    fn dispatch(
        &self,
        request: SearchPlaneIngestIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneIngestIpcResponse {
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
    fn dispatch(&self, request: Request, budget: &RequestBudgetV1) -> Response {
        self.inner.dispatch(request, budget)
    }
}
