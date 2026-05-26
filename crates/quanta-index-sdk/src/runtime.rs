use quanta_index_contract::lex::DirtyRecord;
use quanta_index_contract::{
    ChunkId, DirtyDelete, DirtyIngestBatch, DirtyMutation, GenerationPin, GenerationSelector,
    ManifestGeneration, RepoId, RevisionId, RuntimeMetadataQueryRequest,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneRuntimeMetadataQueryResponse, TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
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
    pub overlay_epoch_ms: u64,
    pub batch_digest: String,
    pub entries: Vec<DirtyBatchMutation>,
}

impl DirtyBatch {
    #[must_use]
    pub fn new(
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
        overlay_epoch_ms: u64,
        batch_digest: impl Into<String>,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            overlay_epoch_ms,
            batch_digest: batch_digest.into(),
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

struct RuntimeNs;

impl crate::NamespaceIngest for RuntimeNs {
    type Batch = DirtyBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &DirtyBatch) -> Result<BatchReceipt, SdkError> {
        let wire = DirtyIngestBatch {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            overlay_epoch_ms: batch.overlay_epoch_ms,
            batch_digest: batch.batch_digest.clone(),
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
            other @ (SearchPlaneIngestIpcResponse::LexicalReceipt(_)
            | SearchPlaneIngestIpcResponse::SemanticReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "dirty receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }
}

impl crate::NamespaceQuery for RuntimeNs {
    type QueryBuilder<'a> = RuntimeQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> RuntimeQueryBuilder<'_> {
        RuntimeQueryBuilder::new(client)
    }
}

pub struct RuntimeQueryBuilder<'a> {
    client: &'a QuantaIndex,
    state: TextQueryBuilderState,
}

impl<'a> RuntimeQueryBuilder<'a> {
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

    pub fn execute(self) -> Result<SearchPlaneRuntimeMetadataQueryResponse, SdkError> {
        let text_query = self.state.build_request("runtime")?;
        let response = self
            .client
            .dispatch_query(SearchPlaneQueryIpcRequest::RuntimeMetadata(
                RuntimeMetadataQueryRequest { text_query },
            ))?;
        match response {
            SearchPlaneQueryIpcResponse::RuntimeMetadata(results) => Ok(results),
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "runtime response",
                QuantaIndex::query_response_kind(&other),
            )),
        }
    }
}
