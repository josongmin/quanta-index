use quanta_index_contract::lex::ParseTreeRecord;
use quanta_index_contract::{
    ChunkId, GenerationPin, GenerationSelector, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneStructuralQueryResponse, SearchScopeKey,
    StructuralIngestBatch, StructuralQueryRequest, StructuralReplaceScope,
    StructuralTombstoneScope, StructuralTreeRecord, TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{BatchMode, BatchReceipt, QuantaIndex, SdkError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub manifest_digest: String,
    pub batch_digest: String,
    pub mode: BatchMode,
    pub replace_scopes: Vec<StructuralReplaceScope>,
    pub tombstone_scopes: Vec<StructuralTombstoneScope>,
    pub seal: bool,
}

impl StructuralBatch {
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
        trees: Vec<StructuralTreeRecord>,
    ) -> Self {
        self.replace_scopes.push(StructuralReplaceScope {
            scope,
            scope_digest: scope_digest.into(),
            trees,
        });
        self
    }

    #[must_use]
    pub fn replace_tree(
        mut self,
        scope: SearchScopeKey,
        scope_digest: impl Into<String>,
        chunk_id: ChunkId,
        record: ParseTreeRecord,
    ) -> Self {
        self.replace_scopes.push(StructuralReplaceScope {
            scope,
            scope_digest: scope_digest.into(),
            trees: vec![StructuralTreeRecord { chunk_id, record }],
        });
        self
    }

    #[must_use]
    pub fn tombstone_scope(mut self, scope: SearchScopeKey) -> Self {
        self.tombstone_scopes
            .push(StructuralTombstoneScope { scope });
        self
    }

    #[must_use]
    pub fn without_seal(mut self) -> Self {
        self.seal = false;
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

struct StructuralNs;

impl crate::NamespaceIngest for StructuralNs {
    type Batch = StructuralBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &StructuralBatch) -> Result<BatchReceipt, SdkError> {
        let wire = StructuralIngestBatch {
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
            client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishStructuralBatch(wire))?;
        match response {
            SearchPlaneIngestIpcResponse::StructuralReceipt(receipt) => Ok(receipt),
            other @ (SearchPlaneIngestIpcResponse::LexicalReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "structural receipt",
                QuantaIndex::ingest_response_kind(&other),
            )),
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
    state: TextQueryBuilderState,
}

impl<'a> StructuralQueryBuilder<'a> {
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

    pub fn execute(self) -> Result<SearchPlaneStructuralQueryResponse, SdkError> {
        let text_query = self.state.build_request("structural")?;
        let response = self
            .client
            .dispatch_query(SearchPlaneQueryIpcRequest::Structural(
                StructuralQueryRequest { text_query },
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
            | SearchPlaneQueryIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
                "structural response",
                QuantaIndex::query_response_kind(&other),
            )),
        }
    }
}
