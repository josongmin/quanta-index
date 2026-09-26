#![expect(
    clippy::redundant_pub_crate,
    reason = "crate-private marker type stays visible across sibling SDK modules only"
)]

use std::collections::BTreeMap;

use quanta_index_contract::lex::SymbolRecord;
use quanta_index_contract::{
    ChunkRecord, ClusterMembershipReplaceV1, ContinuationTokenV2, GenerationSelector,
    ManifestGeneration, RepoId, RevisionId, SearchCorpusActiveHeadV1,
    SearchCorpusGenerationIdentityV1, SearchCorpusIngestBatch, SearchCorpusReplaceScope,
    SearchCorpusTombstoneScope, SearchPlaneActivateSearchCorpusGenerationCasRequest,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneSearchCorpusActivationCasAck, SearchPlaneTrackKind,
    SearchScopeKey, SearchScopeSurface, SemanticCorpusKindV1, SemanticSourceRecordV1,
    SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, TextQueryRequest, TextQueryResponse,
    TextQuerySyntax,
};

use crate::text_query_builder::TextQueryBuilderState;
use crate::{BatchMode, BatchReceipt, QuantaIndex, SdkError, stamp_batch_digest_v1};

/// A search-corpus publish under construction.
///
/// The batch's `batch_digest` is not chosen by the caller: it is the
/// canonical digest of the wire body (QI-BB-032), computed when the batch
/// is sent, so a resend of the same content is a replay of the same
/// idempotency key and the search plane refuses any digest that is not the
/// body's. [`Self::batch_digest`] computes it ahead of publishing for
/// callers that correlate receipts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusBatch<const SEALED: bool = true> {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    base_generation: Option<ManifestGeneration>,
    manifest_digest: String,
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
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            base_generation: None,
            manifest_digest: manifest_digest.into(),
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
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            generation,
            base_generation: Some(base_generation),
            manifest_digest: manifest_digest.into(),
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

    /// Replace one producer-authored semantic-source scope together with its
    /// structured `ClusterCard` membership authority. Scope mutations and
    /// memberships are kept in canonical key order so equivalent builder
    /// sequences emit identical wire batches.
    #[must_use]
    pub fn replace_semantic_scope(
        mut self,
        scope: SemanticSourceScopeKeyV1,
        scope_digest: impl Into<String>,
        sources: Vec<SemanticSourceRecordV1>,
        mut cluster_memberships: Vec<ClusterMembershipReplaceV1>,
    ) -> Self {
        cluster_memberships
            .sort_by(|left, right| left.cluster_record_id.cmp(&right.cluster_record_id));
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
                cluster_memberships,
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

    /// The canonical digest this batch publishes under: the idempotency
    /// key the receipt will name. Computed from the wire body, so it
    /// changes with any mutation of the batch.
    pub fn batch_digest(&self) -> Result<String, SdkError> {
        Ok(self.to_wire_batch()?.batch_digest)
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

    /// The wire batch, stamped with its canonical digest.
    fn to_wire_batch(&self) -> Result<SearchCorpusIngestBatch, SdkError> {
        let mut wire = SearchCorpusIngestBatch {
            repo_id: self.repo_id.clone(),
            revision_id: self.revision_id.clone(),
            generation: self.generation,
            base_generation: self.base_generation,
            manifest_digest: self.manifest_digest.clone(),
            batch_digest: String::new(),
            mode: self.mode.to_wire(),
            bundle_payload: None,
            clear_surfaces: self.clear_surfaces.clone(),
            replace_scopes: self.replace_scopes.clone(),
            tombstone_scopes: self.tombstone_scopes.clone(),
            semantic_replace_scopes: self.semantic_replace_scopes.clone(),
            semantic_tombstone_scopes: self.semantic_tombstone_scopes.clone(),
            seal: SEALED,
        };
        stamp_batch_digest_v1(&mut wire)
            .map_err(|err| SdkError::Serialization(format!("search corpus batch digest: {err}")))?;
        Ok(wire)
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
    ///
    /// The candidate names the semantic content roots the sealed receipt
    /// attested (QI-BB-028): what the plane actually sealed, not what the
    /// batch asked for. A sealed receipt that attests none is a protocol
    /// error, never an activation with guessed roots.
    pub fn publish_and_activate(
        &self,
        batch: &SearchCorpusBatch,
        expected_active: Option<SearchCorpusActiveHeadV1>,
    ) -> Result<(BatchReceipt, SearchPlaneSearchCorpusActivationCasAck), SdkError> {
        let (outcome, activation) =
            self.publish_and_activate_outcome(batch, expected_active, false)?;
        Ok((outcome.receipt, activation))
    }

    /// The same validated publish/CAS path, retaining the ingest request's
    /// observation. Activation timing is intentionally not part of ingest.
    pub fn publish_and_activate_observed(
        &self,
        batch: &SearchCorpusBatch,
        expected_active: Option<SearchCorpusActiveHeadV1>,
    ) -> Result<
        (
            quanta_index_contract::SearchCorpusPublishOutcome,
            SearchPlaneSearchCorpusActivationCasAck,
        ),
        SdkError,
    > {
        self.publish_and_activate_outcome(batch, expected_active, true)
    }

    fn publish_and_activate_outcome(
        &self,
        batch: &SearchCorpusBatch,
        expected_active: Option<SearchCorpusActiveHeadV1>,
        observation_required: bool,
    ) -> Result<
        (
            quanta_index_contract::SearchCorpusPublishOutcome,
            SearchPlaneSearchCorpusActivationCasAck,
        ),
        SdkError,
    > {
        // An expectation that could never be met — invalid, another pair,
        // or not advanced by this batch — is refused before any byte is
        // published, by the contract's own rule.
        SearchPlaneActivateSearchCorpusGenerationCasRequest::validate_expected_active_v1(
            &batch_lexical_scope_v1(batch),
            expected_active.as_ref(),
        )
        .map_err(|error| {
            SdkError::Protocol(format!("composite activation request is invalid: {error}"))
        })?;
        let outcome = dispatch_search_corpus_publish_outcome_v1(self.client, batch)?;
        if observation_required && outcome.observation.is_none() {
            return Err(SdkError::Protocol(
                "search corpus observation is missing".to_string(),
            ));
        }
        let request = SearchPlaneActivateSearchCorpusGenerationCasRequest {
            candidate: search_corpus_identity_from_sealed_receipt_v1(batch, &outcome.receipt)?,
            expected_active,
        };
        request.validate_v1().map_err(|error| {
            SdkError::Protocol(format!("composite activation request is invalid: {error}"))
        })?;
        let response = self.client.dispatch_control(
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(request),
        )?;
        let activation = match response {
            SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack) => ack,
            other @ (SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)
            | SearchPlaneControlIpcResponse::ProcessRequestEventsV1(_)) => {
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
        Ok((outcome, activation))
    }
}

/// The lexical scope a batch publishes into: what the CAS expectation is
/// validated against before the batch is published.
fn batch_lexical_scope_v1(batch: &SearchCorpusBatch) -> quanta_index_contract::GenerationSnapshot {
    quanta_index_contract::GenerationSnapshot {
        repo_id: batch.repo_id().clone(),
        revision_id: batch.revision_id().clone(),
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: batch.generation(),
        manifest_digest: batch.manifest_digest().to_string(),
    }
}

/// The composite candidate a sealed receipt describes: the batch's
/// identity on both tracks (the publish already proved the receipt names
/// this batch) and the semantic content roots the receipt attested.
fn search_corpus_identity_from_sealed_receipt_v1(
    batch: &SearchCorpusBatch,
    receipt: &BatchReceipt,
) -> Result<SearchCorpusGenerationIdentityV1, SdkError> {
    let Some(semantic_content) = receipt.semantic_content.clone() else {
        return Err(SdkError::Protocol(
            "sealed receipt attests no semantic content roots; the candidate cannot name what the plane sealed"
                .to_string(),
        ));
    };
    let identity = SearchCorpusGenerationIdentityV1 {
        lexical: batch_lexical_scope_v1(batch),
        semantic: quanta_index_contract::GenerationSnapshot {
            repo_id: batch.repo_id().clone(),
            revision_id: batch.revision_id().clone(),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: batch.generation(),
            manifest_digest: batch.manifest_digest().to_string(),
        },
        semantic_content,
    };
    identity.validate_v1().map_err(|error| {
        SdkError::Protocol(format!(
            "search corpus batch cannot form a composite generation identity: {error}"
        ))
    })?;
    Ok(identity)
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
    Ok(dispatch_search_corpus_publish_outcome_v1(client, batch)?.receipt)
}

pub(crate) fn dispatch_search_corpus_publish_observed_v1<const SEALED: bool>(
    client: &QuantaIndex,
    batch: &SearchCorpusBatch<SEALED>,
) -> Result<quanta_index_contract::SearchCorpusPublishOutcome, SdkError> {
    let outcome = dispatch_search_corpus_publish_outcome_v1(client, batch)?;
    if outcome.observation.is_none() {
        return Err(SdkError::Protocol(
            "search corpus observation is missing".to_string(),
        ));
    }
    Ok(outcome)
}

fn dispatch_search_corpus_publish_outcome_v1<const SEALED: bool>(
    client: &QuantaIndex,
    batch: &SearchCorpusBatch<SEALED>,
) -> Result<quanta_index_contract::SearchCorpusPublishOutcome, SdkError> {
    validate_semantic_cluster_membership_authority_v1(batch.semantic_replace_scopes())?;
    let wire_batch = batch.to_wire_batch()?;
    wire_batch.validate_surface_mutations_v1().map_err(|err| {
        SdkError::Protocol(format!("invalid search corpus surface mutation: {err}"))
    })?;
    let batch_digest = wire_batch.batch_digest.clone();
    let response = client.dispatch_ingest(
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(wire_batch),
    )?;
    match response {
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome) => {
            validate_search_corpus_publish_receipt_v1(batch, &batch_digest, &outcome.receipt)?;
            Ok(outcome)
        }
        other @ (SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)
        | SearchPlaneIngestIpcResponse::Error(_)) => Err(SdkError::unexpected_response(
            "search corpus receipt",
            QuantaIndex::ingest_response_kind(&other),
        )),
    }
}

