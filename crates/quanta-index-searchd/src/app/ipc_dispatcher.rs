use std::marker::PhantomData;
use std::sync::Arc;

use quanta_index_contract::{
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse,
};
use quanta_index_ipc::IpcDispatcher;
use quanta_index_search_plane::{SearchPlaneControlDispatcher, SearchPlaneDispatcher};

pub(super) trait PlaneDispatch<Request, Response>: Send + Sync {
    fn dispatch(&self, request: Request) -> Response;
}

impl PlaneDispatch<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>
    for SearchPlaneDispatcher
{
    fn dispatch(&self, request: SearchPlaneQueryIpcRequest) -> SearchPlaneQueryIpcResponse {
        SearchPlaneDispatcher::dispatch(self, request)
    }
}

impl PlaneDispatch<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>
    for SearchPlaneControlDispatcher
{
    fn dispatch(&self, request: SearchPlaneControlIpcRequest) -> SearchPlaneControlIpcResponse {
        SearchPlaneControlDispatcher::dispatch(self, request)
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
    fn dispatch(&self, request: Request) -> Response {
        self.inner.dispatch(request)
    }
}
