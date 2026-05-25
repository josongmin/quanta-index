use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord};
use quanta_index_contract::{
    GenerationPin, GenerationSelector, HistoryDiffHunkUpsert, HistoryIngestBatch,
    HistoryQueryRequest, HistoryRefDelete, HistoryRefMutation, HistoryRefUpsert,
    HistoryTagMutation, ManifestGeneration, RepoId, RevisionId, SearchPlaneHistoryQueryResponse,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, TextQueryRequest, TextQuerySyntax,
};

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
    pub commits: Vec<CommitRecord>,
    pub refs: Vec<RefMutation>,
    pub tags: Vec<RefMutation>,
    pub diff_hunks: Vec<DiffHunkMutation>,
}

impl HistoryBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            commits: Vec::new(),
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        }
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
            other => Err(SdkError::Protocol(format!(
                "expected history receipt, got {}",
                QuantaIndex::ingest_response_kind(&other)
            ))),
        }
    }
}

impl crate::NamespaceQuery for HistoryNs {
    type QueryBuilder<'a> = HistoryQueryBuilder<'a>;

    fn query<'a>(client: &'a QuantaIndex) -> HistoryQueryBuilder<'a> {
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
    syntax: TextQuerySyntax,
    query_text: Option<String>,
    selection: Option<GenerationSelector>,
    top_k: Option<u32>,
}

impl<'a> HistoryQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            syntax: TextQuerySyntax::Native,
            query_text: None,
            selection: None,
            top_k: None,
        }
    }

    #[must_use]
    pub fn native(mut self, query_text: impl Into<String>) -> Self {
        self.syntax = TextQuerySyntax::Native;
        self.query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn sourcegraph(mut self, query_text: impl Into<String>) -> Self {
        self.syntax = TextQuerySyntax::Sourcegraph;
        self.query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn pinned(mut self, pin: GenerationPin) -> Self {
        self.selection = Some(GenerationSelector::Pinned(pin));
        self
    }

    #[must_use]
    pub fn active(mut self, repo_id: RepoId, revision_id: RevisionId) -> Self {
        self.selection = Some(GenerationSelector::Active {
            repo_id,
            revision_id,
        });
        self
    }

    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn execute(self) -> Result<SearchPlaneHistoryQueryResponse, SdkError> {
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage("history query text is required".to_string()))?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("history generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("history top_k is required".to_string()))?;
        let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection);
        let response = self
            .client
            .dispatch_query(SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
                text_query: TextQueryRequest {
                    syntax: self.syntax,
                    query_text,
                    generation,
                    generation_selector,
                    top_k,
                },
            }))?;
        match response {
            SearchPlaneQueryIpcResponse::History(results) => Ok(results),
            other => Err(SdkError::Protocol(format!(
                "expected history response, got {}",
                QuantaIndex::query_response_kind(&other)
            ))),
        }
    }
}
