//! Request-bound Native `RepoMap` rank client.

use std::{collections::BTreeSet, path::Path};

use quanta_index_contract::{
    RepoMapNodeRef,
    index::{IndexInstanceId, ViewSelection},
    native_protocol::{
        NativeIndexRequest, NativeIndexRequestEnvelope, NativeIndexResponse, NativeRepoMapQuery,
        NativeRepoMapResult,
    },
};
use quanta_index_ipc::RequestBudgetV1;

use super::{NativeUdsClient, NativeUdsError, allocate_request_id, selection_matches};

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Return ranked `RepoMap` hits from one selected, certified Native view.
    pub fn query_repomap(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: NativeRepoMapQuery,
        budget: &RequestBudgetV1,
    ) -> Result<NativeRepoMapResult, NativeUdsError> {
        if !query.valid_native_rank_query() {
            return Err(NativeUdsError::Protocol(
                "RepoMap rank query exceeds request bounds".into(),
            ));
        }
        let top_k = query.top_k;
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::QueryRepoMap {
                selection,
                instance,
                query,
            },
        };
        let NativeIndexResponse::RepoMapRanked(result) =
            self.request_wire(request_id, &envelope, budget)?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected RepoMap rank reply".into(),
            ));
        };
        if !selection_matches(selection, result.view)
            || result.instance != instance
            || !result.exhaustive
            || usize::try_from(top_k).map_or(true, |limit| result.hits.len() > limit)
        {
            return Err(NativeUdsError::Protocol(
                "RepoMap rank reply differs from selected request".into(),
            ));
        }
        let mut identities = BTreeSet::new();
        for hit in &result.hits {
            let raw_identity = match &hit.subject {
                RepoMapNodeRef::File(id) => id.as_str(),
                RepoMapNodeRef::Symbol(id) => id.as_str(),
                RepoMapNodeRef::Module(_) | RepoMapNodeRef::Chunk(_) => {
                    return Err(NativeUdsError::Protocol(
                        "RepoMap rank reply contains an unsupported subject".into(),
                    ));
                }
            };
            if !identities.insert(raw_identity) {
                return Err(NativeUdsError::Protocol(
                    "RepoMap rank reply repeats a raw subject identity".into(),
                ));
            }
        }
        Ok(result)
    }
}
