use quanta_index_contract::lex::SymbolRecord;
use quanta_index_contract::{
    ChunkRecord, GenerationSelector, LexicalIngestBatch, LexicalReplaceScope,
    LexicalTombstoneScope, ManifestGeneration, RepoId, RevisionId, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchScopeKey, TextQueryRequest, TextQueryResponse,
    TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{BatchMode, BatchReceipt, QuantaIndex, SdkError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub manifest_digest: String,
    pub batch_digest: String,
    pub mode: BatchMode,
    pub replace_scopes: Vec<LexicalReplaceScope>,
    pub tombstone_scopes: Vec<LexicalTombstoneScope>,
    pub seal: bool,
}

impl LexicalBatch {
    #[must_use]
    pub fn replace_generation(
        repo_id: RepoId,
        revision_id: RevisionId,
        generation: ManifestGeneration,
        manifest_digest: impl Into<String>,
        batch_digest: impl Into<String>,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            base_generation: None,
            manifest_digest: manifest_digest.into(),
            batch_digest: batch_digest.into(),
            mode: BatchMode::ReplaceGeneration,
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
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            base_generation: Some(base_generation),
            manifest_digest: manifest_digest.into(),
            batch_digest: batch_digest.into(),
            mode: BatchMode::Delta,
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
        chunks: Vec<ChunkRecord>,
        symbols: Vec<SymbolRecord>,
    ) -> Self {
        self.replace_scopes.push(LexicalReplaceScope {
            scope,
            scope_digest: scope_digest.into(),
            chunks,
            symbols,
        });
        self
    }

    #[must_use]
    pub fn tombstone_scope(mut self, scope: SearchScopeKey) -> Self {
        self.tombstone_scopes.push(LexicalTombstoneScope { scope });
        self
    }

    #[must_use]
    pub fn without_seal(mut self) -> Self {
        self.seal = false;
        self
    }
}

pub struct LexicalNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> LexicalNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Sugar for `client.ns::<LexicalNs>().query()`. See QI-NS-01.
    #[must_use]
    pub fn query(&self) -> LexicalQueryBuilder<'a> {
        <LexicalNs as crate::NamespaceQuery>::query(self.client)
    }

    /// Sugar for `client.ns::<LexicalNs>().publish(batch)`. See QI-NS-01.
    pub fn publish(&self, batch: &LexicalBatch) -> Result<BatchReceipt, SdkError> {
        <LexicalNs as crate::NamespaceIngest>::publish(self.client, batch)
    }

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(&self, request: TextQueryRequest) -> Result<TextQueryResponse, SdkError> {
        dispatch_text_query_request_v1(self.client, request)
    }
}

/// QI-NS-01 marker type for the built-in lexical namespace.
///
/// The `client.lexical()` sugar delegates here through
/// [`crate::NamespaceIngest`] and [`crate::NamespaceQuery`]. Downstream
/// callers can use `client.ns::<LexicalNs>()` directly.
pub struct LexicalNs;

impl crate::NamespaceIngest for LexicalNs {
    type Batch = LexicalBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &LexicalBatch) -> Result<BatchReceipt, SdkError> {
        let wire_batch = LexicalIngestBatch {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            base_generation: batch.base_generation,
            manifest_digest: batch.manifest_digest.clone(),
            batch_digest: batch.batch_digest.clone(),
            mode: batch.mode.to_wire(),
            replace_scopes: batch.replace_scopes.clone(),
            tombstone_scopes: batch.tombstone_scopes.clone(),
            seal: batch.seal,
        };
        let response =
            client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishLexicalBatch(wire_batch))?;
        match response {
            SearchPlaneIngestIpcResponse::LexicalReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::SemanticReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | quanta_index_contract::SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "lexical receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
        }
    }
}

impl crate::NamespaceQuery for LexicalNs {
    type QueryBuilder<'a> = LexicalQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> LexicalQueryBuilder<'_> {
        LexicalQueryBuilder::new(client)
    }
}

pub struct LexicalQueryBuilder<'a> {
    client: &'a QuantaIndex,
    state: TextQueryBuilderState,
}

impl<'a> LexicalQueryBuilder<'a> {
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

    /// QI-QRY-01: required result cap. SDK enforces this is set before
    /// dispatch so the contract DTO carries an authoritative value.
    #[must_use]
    pub fn top_k(mut self, top_k: u32) -> Self {
        self.state.top_k = Some(top_k);
        self
    }

    pub fn execute(self) -> Result<TextQueryResponse, SdkError> {
        dispatch_text_query_request_v1(self.client, self.state.build_request("lexical")?)
    }
}

fn dispatch_text_query_request_v1(
    client: &QuantaIndex,
    request: TextQueryRequest,
) -> Result<TextQueryResponse, SdkError> {
    let response = client.dispatch_query(
        quanta_index_contract::SearchPlaneQueryIpcRequest::Text(request),
    )?;
    match response {
        quanta_index_contract::SearchPlaneQueryIpcResponse::Text(results) => Ok(results),
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Bridge(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::unexpected_response(
                "text query response",
                QuantaIndex::query_response_kind(&other),
            ))
        }
    }
}
