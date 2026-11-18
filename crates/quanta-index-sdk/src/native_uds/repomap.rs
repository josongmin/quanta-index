//! Request-bound Native `RepoMap` rank client.

use std::{collections::BTreeSet, path::Path};

use quanta_index_contract::{
    RepoId, RepoMapNodeRef, RevisionId,
    index::{IndexInstanceId, ViewSelection},
    native_protocol::{
        NativeImportedRepoMapQueryV1, NativeImportedRepoMapResultV1, NativeIndexRequest,
        NativeIndexRequestEnvelope, NativeIndexResponse, NativeRepoMapQuery, NativeRepoMapResult,
    },
};
use quanta_index_ipc::RequestBudgetV1;

use super::{NativeUdsClient, NativeUdsError, allocate_request_id, selection_matches};

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Resolve the selected BOOT route and rank its exact imported RepoMap view.
    pub fn query_protected_repomap_rank_v1(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        query: NativeRepoMapQuery,
        budget: &RequestBudgetV1,
    ) -> Result<NativeRepoMapResult, NativeUdsError> {
        let resolved = self.resolve_protected_repomap_head_v1(repo_id, revision_id, budget)?;
        self.query_repomap(
            ViewSelection::Exact(resolved.root.view.identity),
            resolved.instance,
            query,
            budget,
        )
    }

    /// Resolve the selected BOOT route and query its exact protected graph.
    pub fn query_protected_imported_repomap_v1(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        query: NativeImportedRepoMapQueryV1,
        budget: &RequestBudgetV1,
    ) -> Result<NativeImportedRepoMapResultV1, NativeUdsError> {
        let resolved = self.resolve_protected_repomap_head_v1(repo_id, revision_id, budget)?;
        self.query_imported_repomap(ViewSelection::Exact(resolved.root.view.identity),
            resolved.instance, query, budget)
    }

    /// Query one imported, exact Legacy compiler graph member in a pinned view.
    pub fn query_imported_repomap(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: NativeImportedRepoMapQueryV1,
        budget: &RequestBudgetV1,
    ) -> Result<NativeImportedRepoMapResultV1, NativeUdsError> {
        if !query.valid_v1() {
            return Err(NativeUdsError::Protocol(
                "imported RepoMap query exceeds request bounds".into(),
            ));
        }
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::QueryImportedRepoMap {
                selection,
                instance,
                query,
            },
        };
        let NativeIndexResponse::ImportedRepoMap(result) =
            self.request_wire(request_id, &envelope, budget, Some(&envelope.payload))?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected imported RepoMap reply".into(),
            ));
        };
        let NativeIndexRequest::QueryImportedRepoMap { query, .. } = &envelope.payload else {
            return Err(NativeUdsError::Protocol(
                "imported RepoMap request context changed".into(),
            ));
        };
        if !selection_matches(selection, result.view)
            || result.instance != instance
            || result.source != query.source
            || result.direction != query.direction
            || usize::try_from(query.max_edges).map_or(true, |max| result.edges.len() > max)
            || result.next.is_some() == result.exhaustive
            || result.next.is_some_and(|next| {
                next.direction != query.direction
                    || result.edges.last().map(|edge| edge.cursor_start) != Some(next.start)
                    || query.after.is_some_and(|after| {
                        next.member != after.member
                            || next.payload_sha256 != after.payload_sha256
                            || next.source_digest != after.source_digest
                            || next.start <= after.start
                    })
            })
            || result
                .edges
                .windows(2)
                .any(|pair| pair[0].cursor_start >= pair[1].cursor_start)
            || result
                .node
                .as_ref()
                .is_some_and(|node| node.identity != query.source)
            || result.edges.iter().any(|edge| match query.direction {
                quanta_index_contract::native_protocol::NativeGraphDirection::Outgoing => {
                    edge.source != query.source
                }
                quanta_index_contract::native_protocol::NativeGraphDirection::Incoming => {
                    edge.target != query.source
                }
            })
        {
            return Err(NativeUdsError::Protocol(
                "imported RepoMap reply differs from selected request".into(),
            ));
        }
        Ok(result)
    }

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
            self.request_wire(request_id, &envelope, budget, Some(&envelope.payload))?
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
