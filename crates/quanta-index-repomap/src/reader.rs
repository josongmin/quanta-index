use quanta_index_contract::{RepoMapQueryRequest, RepoMapQueryResponse};
use quanta_index_core::CoreError;

use crate::store::RepoMapGenerationStore;

pub struct RepoMapPinnedReader;

impl RepoMapPinnedReader {
    pub fn read_query_snapshot(
        store: &RepoMapGenerationStore,
        request: &RepoMapQueryRequest,
    ) -> Result<RepoMapQueryResponse, CoreError> {
        store.read_query_snapshot(request)
    }
}
