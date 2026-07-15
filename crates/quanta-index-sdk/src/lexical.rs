#![expect(
    clippy::redundant_pub_crate,
    reason = "crate-private marker type stays visible across sibling SDK modules only"
)]

use quanta_index_contract::lex::SymbolRecord;
use quanta_index_contract::{
    ChunkRecord, GenerationSelector, ManifestGeneration, RepoId, RevisionId,
    SearchCorpusGenerationIdentityV1, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchCorpusTombstoneScope, SearchPlaneActivateSearchCorpusGenerationCasRequest,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneSearchCorpusActivationCasAck, SearchPlaneTrackKind,
    SearchScopeKey, SearchScopeSurface, SemanticSourceRecordV1, SemanticSourceReplaceScopeV1,
    SemanticSourceScopeKeyV1, TextQueryRequest, TextQueryResponse, TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{BatchMode, BatchReceipt, QuantaIndex, SdkError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusBatch<const SEALED: bool = true> {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    base_generation: Option<ManifestGeneration>,
    manifest_digest: String,
    batch_digest: String,
    mode: BatchMode,
    clear_surfaces: Vec<SearchScopeSurface>,
    replace_scopes: Vec<SearchCorpusReplaceScope>,
    tombstone_scopes: Vec<SearchCorpusTombstoneScope>,
    semantic_replace_scopes: Vec<SemanticSourceReplaceScopeV1>,
    semantic_tombstone_scopes: Vec<SemanticSourceScopeKeyV1>,
}

impl SearchCorpusBatch {
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
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
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
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
        }
    }
}

impl<const SEALED: bool> SearchCorpusBatch<SEALED> {
    /// Clear every indexed row on one canonical document surface. Repeated
    /// calls are idempotent and the wire vector remains canonically ordered.
    #[must_use]
    pub fn clear_surface(mut self, surface: SearchScopeSurface) -> Self {
        if let Err(index) = self.clear_surfaces.binary_search(&surface) {
            self.clear_surfaces.insert(index, surface);
        }
        self
    }

    #[must_use]
    pub fn replace_scope(
        mut self,
        scope: SearchScopeKey,
        scope_digest: impl Into<String>,
        chunks: Vec<ChunkRecord>,
        symbols: Vec<SymbolRecord>,
    ) -> Self {
        self.replace_scopes.push(SearchCorpusReplaceScope {
            scope,
            scope_digest: scope_digest.into(),
            chunks,
            symbols,
        });
        self
    }

    #[must_use]
    pub fn tombstone_scope(mut self, scope: SearchScopeKey) -> Self {
        self.tombstone_scopes
            .push(SearchCorpusTombstoneScope { scope });
        self
    }

    /// Replace one producer-authored semantic-source scope. Scope mutations
    /// are kept in canonical key order so equivalent builder sequences emit
    /// identical wire batches.
    #[must_use]
    pub fn replace_semantic_scope(
        mut self,
        scope: SemanticSourceScopeKeyV1,
        scope_digest: impl Into<String>,
        sources: Vec<SemanticSourceRecordV1>,
    ) -> Self {
        let search_result = self.semantic_replace_scopes.binary_search_by(|candidate| {
            semantic_scope_sort_key_v1(&candidate.scope).cmp(&semantic_scope_sort_key_v1(&scope))
        });
        let index = match search_result {
            Ok(index) | Err(index) => index,
        };
        self.semantic_replace_scopes.insert(
            index,
            SemanticSourceReplaceScopeV1 {
                scope,
                scope_digest: scope_digest.into(),
                sources,
            },
        );
        self
    }

    /// Tombstone one producer-authored semantic-source scope in canonical key
    /// order. Conflicts with whole-surface clears are rejected before I/O.
    #[must_use]
    pub fn tombstone_semantic_scope(mut self, scope: SemanticSourceScopeKeyV1) -> Self {
        let search_result = self
            .semantic_tombstone_scopes
            .binary_search_by(|candidate| {
                semantic_scope_sort_key_v1(candidate).cmp(&semantic_scope_sort_key_v1(&scope))
            });
        let index = match search_result {
            Ok(index) | Err(index) => index,
        };
        self.semantic_tombstone_scopes.insert(index, scope);
        self
    }

