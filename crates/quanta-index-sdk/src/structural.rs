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
pub struct StructuralBatch<const SEALED: bool = true> {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    base_generation: Option<ManifestGeneration>,
    manifest_digest: String,
    batch_digest: String,
    mode: BatchMode,
    replace_scopes: Vec<StructuralReplaceScope>,
    tombstone_scopes: Vec<StructuralTombstoneScope>,
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
        }
    }
}

impl<const SEALED: bool> StructuralBatch<SEALED> {
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
    pub fn without_seal(self) -> StructuralBatch<false> {
        StructuralBatch {
            repo_id: self.repo_id,
            revision_id: self.revision_id,
            generation: self.generation,
            base_generation: self.base_generation,
            manifest_digest: self.manifest_digest,
            batch_digest: self.batch_digest,
            mode: self.mode,
            replace_scopes: self.replace_scopes,
            tombstone_scopes: self.tombstone_scopes,
        }
    }

    #[must_use]
    pub const fn repo_id(&self) -> &RepoId {
        &self.repo_id
    }

    #[must_use]
    pub const fn revision_id(&self) -> &RevisionId {
        &self.revision_id
    }

    #[must_use]
    pub const fn generation(&self) -> ManifestGeneration {
        self.generation
    }

    #[must_use]
    pub const fn base_generation(&self) -> Option<ManifestGeneration> {
        self.base_generation
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    #[must_use]
    pub fn batch_digest(&self) -> &str {
        &self.batch_digest
    }

    #[must_use]
    pub const fn mode(&self) -> BatchMode {
        self.mode
    }

    #[must_use]
    pub fn replace_scopes(&self) -> &[StructuralReplaceScope] {
        &self.replace_scopes
    }

    #[must_use]
    pub fn tombstone_scopes(&self) -> &[StructuralTombstoneScope] {
        &self.tombstone_scopes
    }

    #[must_use]
    pub const fn seal_requested(&self) -> bool {
        SEALED
    }

    fn to_wire_batch(&self) -> StructuralIngestBatch {
        StructuralIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            base_generation: self.base_generation,
            manifest_digest: self.manifest_digest.clone(),
            batch_digest: self.batch_digest.clone(),
            mode: self.mode.to_wire(),
            replace_scopes: self.replace_scopes.clone(),
            tombstone_scopes: self.tombstone_scopes.clone(),
            seal: SEALED,
        }
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

    pub fn publish<const SEALED: bool>(
        &self,
        batch: &StructuralBatch<SEALED>,
    ) -> Result<BatchReceipt, SdkError> {
        publish_structural_batch(self.client, batch)
    }

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(
        &self,
        request: StructuralQueryRequest,
    ) -> Result<SearchPlaneStructuralQueryResponse, SdkError> {
        dispatch_structural_query_request_v1(self.client, request)
    }
}

struct StructuralNs;

impl crate::NamespaceIngest for StructuralNs {
    type Batch = StructuralBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &StructuralBatch) -> Result<BatchReceipt, SdkError> {
        publish_structural_batch(client, batch)
    }
}

fn publish_structural_batch<const SEALED: bool>(
    client: &QuantaIndex,
    batch: &StructuralBatch<SEALED>,
) -> Result<BatchReceipt, SdkError> {
    let response = client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishStructuralBatch(
        batch.to_wire_batch(),
    ))?;
    match response {
        SearchPlaneIngestIpcResponse::StructuralReceipt(receipt) => Ok(receipt),
        other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
        | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
            "structural receipt",
            QuantaIndex::ingest_response_kind(&other),
        )),
    }
}

impl crate::NamespaceQuery for StructuralNs {
    type QueryBuilder<'a> = StructuralQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> StructuralQueryBuilder<'_> {
        StructuralQueryBuilder::new(client)
    }
}

pub struct StructuralQueryBuilder<
    'a,
    const HAS_TEXT: bool = false,
    const HAS_SELECTION: bool = false,
    const HAS_TOP_K: bool = false,
> {
    client: &'a QuantaIndex,
    state: TextQueryBuilderState,
}

impl<'a> StructuralQueryBuilder<'a> {
    fn new(client: &'a QuantaIndex) -> Self {
        Self {
            client,
            state: TextQueryBuilderState::new(),
        }
    }
}

impl<'a, const HAS_TEXT: bool, const HAS_SELECTION: bool, const HAS_TOP_K: bool>
    StructuralQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K>
{
    fn transition<const NEXT_TEXT: bool, const NEXT_SELECTION: bool, const NEXT_TOP_K: bool>(
        mut self,
        update: impl FnOnce(&mut TextQueryBuilderState),
    ) -> StructuralQueryBuilder<'a, NEXT_TEXT, NEXT_SELECTION, NEXT_TOP_K> {
        update(&mut self.state);
        StructuralQueryBuilder {
            client: self.client,
            state: self.state,
        }
    }

    #[must_use]
    pub fn native(
        self,
        query_text: impl Into<String>,
    ) -> StructuralQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Native;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn sourcegraph(
        self,
        query_text: impl Into<String>,
    ) -> StructuralQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Sourcegraph;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn pinned(
        self,
        pin: GenerationPin,
    ) -> StructuralQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Pinned(pin));
        })
    }

    #[must_use]
    pub fn top_k(self, top_k: u32) -> StructuralQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, true> {
        self.transition(|state| {
            state.top_k = Some(top_k);
        })
    }
}

impl StructuralQueryBuilder<'_, true, true, true> {
    pub fn execute(self) -> Result<SearchPlaneStructuralQueryResponse, SdkError> {
        let text_query = self.state.build_request("structural")?;
        dispatch_structural_query_request_v1(self.client, StructuralQueryRequest { text_query })
    }
}

fn dispatch_structural_query_request_v1(
    client: &QuantaIndex,
    request: StructuralQueryRequest,
) -> Result<SearchPlaneStructuralQueryResponse, SdkError> {
    let response = client.dispatch_query(SearchPlaneQueryIpcRequest::Structural(request))?;
    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => Ok(results),
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
            "structural response",
            QuantaIndex::query_response_kind(&other),
        )),
    }
}
