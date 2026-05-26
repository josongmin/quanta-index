use quanta_index_contract::{
    EmbeddingModelContract, EmbeddingRecord, GenerationSelector, ManifestGeneration, RepoId,
    RevisionId, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchScopeKey,
    SemanticIngestBatch, SemanticQueryRequest, SemanticQueryResponse, SemanticReplaceScope,
    SemanticTombstoneScope,
};

use crate::{
    BatchMode, BatchReceipt, QuantaIndex, SdkError, TextQuerySyntax,
    text_query_builder::VectorQueryBuilderState,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub manifest_digest: String,
    pub batch_digest: String,
    pub mode: BatchMode,
    pub model_contract: EmbeddingModelContract,
    pub replace_scopes: Vec<SemanticReplaceScope>,
    pub tombstone_scopes: Vec<SemanticTombstoneScope>,
    pub seal: bool,
}

impl SemanticBatch {
    #[must_use]
    pub fn replace_generation(
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
        manifest_digest: impl Into<String>,
        batch_digest: impl Into<String>,
        model_contract: EmbeddingModelContract,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            base_generation: None,
            manifest_digest: manifest_digest.into(),
            batch_digest: batch_digest.into(),
            mode: BatchMode::ReplaceGeneration,
            model_contract,
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        }
    }

    #[must_use]
    pub fn delta(
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
        base_generation: ManifestGeneration,
        manifest_digest: impl Into<String>,
        batch_digest: impl Into<String>,
        model_contract: EmbeddingModelContract,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            base_generation: Some(base_generation),
            manifest_digest: manifest_digest.into(),
            batch_digest: batch_digest.into(),
            mode: BatchMode::Delta,
            model_contract,
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        }
    }

    #[must_use]
    pub fn replace_scope(
        mut self,
        scope: SearchScopeKey,
        scope_digest: impl Into<String>,
        embeddings: Vec<EmbeddingRecord>,
    ) -> Self {
        self.replace_scopes.push(SemanticReplaceScope {
            scope,
            scope_digest: scope_digest.into(),
            embeddings,
        });
        self
    }

    #[must_use]
    pub fn tombstone_scope(mut self, scope: SearchScopeKey) -> Self {
        self.tombstone_scopes.push(SemanticTombstoneScope { scope });
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
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Typed semantic query entry point backed by the crate-private
    /// namespace trait owner. See QI-NS-01.
    #[must_use]
    pub fn query(&self) -> SemanticQueryBuilder<'a> {
        <SemanticNs as crate::NamespaceQuery>::query(self.client)
    }

    pub fn publish(&self, batch: &SemanticBatch) -> Result<BatchReceipt, SdkError> {
        <SemanticNs as crate::NamespaceIngest>::publish(self.client, batch)
    }

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(
        &self,
        request: SemanticQueryRequest,
    ) -> Result<SemanticQueryResponse, SdkError> {
        dispatch_semantic_query_request_v1(self.client, request)
    }
}

/// QI-NS-01: marker type for the built-in semantic namespace.
struct SemanticNs;

impl crate::NamespaceIngest for SemanticNs {
    type Batch = SemanticBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &SemanticBatch) -> Result<BatchReceipt, SdkError> {
        let wire_batch = SemanticIngestBatch {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            base_generation: batch.base_generation,
            manifest_digest: batch.manifest_digest.clone(),
            batch_digest: batch.batch_digest.clone(),
            mode: batch.mode.to_wire(),
            model_contract: batch.model_contract.clone(),
            replace_scopes: batch.replace_scopes.clone(),
            tombstone_scopes: batch.tombstone_scopes.clone(),
            seal: batch.seal,
        };
        let response = client.dispatch_ingest(
            SearchPlaneIngestIpcRequest::PublishSemanticBatch(wire_batch),
        )?;
        match response {
            SearchPlaneIngestIpcResponse::SemanticReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::LexicalReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "semantic receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }
}

impl crate::NamespaceQuery for SemanticNs {
    type QueryBuilder<'a> = SemanticQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> SemanticQueryBuilder<'_> {
        SemanticQueryBuilder::new(client)
    }
}

pub struct SemanticQueryBuilder<'a> {
    client: &'a QuantaIndex,
    state: VectorQueryBuilderState,
}

impl<'a> SemanticQueryBuilder<'a> {
    const fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: VectorQueryBuilderState::new(),
        }
    }

    #[must_use]
    pub fn text(mut self, query_text: impl Into<String>) -> Self {
        self.state.semantic_query_text = Some(query_text.into());
        self
    }

    #[must_use]
    pub fn scope_native(mut self, query_text: impl Into<String>) -> Self {
        self.state.scope_leg = Some((TextQuerySyntax::Native, query_text.into()));
        self
    }

    #[must_use]
    pub fn scope_sourcegraph(mut self, query_text: impl Into<String>) -> Self {
        self.state.scope_leg = Some((TextQuerySyntax::Sourcegraph, query_text.into()));
        self
    }

    #[must_use]
    pub fn pinned(mut self, pin: quanta_index_contract::GenerationPin) -> Self {
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

    /// QI-QRY-01: explicit lexical-scope candidate cap. When the
    /// builder's lexical scope (`scope_native` / `scope_sourcegraph`) is
    /// set, this MUST also be set; [`Self::execute`] returns
    /// [`SdkError::Usage`] otherwise. The two values are semantically
    /// distinct: outer `top_k` is the final semantic recall cap, while
    /// `scope_top_k` is the lexical candidate cap fed into the hybrid
    /// scope stage.
    #[must_use]
    pub fn scope_top_k(mut self, scope_top_k: u32) -> Self {
        self.state.scope_top_k = Some(scope_top_k);
        self
    }

    pub fn execute(self) -> Result<SemanticQueryResponse, SdkError> {
        dispatch_semantic_query_request_v1(self.client, self.state.build_semantic_request()?)
    }
}

fn dispatch_semantic_query_request_v1(
    client: &QuantaIndex,
    request: SemanticQueryRequest,
) -> Result<SemanticQueryResponse, SdkError> {
    let response = client.dispatch_query(
        quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(request),
    )?;
    match response {
        quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(results) => Ok(results),
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::Text(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Bridge(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::unexpected_response(
                "semantic query response",
                QuantaIndex::query_response_kind(&other),
            ))
        }
    }
}
