use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord};
use quanta_index_contract::{
    GenerationPin, GenerationSelector, HistoryDiffHunkUpsert, HistoryIngestBatch,
    HistoryQueryRequest, HistoryRefDelete, HistoryRefMutation, HistoryRefUpsert,
    HistoryTagMutation, ManifestGeneration, RepoId, RevisionId, SearchPlaneHistoryQueryResponse,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{BatchReceipt, QuantaIndex, SdkError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefMutation {
    Upsert { name: String, sha: CommitSha },
    Delete { name: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunkMutation {
    pub commit_sha: CommitSha,
    pub file_path: String,
    pub record: DiffHunkRecord,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub manifest_digest: Option<String>,
    pub batch_digest: String,
    pub commits: Vec<CommitRecord>,
    pub refs: Vec<RefMutation>,
    pub tags: Vec<RefMutation>,
    pub diff_hunks: Vec<DiffHunkMutation>,
}

impl HistoryBatch {
    #[must_use]
    pub fn new(
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
        batch_digest: impl Into<String>,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            manifest_digest: None,
            batch_digest: batch_digest.into(),
            commits: Vec::new(),
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        }
    }

    #[must_use]
    pub fn manifest_digest(mut self, manifest_digest: impl Into<String>) -> Self {
        self.manifest_digest = Some(manifest_digest.into());
        self
    }

    #[must_use]
    pub fn commit(mut self, record: CommitRecord) -> Self {
        self.commits.push(record);
        self
    }

    #[must_use]
    pub fn ref_upsert(mut self, name: impl Into<String>, sha: CommitSha) -> Self {
        self.refs.push(RefMutation::Upsert {
            name: name.into(),
            sha,
        });
        self
    }

    #[must_use]
    pub fn ref_delete(mut self, name: impl Into<String>) -> Self {
        self.refs.push(RefMutation::Delete { name: name.into() });
        self
    }

    #[must_use]
    pub fn tag_upsert(mut self, name: impl Into<String>, sha: CommitSha) -> Self {
        self.tags.push(RefMutation::Upsert {
            name: name.into(),
            sha,
        });
        self
    }

    #[must_use]
    pub fn tag_delete(mut self, name: impl Into<String>) -> Self {
        self.tags.push(RefMutation::Delete { name: name.into() });
        self
    }

    #[must_use]
    pub fn diff_hunk(
        mut self,
        commit_sha: CommitSha,
        file_path: impl Into<String>,
        record: DiffHunkRecord,
    ) -> Self {
        self.diff_hunks.push(DiffHunkMutation {
            commit_sha,
            file_path: file_path.into(),
            record,
        });
        self
    }
}

pub struct HistoryNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> HistoryNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> HistoryQueryBuilder<'a> {
        <HistoryNs as crate::NamespaceQuery>::query(self.client)
    }

    pub fn publish(&self, batch: &HistoryBatch) -> Result<BatchReceipt, SdkError> {
        <HistoryNs as crate::NamespaceIngest>::publish(self.client, batch)
    }
}

pub struct HistoryNs;

impl crate::NamespaceIngest for HistoryNs {
    type Batch = HistoryBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &HistoryBatch) -> Result<BatchReceipt, SdkError> {
        let wire = HistoryIngestBatch {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            manifest_digest: batch.manifest_digest.clone(),
            batch_digest: batch.batch_digest.clone(),
            commits: batch.commits.clone(),
            refs: batch.refs.iter().map(map_ref_mutation).collect(),
            tags: batch.tags.iter().map(map_tag_mutation).collect(),
            diff_hunks: batch
                .diff_hunks
                .iter()
                .map(|mutation| HistoryDiffHunkUpsert {
                    commit_sha: mutation.commit_sha,
                    file_path: mutation.file_path.clone().into_boxed_str(),
                    record: mutation.record.clone(),
                })
                .collect(),
        };
        let response =
            client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishHistoryBatch(wire))?;
        match response {
            SearchPlaneIngestIpcResponse::HistoryReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::LexicalReceipt(_)
            | SearchPlaneIngestIpcResponse::SemanticReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "history receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }
}

impl crate::NamespaceQuery for HistoryNs {
    type QueryBuilder<'a> = HistoryQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> HistoryQueryBuilder<'_> {
        HistoryQueryBuilder::new(client)
    }
}

fn map_ref_mutation(mutation: &RefMutation) -> HistoryRefMutation {
    match mutation {
        RefMutation::Upsert { name, sha } => HistoryRefMutation::Upsert(HistoryRefUpsert {
            name: name.clone().into_boxed_str(),
            sha: *sha,
        }),
        RefMutation::Delete { name } => HistoryRefMutation::Delete(HistoryRefDelete {
            name: name.clone().into_boxed_str(),
        }),
    }
}

fn map_tag_mutation(mutation: &RefMutation) -> HistoryTagMutation {
    map_ref_mutation(mutation)
}

pub struct HistoryQueryBuilder<'a> {
    client: &'a QuantaIndex,
    state: TextQueryBuilderState,
}

impl<'a> HistoryQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: TextQueryBuilderState::new(),
        }
    }

    #[must_use]
    pub fn native(mut self, query_text: impl Into<String>) -> Self {
        self.state.syntax = TextQuerySyntax::Native;
        self.state.query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn sourcegraph(mut self, query_text: impl Into<String>) -> Self {
        self.state.syntax = TextQuerySyntax::Sourcegraph;
        self.state.query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn pinned(mut self, pin: GenerationPin) -> Self {
        self.state.selection = Some(GenerationSelector::Pinned(pin));
        self
    }

    #[must_use]
    pub fn active(mut self, repo_id: RepoId, revision_id: RevisionId) -> Self {
        self.state.selection = Some(GenerationSelector::Active {
            repo_id,
            revision_id,
        });
        self
    }

    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.state.top_k = Some(top_k);
        self
    }

    pub fn execute(self) -> Result<SearchPlaneHistoryQueryResponse, SdkError> {
        let text_query = self.state.build_request("history")?;
        let response = self
            .client
            .dispatch_query(SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
                text_query,
            }))?;
        match response {
            SearchPlaneQueryIpcResponse::History(results) => Ok(results),
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "history response",
                QuantaIndex::query_response_kind(&other),
            )),
        }
    }
}
