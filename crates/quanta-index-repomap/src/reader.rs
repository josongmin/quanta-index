use quanta_index_contract::{RepoMapQueryRequestV1, RepoMapQueryResponseV1};
use quanta_index_core::CoreError;

use crate::store::RepoMapGenerationStore;

pub struct RepoMapPinnedReader;

impl RepoMapPinnedReader {
    pub fn read_query_snapshot(
        store: &RepoMapGenerationStore,
        request: &RepoMapQueryRequestV1,
    ) -> Result<RepoMapQueryResponseV1, CoreError> {
        store.read_query_snapshot(request)
    }
}