    #[must_use]
    pub fn without_seal(self) -> SearchCorpusBatch<false> {
        SearchCorpusBatch {
            repo_id: self.repo_id,
            revision_id: self.revision_id,
            generation: self.generation,
            base_generation: self.base_generation,
            manifest_digest: self.manifest_digest,
            batch_digest: self.batch_digest,
            mode: self.mode,
            clear_surfaces: self.clear_surfaces,
            replace_scopes: self.replace_scopes,
            tombstone_scopes: self.tombstone_scopes,
            semantic_replace_scopes: self.semantic_replace_scopes,
            semantic_tombstone_scopes: self.semantic_tombstone_scopes,
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
    pub fn clear_surfaces(&self) -> &[SearchScopeSurface] {
        &self.clear_surfaces
    }

    #[must_use]
    pub fn replace_scopes(&self) -> &[SearchCorpusReplaceScope] {
        &self.replace_scopes
    }

    #[must_use]
    pub fn tombstone_scopes(&self) -> &[SearchCorpusTombstoneScope] {
        &self.tombstone_scopes
    }

    #[must_use]
    pub fn semantic_replace_scopes(&self) -> &[SemanticSourceReplaceScopeV1] {
        &self.semantic_replace_scopes
    }

    #[must_use]
    pub fn semantic_tombstone_scopes(&self) -> &[SemanticSourceScopeKeyV1] {
        &self.semantic_tombstone_scopes
    }

    #[must_use]
    pub const fn seal_requested(&self) -> bool {
        SEALED
    }

    fn to_wire_batch(&self) -> SearchCorpusIngestBatch {
        SearchCorpusIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            base_generation: self.base_generation,
            manifest_digest: self.manifest_digest.clone(),
            batch_digest: self.batch_digest.clone(),
            mode: self.mode.to_wire(),
            bundle_payload: None,
            clear_surfaces: self.clear_surfaces.clone(),
            replace_scopes: self.replace_scopes.clone(),
            tombstone_scopes: self.tombstone_scopes.clone(),
            semantic_replace_scopes: self.semantic_replace_scopes.clone(),
            semantic_tombstone_scopes: self.semantic_tombstone_scopes.clone(),
            seal: SEALED,
        }
    }
}

fn semantic_scope_sort_key_v1(
    scope: &SemanticSourceScopeKeyV1,
) -> (&'static str, &'static str, &str) {
    (
        scope.corpus_kind.as_code_str(),
        scope.owner_kind.as_code_str(),
        scope.owner_id.as_str(),
    )
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

    /// Contract-exact query replay surface. Accepts the shared wire DTO
    /// unchanged and routes it through the query transport.
    pub fn query_request(&self, request: TextQueryRequest) -> Result<TextQueryResponse, SdkError> {
        dispatch_text_query_request_v1(self.client, request)
    }
}

pub struct SearchCorpusNamespace<'a> {
    client: &'a QuantaIndex,
}

impl<'a> SearchCorpusNamespace<'a> {
    pub(super) const fn new(client: &'a QuantaIndex) -> Self {
        Self { client }
    }

    /// Typed search-corpus publish entry point backed by the crate-private
    /// namespace trait owner. See QI-NS-01.
    pub fn publish<const SEALED: bool>(
        &self,
        batch: &SearchCorpusBatch<SEALED>,
    ) -> Result<BatchReceipt, SdkError> {
        dispatch_search_corpus_publish_v1(self.client, batch)
    }

    /// Publishes a sealed search-corpus batch and atomically promotes the
    /// complete lexical + semantic identity against an explicit composite
    /// active identity. If promotion conflicts, the batch remains sealed but
    /// neither reader plane is made active.
    pub fn publish_and_activate(
        &self,
        batch: &SearchCorpusBatch,
        expected_active: Option<SearchCorpusGenerationIdentityV1>,
    ) -> Result<(BatchReceipt, SearchPlaneSearchCorpusActivationCasAck), SdkError> {
        validate_expected_search_corpus_identity_v1(batch, expected_active.as_ref())?;
        let receipt = self.publish(batch)?;
        if !receipt.sealed {
            return Err(SdkError::Protocol(
                "search corpus publish_and_activate requires a sealed receipt".to_string(),
            ));
        }
        let candidate = search_corpus_identity_from_receipt_v1(batch, &receipt)?;
        let expected_ack = SearchPlaneSearchCorpusActivationCasAck {
            active: candidate.clone(),
            previous_sealed_active: expected_active.clone(),
        };
        let response = self.client.dispatch_control(
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                SearchPlaneActivateSearchCorpusGenerationCasRequest {
                    candidate,
                    expected_active,
                },
            ),
        )?;
        let activation = match response {
            SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack) => ack,
            other @ (SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                return Err(SdkError::Protocol(format!(
                    "expected composite search corpus activation CAS ack, got {}",
                    QuantaIndex::control_response_kind(&other)
                )));
            }
            SearchPlaneControlIpcResponse::Error(_) => {
                return Err(SdkError::Protocol(
                    "control dispatch leaked an error response".to_string(),
                ));
            }
        };
        validate_composite_activation_ack_v1(&activation, &expected_ack)?;
        Ok((receipt, activation))
    }
}

