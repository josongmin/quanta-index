use quanta_index_contract::lex::ParseTreeRecord;
use quanta_index_contract::{
    ChunkId, GenerationPin, GenerationSelector, ManifestGeneration, ParseTreeDelete,
    ParseTreeMutation, ParseTreeUpsert, RepoId, RevisionId, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    SearchPlaneStructuralQueryResponse, StructuralIngestBatch, StructuralQueryRequest,
    TextQueryRequest, TextQuerySyntax,
};

use crate::{BatchReceipt, QuantaIndex, SdkError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StructuralBatchMutation {
    Upsert {
        chunk_id: ChunkId,
        record: ParseTreeRecord,
    },
    Delete {
        chunk_id: ChunkId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub trees: Vec<StructuralBatchMutation>,
}

impl StructuralBatch {
    #[must_use]
    pub fn new(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            trees: Vec::new(),
        }
    }

    #[must_use]
    pub fn upsert(mut self, chunk_id: ChunkId, record: ParseTreeRecord) -> Self {
        self.trees
            .push(StructuralBatchMutation::Upsert { chunk_id, record });
        self
    }

    #[must_use]
    pub fn delete(mut self, chunk_id: ChunkId) -> Self {
        self.trees
            .push(StructuralBatchMutation::Delete { chunk_id });
        self
    }
}

pub struct StructuralNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> StructuralNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> StructuralQueryBuilder<'a> {
        <StructuralNs as crate::NamespaceQuery>::query(self.client)
    }

    pub fn publish(&self, batch: &StructuralBatch) -> Result<BatchReceipt, SdkError> {
        <StructuralNs as crate::NamespaceIngest>::publish(self.client, batch)
    }
}

pub struct StructuralNs;

impl crate::NamespaceIngest for StructuralNs {
    type Batch = StructuralBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &StructuralBatch) -> Result<BatchReceipt, SdkError> {
        let wire = StructuralIngestBatch {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            trees: batch
                .trees
                .iter()
                .map(|tree| match tree {
                    StructuralBatchMutation::Upsert { chunk_id, record } => {
                        ParseTreeMutation::Upsert(ParseTreeUpsert {
                            chunk_id: chunk_id.clone(),
                            record: record.clone(),
                        })
                    }
                    StructuralBatchMutation::Delete { chunk_id } => {
                        ParseTreeMutation::Delete(ParseTreeDelete {
                            chunk_id: chunk_id.clone(),
                        })
                    }
                })
                .collect(),
        };
        let response =
            client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishStructuralBatch(wire))?;
        match response {
            SearchPlaneIngestIpcResponse::StructuralReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::LexicalReceipt(_)
            | SearchPlaneIngestIpcResponse::SemanticReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::Protocol(format!(
                "expected structural receipt, got {}",
                QuantaIndex::ingest_response_kind(&other)
            ))),
        }
    }
}

impl crate::NamespaceQuery for StructuralNs {
    type QueryBuilder<'a> = StructuralQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> StructuralQueryBuilder<'_> {
        StructuralQueryBuilder::new(client)
    }
}

pub struct StructuralQueryBuilder<'a> {
    client: &'a QuantaIndex,
    syntax: TextQuerySyntax,
    query_text: Option<String>,
    selection: Option<GenerationSelector>,
    top_k: Option<u32>,
}

impl<'a> StructuralQueryBuilder<'a> {
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

    pub fn execute(self) -> Result<SearchPlaneStructuralQueryResponse, SdkError> {
        let query_text = self
            .query_text
            .ok_or_else(|| SdkError::Usage("structural query text is required".to_string()))?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("structural generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("structural top_k is required".to_string()))?;
        let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection);
        let response = self
            .client
            .dispatch_query(SearchPlaneQueryIpcRequest::Structural(
                StructuralQueryRequest {
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
            SearchPlaneQueryIpcResponse::Structural(results) => Ok(results),
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => Err(SdkError::Protocol(format!(
                "expected structural response, got {}",
                QuantaIndex::query_response_kind(&other)
            ))),
        }
    }
}
