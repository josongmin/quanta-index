use quanta_index_contract::lex::DirtyRecord;
use quanta_index_contract::{
    ChunkId, DirtyDelete, DirtyIngestBatch, DirtyMutation, GenerationPin, GenerationSelector,
    ManifestGeneration, RepoId, RevisionId, RuntimeMetadataQueryRequest,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneRuntimeMetadataQueryResponse, TextQueryRequest,
    TextQuerySyntax,
};

use crate::{BatchReceipt, QuantaIndex, SdkError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirtyBatchMutation {
    Upsert(DirtyRecord),
    Delete { doc_id: ChunkId },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub entries: Vec<DirtyBatchMutation>,
}

impl DirtyBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            entries: Vec::new(),
        }
    }

    #[must_use]
    pub fn upsert(mut self, record: DirtyRecord) -> Self {
        self.entries.push(DirtyBatchMutation::Upsert(record));
        self
    }

    #[must_use]
    pub fn delete(mut self, doc_id: ChunkId) -> Self {
        self.entries.push(DirtyBatchMutation::Delete { doc_id });
        self
    }
}

pub struct RuntimeNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> RuntimeNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> RuntimeQueryBuilder<'a> {
        <RuntimeNs as crate::NamespaceQuery>::query(self.client)
    }

    pub fn publish_dirty(&self, batch: &DirtyBatch) -> Result<BatchReceipt, SdkError> {
        <RuntimeNs as crate::NamespaceIngest>::publish(self.client, batch)
    }
}

pub struct RuntimeNs;

impl crate::NamespaceIngest for RuntimeNs {
    type Batch = DirtyBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &DirtyBatch) -> Result<BatchReceipt, SdkError> {
        let wire = DirtyIngestBatch {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            entries: batch
                .entries
                .iter()
                .map(|entry| match entry {
                    DirtyBatchMutation::Upsert(record) => DirtyMutation::Upsert(record.clone()),
                    DirtyBatchMutation::Delete { doc_id } => DirtyMutation::Delete(DirtyDelete {
                        doc_id: doc_id.clone(),
                    }),
                })
                .collect(),
        };
        let response =
            client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishDirtyBatch(wire))?;
        match response {
            SearchPlaneIngestIpcResponse::DirtyReceipt(receipt) => Ok(receipt),
            other => Err(SdkError::Protocol(format!(
                "expected dirty receipt, got {}",
                QuantaIndex::ingest_response_kind(&other)
            ))),
        }
    }
}

impl crate::NamespaceQuery for RuntimeNs {
    type QueryBuilder<'a> = RuntimeQueryBuilder<'a>;

    fn query<'a>(client: &'a QuantaIndex) -> RuntimeQueryBuilder<'a> {
        RuntimeQueryBuilder::new(client)
    }
}

pub struct RuntimeQueryBuilder<'a> {
    client: &'a QuantaIndex,
    syntax: TextQuerySyntax,
    query_text: Option<String>,
    selection: Option<GenerationSelector>,
    top_k: Option<u32>,
}

impl<'a> RuntimeQueryBuilder<'a> {
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

    pub fn execute(self) -> Result<SearchPlaneRuntimeMetadataQueryResponse, SdkError> {
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage("runtime query text is required".to_string()))?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("runtime generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("runtime top_k is required".to_string()))?;
        let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection);
        let response =
            self.client
                .dispatch_query(SearchPlaneQueryIpcRequest::RuntimeMetadata(
                    RuntimeMetadataQueryRequest {
                        text_query: TextQueryRequest {
                            syntax: self.syntax,
                            query_text,
                            generation,
                            generation_selector,
                            top_k,
                        },
                    },
                ))?;
        match response {
            SearchPlaneQueryIpcResponse::RuntimeMetadata(results) => Ok(results),
            other => Err(SdkError::Protocol(format!(
                "expected runtime response, got {}",
                QuantaIndex::query_response_kind(&other)
            ))),
        }
    }
}
