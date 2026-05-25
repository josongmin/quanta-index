use quanta_index_channel::{BundleChannelPublisher, open_semantic_publisher};
use quanta_index_contract::{
    EmbeddingId, EmbeddingRecord, GenerationSelector, ManifestGeneration, RepoId, RevisionId,
    SemanticCandidateScope, SemanticChannelOp, SemanticFullBundle, SemanticQueryRequest,
    SemanticQueryResponse, SemanticVectorRef, UpsertEmbedding,
};

use crate::{BatchMode, BatchReceipt, QuantaIndex, SdkError, TextQuerySyntax};

#[derive(Clone, Debug, PartialEq)]
pub enum SemanticVector {
    Inline(Vec<f32>),
    Handle(String),
}

impl SemanticVector {
    fn into_ref(self) -> Result<SemanticVectorRef, SdkError> {
        match self {
            Self::Inline(vector) => {
                if vector.is_empty() {
                    return Err(SdkError::Usage(
                        "semantic vector must not be empty".to_string(),
                    ));
                }
                Ok(SemanticVectorRef::Inline(vector))
            }
            Self::Handle(handle) => {
                if handle.is_empty() {
                    return Err(SdkError::Usage(
                        "semantic vector handle must not be empty".to_string(),
                    ));
                }
                Ok(SemanticVectorRef::Handle(handle))
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EmbeddingMutation {
    Upsert {
        embedding_id: EmbeddingId,
        record: EmbeddingRecord,
    },
    Delete {
        embedding_id: EmbeddingId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub mode: BatchMode,
    pub manifest_payload: Vec<u8>,
    pub embeddings: Vec<EmbeddingMutation>,
    pub seal: bool,
}

impl SemanticBatch {
    #[must_use]
    pub fn replace_generation(
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            mode: BatchMode::ReplaceGeneration,
            manifest_payload: Vec::new(),
            embeddings: Vec::new(),
            seal: true,
        }
    }

    #[must_use]
    pub fn delta(repo_id: RepoId, revision_id: RevisionId, generation: ManifestGeneration) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            mode: BatchMode::Delta,
            manifest_payload: Vec::new(),
            embeddings: Vec::new(),
            seal: true,
        }
    }

    #[must_use]
    pub fn manifest_payload(mut self, payload: Vec<u8>) -> Self {
        self.manifest_payload = payload;
        self
    }

    #[must_use]
    pub fn embedding_upsert(mut self, embedding_id: EmbeddingId, record: EmbeddingRecord) -> Self {
        self.embeddings
            .push(EmbeddingMutation::Upsert { embedding_id, record });
        self
    }

    #[must_use]
    pub fn embedding_delete(mut self, embedding_id: EmbeddingId) -> Self {
        self.embeddings.push(EmbeddingMutation::Delete { embedding_id });
        self
    }

    #[must_use]
    pub fn without_seal(mut self) -> Self {
        self.seal = false;
        self
    }
}

pub struct SemanticNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SemanticNamespace<'a> {
    pub(crate) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    #[must_use]
    pub fn query(&self) -> SemanticQueryBuilder<'a> {
        SemanticQueryBuilder::new(self.client)
    }

    pub fn publish(&self, batch: &SemanticBatch) -> Result<BatchReceipt, SdkError> {
        let publisher = open_semantic_publisher(self.client.state_root()?)?;
        let mut receipt = BatchReceipt::default();
        if matches!(batch.mode, BatchMode::ReplaceGeneration) {
            let seq = publisher.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
                payload: batch.manifest_payload.clone(),
            }))?;
            receipt.record(seq);
        }
        for embedding in &batch.embeddings {
            let op = match embedding {
                EmbeddingMutation::Upsert { embedding_id, record } => {
                    let payload = QuantaIndex::encode_cbor(record)?;
                    SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        embedding_id: embedding_id.clone(),
                        payload,
                    })
                }
                EmbeddingMutation::Delete { embedding_id } => {
                    SemanticChannelOp::DeleteEmbedding(quanta_index_contract::DeleteEmbedding {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation: batch.generation,
                        embedding_id: embedding_id.clone(),
                    })
                }
            };
            let seq = publisher.publish(op)?;
            receipt.record(seq);
        }
        if batch.seal {
            let seq =
                publisher.seal(batch.repo_id.clone(), batch.revision_id.clone(), batch.generation)?;
            receipt.record(seq);
            receipt.mark_sealed();
        }
        publisher.flush()?;
        Ok(receipt)
    }
}

pub struct SemanticQueryBuilder<'a> {
    client: &'a QuantaIndex,
    vector: Option<SemanticVector>,
    selection: Option<GenerationSelector>,
    scope: Option<(TextQuerySyntax, String)>,
    top_k: Option<u32>,
}

impl<'a> SemanticQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            vector: None,
            selection: None,
            scope: None,
            top_k: None,
        }
    }

    #[must_use]
    pub fn vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(SemanticVector::Inline(vector));
        self
    }

    #[must_use]
    pub fn vector_handle(mut self, handle: impl Into<String>) -> Self {
        self.vector = Some(SemanticVector::Handle(handle.into()));
        self
    }

    #[must_use]
    pub fn scope_native(mut self, query_text: impl Into<String>) -> Self {
        self.scope = Some((TextQuerySyntax::Native, query_text.into()));
        self
    }

    #[must_use]
    pub fn scope_sourcegraph(mut self, query_text: impl Into<String>) -> Self {
        self.scope = Some((TextQuerySyntax::Sourcegraph, query_text.into()));
        self
    }

    #[must_use]
    pub fn pinned(mut self, pin: quanta_index_contract::GenerationPin) -> Self {
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

    pub fn execute(self) -> Result<SemanticQueryResponse, SdkError> {
        let vector_ref = self
            .vector
            .ok_or_else(|| SdkError::Usage("semantic vector is required".to_string()))?
            .into_ref()?;
        let selection = self.selection.ok_or_else(|| {
            SdkError::Usage("semantic generation selection is required".to_string())
        })?;
        let top_k = self
            .top_k
            .ok_or_else(|| SdkError::Usage("semantic top_k is required".to_string()))?;
        let (generation, generation_selector) = QuantaIndex::selection_to_fields(selection.clone());
        let scope = if let Some((syntax, query_text)) = self.scope {
            let (scope_generation, scope_generation_selector) =
                QuantaIndex::selection_to_fields(selection);
            Some(SemanticCandidateScope {
                syntax,
                query_text,
                generation: scope_generation,
                generation_selector: scope_generation_selector,
            })
        } else {
            None
        };
        let response = self.client.dispatch_query(
            quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: String::new(),
                query_vector: None,
                query_vector_ref: Some(vector_ref),
                generation,
                generation_selector,
                scope,
                top_k,
            }),
        )?;
        match response {
            quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(results) => Ok(results),
            other => Err(SdkError::Protocol(format!(
                "expected semantic query response, got {other:?}"
            ))),
        }
    }
}