fn validate_semantic_cluster_membership_authority_v1(
    scopes: &[SemanticSourceReplaceScopeV1],
) -> Result<(), SdkError> {
    for scope in scopes {
        if scope.scope.corpus_kind != SemanticCorpusKindV1::ClusterCard {
            if !scope.cluster_memberships.is_empty() {
                return Err(SdkError::Protocol(format!(
                    "non-ClusterCard semantic scope {:?} must not carry cluster membership authority",
                    scope.scope.owner_id
                )));
            }
            continue;
        }

        if scope.sources.is_empty() || scope.cluster_memberships.is_empty() {
            return Err(SdkError::Protocol(format!(
                "ClusterCard semantic scope {:?} requires one typed membership per source",
                scope.scope.owner_id
            )));
        }
        if scope.sources.len() != scope.cluster_memberships.len() {
            return Err(SdkError::Protocol(format!(
                "ClusterCard semantic scope {:?} source and membership counts differ: sources={} memberships={}",
                scope.scope.owner_id,
                scope.sources.len(),
                scope.cluster_memberships.len()
            )));
        }

        let mut source_authority_by_record_id = BTreeMap::new();
        for source in &scope.sources {
            if source.corpus_kind != SemanticCorpusKindV1::ClusterCard {
                return Err(SdkError::Protocol(format!(
                    "ClusterCard semantic scope {:?} contains a non-ClusterCard source {:?}",
                    scope.scope.owner_id, source.record_id
                )));
            }
            if source_authority_by_record_id
                .insert(source.record_id.as_str(), source.authority_digest.as_str())
                .is_some()
            {
                return Err(SdkError::Protocol(format!(
                    "ClusterCard semantic scope {:?} contains duplicate source record {:?}",
                    scope.scope.owner_id, source.record_id
                )));
            }
        }

        for membership in &scope.cluster_memberships {
            membership.validate_v1().map_err(|message| {
                SdkError::Protocol(format!(
                    "invalid typed cluster membership {:?}: {message}",
                    membership.cluster_record_id
                ))
            })?;
            let Some(expected_authority_digest) =
                source_authority_by_record_id.remove(membership.cluster_record_id.as_str())
            else {
                return Err(SdkError::Protocol(format!(
                    "typed cluster membership {:?} has no matching source record",
                    membership.cluster_record_id
                )));
            };
            if expected_authority_digest != membership.authority_digest {
                return Err(SdkError::Protocol(format!(
                    "typed cluster membership {:?} authority digest does not match its source",
                    membership.cluster_record_id
                )));
            }
        }
    }
    Ok(())
}