fn validate_composite_activation_ack_v1(
    observed: &SearchPlaneSearchCorpusActivationCasAck,
    expected: &SearchPlaneSearchCorpusActivationCasAck,
) -> Result<(), SdkError> {
    if observed != expected {
        return Err(SdkError::Protocol(
            "composite activation acknowledgement does not match the published candidate and expected active identity"
                .to_string(),
        ));
    }
    Ok(())
}

fn search_corpus_identity_from_receipt_v1(
    batch: &SearchCorpusBatch,
    receipt: &BatchReceipt,
) -> Result<SearchCorpusGenerationIdentityV1, SdkError> {
    if receipt.generation != batch.generation() {
        return Err(SdkError::Protocol(
            "sealed search corpus receipt generation differs from the published batch".to_string(),
        ));
    }
    if receipt.manifest_digest != batch.manifest_digest() {
        return Err(SdkError::Protocol(
            "sealed search corpus receipt manifest digest differs from the published batch"
                .to_string(),
        ));
    }
    let identity = SearchCorpusGenerationIdentityV1 {
        lexical: quanta_index_contract::GenerationSnapshot {
            repo_id: batch.repo_id().clone(),
            revision_id: batch.revision_id().clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: receipt.generation,
            manifest_digest: receipt.manifest_digest.clone(),
        },
        semantic: quanta_index_contract::GenerationSnapshot {
            repo_id: batch.repo_id().clone(),
            revision_id: batch.revision_id().clone(),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: receipt.generation,
            manifest_digest: receipt.manifest_digest.clone(),
        },
    };
    identity.validate_v1().map_err(|error| {
        SdkError::Protocol(format!(
            "sealed search corpus receipt cannot form a composite generation identity: {error}"
        ))
    })?;
    Ok(identity)
}

fn validate_expected_search_corpus_identity_v1(
    batch: &SearchCorpusBatch,
    expected_active: Option<&SearchCorpusGenerationIdentityV1>,
) -> Result<(), SdkError> {
    let Some(expected_active) = expected_active else {
        return Ok(());
    };
    expected_active.validate_v1().map_err(|error| {
        SdkError::Protocol(format!(
            "expected active search corpus identity is invalid: {error}"
        ))
    })?;
    if &expected_active.lexical.repo_id != batch.repo_id()
        || &expected_active.lexical.revision_id != batch.revision_id()
    {
        return Err(SdkError::Protocol(
            "expected active search corpus identity belongs to a different repository revision"
                .to_string(),
        ));
    }
    Ok(())
}

/// QI-NS-01 marker type for the built-in lexical query namespace.
///
/// The `client.lexical()` surface delegates here through
/// [`crate::NamespaceQuery`].
pub(crate) struct LexicalNs;

/// QI-NS-01 test-local marker type for the built-in search-corpus ingest namespace.
///
/// Production `client.search_corpus()` publishes directly so sealed and
/// unsealed batches share one entry point; the trait marker stays test-only
/// for namespace-conformance coverage.
#[cfg(test)]
pub(crate) struct SearchCorpusNs;

#[cfg(test)]
impl crate::NamespaceIngest for SearchCorpusNs {
    type Batch = SearchCorpusBatch;
    type Receipt = BatchReceipt;

    fn publish(client: &QuantaIndex, batch: &SearchCorpusBatch) -> Result<BatchReceipt, SdkError> {
        dispatch_search_corpus_publish_v1(client, batch)
    }
}

fn dispatch_search_corpus_publish_v1<const SEALED: bool>(
    client: &QuantaIndex,
    batch: &SearchCorpusBatch<SEALED>,
) -> Result<BatchReceipt, SdkError> {
    let wire_batch = batch.to_wire_batch();
    wire_batch.validate_surface_mutations_v1().map_err(|err| {
        SdkError::Protocol(format!("invalid search corpus surface mutation: {err}"))
    })?;
    let response = client.dispatch_ingest(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(wire_batch),
    )?;
    match response {
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt) => {
            let expected_clear_surfaces =
                u32::try_from(batch.clear_surfaces.len()).map_err(|err| {
                    SdkError::Protocol(format!("search corpus clear surface count overflow: {err}"))
                })?;
            if receipt.accepted_clear_surfaces != expected_clear_surfaces {
                return Err(SdkError::Protocol(format!(
                    "search corpus clear receipt mismatch: expected {expected_clear_surfaces}, received {}",
                    receipt.accepted_clear_surfaces
                )));
            }
            Ok(receipt)
        }
        other @ (SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
        | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
            "search corpus receipt",
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
    fn new(client: &'a QuantaIndex) -> Self {
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

    /// Replace the canonical OR-set of language constraints.
    #[must_use]
    pub fn language_any_of(
        self,
        languages: impl IntoIterator<Item = quanta_index_contract::lex::LanguageCode>,
    ) -> Self {
        self.transition(|state| {
            state.constraints =
                quanta_index_contract::QueryConstraintSetV1::from_languages(languages);
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
