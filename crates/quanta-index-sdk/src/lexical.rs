#![expect(
    clippy::redundant_pub_crate,
    reason = "crate-private marker type stays visible across sibling SDK modules only"
)]

use quanta_index_contract::lex::SymbolRecord;
use quanta_index_contract::{
    ChunkRecord, GenerationSelector, LexicalIngestBatch, LexicalReplaceScope,
    LexicalTombstoneScope, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneActivateGenerationRequest, SearchPlaneActivationAck, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneTrackKind, SearchScopeKey, TextQueryRequest,
    TextQueryResponse, TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{BatchMode, BatchReceipt, QuantaIndex, SdkError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalBatch<const SEALED: bool = true> {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    base_generation: Option<ManifestGeneration>,
    manifest_digest: String,
    batch_digest: String,
    mode: BatchMode,
    replace_scopes: Vec<LexicalReplaceScope>,
    tombstone_scopes: Vec<LexicalTombstoneScope>,
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

impl<const SEALED: bool> LexicalBatch<SEALED> {
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
    pub fn without_seal(self) -> LexicalBatch<false> {
        LexicalBatch {
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
    pub fn replace_scopes(&self) -> &[LexicalReplaceScope] {
        &self.replace_scopes
    }

    #[must_use]
    pub fn tombstone_scopes(&self) -> &[LexicalTombstoneScope] {
        &self.tombstone_scopes
    }

    #[must_use]
    pub const fn seal_requested(&self) -> bool {
        SEALED
    }

    fn to_wire_batch(&self) -> LexicalIngestBatch {
        LexicalIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            base_generation: self.base_generation,
            manifest_digest: self.manifest_digest.clone(),
            batch_digest: self.batch_digest.clone(),
            mode: self.mode.to_wire(),
            bundle_payload: None,
            replace_scopes: self.replace_scopes.clone(),
            tombstone_scopes: self.tombstone_scopes.clone(),
            seal: SEALED,
        }
    }
}

pub struct LexicalNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> LexicalNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Typed lexical query entry point backed by the crate-private
    /// namespace trait owner. See QI-NS-01.
    #[must_use]
    pub fn query(&self) -> LexicalQueryBuilder<'a> {
        <LexicalNs as crate::NamespaceQuery>::query(self.client)
    }

    /// Typed lexical publish entry point backed by the crate-private
    /// namespace trait owner. See QI-NS-01.
    pub fn publish<const SEALED: bool>(
        &self,
        batch: &LexicalBatch<SEALED>,
    ) -> Result<BatchReceipt, SdkError> {
        publish_lexical_batch(self.client, batch)
    }

    /// Publishes a sealed lexical batch and activates the accepted
    /// generation on the lexical track. If activation fails, the batch was
    /// still ingested successfully and callers must reconcile that partial
    /// state explicitly.
    pub fn publish_and_activate(
        &self,
        batch: &LexicalBatch,
    ) -> Result<(BatchReceipt, SearchPlaneActivationAck), SdkError> {
        let receipt = self.publish(batch)?;
        if !receipt.sealed {
            return Err(SdkError::Protocol(
                "lexical publish_and_activate requires a sealed receipt".to_string(),
            ));
        }
        let activation =
            self.client
                .generations()
                .commit(SearchPlaneActivateGenerationRequest {
                    repo_id: batch.repo_id().clone(),
                    revision_id: batch.revision_id().clone(),
                    manifest_generation: receipt.generation,
                    manifest_digest: receipt.manifest_digest.clone(),
                    tracks: vec![SearchPlaneTrackKind::Lexical],
                })?;
        Ok((receipt, activation))
    }

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(&self, request: TextQueryRequest) -> Result<TextQueryResponse, SdkError> {
        dispatch_text_query_request_v1(self.client, request)
    }
}

/// QI-NS-01 marker type for the built-in lexical namespace.
///
/// The `client.lexical()` surface delegates here through
/// [`crate::NamespaceIngest`] and [`crate::NamespaceQuery`].
pub(crate) struct LexicalNs;

impl crate::NamespaceIngest for LexicalNs {
    type Batch = LexicalBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &LexicalBatch) -> Result<BatchReceipt, SdkError> {
        publish_lexical_batch(client, batch)
    }
}

fn publish_lexical_batch<const SEALED: bool>(
    client: &QuantaIndex,
    batch: &LexicalBatch<SEALED>,
) -> Result<BatchReceipt, SdkError> {
    let response = client.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishLexicalBatch(
        batch.to_wire_batch(),
    ))?;
    match response {
        SearchPlaneIngestIpcResponse::LexicalReceipt(receipt) => Ok(receipt),
        other => Err(SdkError::unexpected_response(
            "lexical receipt",
            QuantaIndex::ingest_response_kind(&other),
        )),
    }
}

impl crate::NamespaceQuery for LexicalNs {
    type QueryBuilder<'a> = LexicalQueryBuilder<'a>;

    fn query(client: &QuantaIndex) -> LexicalQueryBuilder<'_> {
        LexicalQueryBuilder::new(client)
    }
}

pub struct LexicalQueryBuilder<
    'a,
    const HAS_TEXT: bool = false,
    const HAS_SELECTION: bool = false,
    const HAS_TOP_K: bool = false,
> {
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
}

impl<'a, const HAS_TEXT: bool, const HAS_SELECTION: bool, const HAS_TOP_K: bool>
    LexicalQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, HAS_TOP_K>
{
    fn transition<const NEXT_TEXT: bool, const NEXT_SELECTION: bool, const NEXT_TOP_K: bool>(
        mut self,
        update: impl FnOnce(&mut TextQueryBuilderState),
    ) -> LexicalQueryBuilder<'a, NEXT_TEXT, NEXT_SELECTION, NEXT_TOP_K> {
        update(&mut self.state);
        LexicalQueryBuilder {
            client: self.client,
            state: self.state,
        }
    }

    #[must_use]
    pub fn native(
        self,
        query_text: impl Into<String>,
    ) -> LexicalQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Native;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn sourcegraph(
        self,
        query_text: impl Into<String>,
    ) -> LexicalQueryBuilder<'a, true, HAS_SELECTION, HAS_TOP_K> {
        self.transition(|state| {
            state.syntax = TextQuerySyntax::Sourcegraph;
            state.query_text = Some(query_text.into());
        })
    }

    #[must_use]
    pub fn pinned(
        self,
        pin: quanta_index_contract::GenerationPin,
    ) -> LexicalQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Pinned(pin));
        })
    }

    #[must_use]
    pub fn active(
        self,
        repo_id: RepoId,
        revision_id: RevisionId,
    ) -> LexicalQueryBuilder<'a, HAS_TEXT, true, HAS_TOP_K> {
        self.transition(|state| {
            state.selection = Some(GenerationSelector::Active {
                repo_id,
                revision_id,
            });
        })
    }

    /// QI-QRY-01: required result cap. SDK enforces this is set before
    /// dispatch so the contract DTO carries an authoritative value.
    #[must_use]
    pub fn top_k(self, top_k: u32) -> LexicalQueryBuilder<'a, HAS_TEXT, HAS_SELECTION, true> {
        self.transition(|state| {
            state.top_k = Some(top_k);
        })
    }
}

impl LexicalQueryBuilder<'_, true, true, true> {
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
        | quanta_index_contract::SearchPlaneQueryIpcResponse::HybridSeed(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
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