fn validate_search_corpus_publish_receipt_v1<const SEALED: bool>(
    batch: &SearchCorpusBatch<SEALED>,
    batch_digest: &str,
    receipt: &BatchReceipt,
) -> Result<(), SdkError> {
    if receipt.generation != batch.generation() {
        return Err(SdkError::Protocol(
            "search corpus receipt generation differs from the published batch".to_string(),
        ));
    }
    if receipt.manifest_digest.as_deref() != Some(batch.manifest_digest()) {
        return Err(SdkError::Protocol(
            "search corpus receipt manifest digest differs from the published batch".to_string(),
        ));
    }
    if receipt.batch_digest != batch_digest {
        return Err(SdkError::Protocol(
            "search corpus receipt batch digest differs from the published batch".to_string(),
        ));
    }
    if receipt.sealed != SEALED {
        return Err(SdkError::Protocol(format!(
            "search corpus receipt seal mismatch: expected {SEALED}, received {}",
            receipt.sealed
        )));
    }

    let expected_replace_scopes = u32::try_from(batch.replace_scopes.len()).map_err(|err| {
        SdkError::Protocol(format!("search corpus replace scope count overflow: {err}"))
    })?;
    let expected_tombstone_scopes = u32::try_from(batch.tombstone_scopes.len()).map_err(|err| {
        SdkError::Protocol(format!(
            "search corpus tombstone scope count overflow: {err}"
        ))
    })?;
    let expected_clear_surfaces = u32::try_from(batch.clear_surfaces.len()).map_err(|err| {
        SdkError::Protocol(format!("search corpus clear surface count overflow: {err}"))
    })?;
    let expected_semantic_replace_scopes = u32::try_from(batch.semantic_replace_scopes.len())
        .map_err(|err| {
            SdkError::Protocol(format!(
                "search corpus semantic replace scope count overflow: {err}"
            ))
        })?;
    let expected_semantic_tombstone_scopes = u32::try_from(batch.semantic_tombstone_scopes.len())
        .map_err(|err| {
        SdkError::Protocol(format!(
            "search corpus semantic tombstone scope count overflow: {err}"
        ))
    })?;

    for (label, expected, observed) in [
        (
            "replace scope",
            expected_replace_scopes,
            receipt.accepted_replace_scopes,
        ),
        (
            "tombstone scope",
            expected_tombstone_scopes,
            receipt.accepted_tombstone_scopes,
        ),
        (
            "clear surface",
            expected_clear_surfaces,
            receipt.accepted_clear_surfaces,
        ),
        (
            "semantic replace scope",
            expected_semantic_replace_scopes,
            receipt.accepted_semantic_replace_scopes,
        ),
        (
            "semantic tombstone scope",
            expected_semantic_tombstone_scopes,
            receipt.accepted_semantic_tombstone_scopes,
        ),
    ] {
        if observed != expected {
            return Err(SdkError::Protocol(format!(
                "search corpus {label} receipt mismatch: expected {expected}, received {observed}"
            )));
        }
    }
    Ok(())
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
            state.constraints = std::mem::take(&mut state.constraints).with_languages(languages);
        })
    }

    /// Restrict candidate generation to one validated repository-relative path.
    #[must_use]
    pub fn exact_repo_relative_path(
        self,
        path: quanta_index_contract::ExactRepoRelativePathV1,
    ) -> Self {
        self.transition(|state| {
            state.constraints =
                std::mem::take(&mut state.constraints).with_exact_repo_relative_path(path);
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

    /// Continue after the last row of a previous page: pass that page's
    /// `next_cursor` and pin its generation (QI-BB-005).
    #[must_use]
    pub fn after(self, cursor: ContinuationTokenV2) -> Self {
        self.transition(|state| {
            state.after = Some(cursor);
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
        other @ (quanta_index_contract::SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(
            _,
        )
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(
            _,
        )
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Symbol(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Semantic(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Hybrid(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::HybridSeed(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::History(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Structural(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            Err(SdkError::unexpected_response(
                "text query response",
                QuantaIndex::query_response_kind(&other),
            ))
        }
    }
}
