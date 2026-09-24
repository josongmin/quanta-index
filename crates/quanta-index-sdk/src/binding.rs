//! SDK contextual response binding (S21-07).
//!
//! Every wire entrypoint the SDK exposes declares, before its payload
//! moves into an envelope, exactly one closed expected-response variant
//! plus the request context that produced it. After the response's
//! request id is confirmed, the dispatcher binds the response: the
//! contract decoders already enforce the intrinsic shape for bytes on
//! the socket, and binding re-checks the ranking and projection policy
//! for typed transports that skip the decoder. The read identity a
//! response reports must be
//! the identity the request asked for, every candidate must belong to
//! that identity, page windows must agree with the rows they describe,
//! and mutation ACKs must echo the operation they acknowledge. A
//! wrong-but-same-variant response fails closed here, before any caller
//! can read a field out of it.
//!
//! The check is shared in this module, not copied route-by-route: routes
//! only declare an expected variant and the identity fields of their
//! request, and the exhaustive matches below make a new wire variant a
//! compile error until it is bound.

use crate::error::ResponseBindingAxis;
use quanta_index_contract::{
    BatchPublishReceipt, ClusterMembershipBatchReadRequestV1, CurrentGenerationRequest,
    FileOwnerProjectionErrorV1, FileOwnerProjectionRow, GenerationPin, GenerationSelector,
    HistoryQueryRequest, HybridCandidatePolicyErrorV1, HybridQueryRequest, HybridSeedQueryRequest,
    LexicalCandidate, ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV2,
    RepoMapActiveHeadRequestV2, RepoMapMutationAck, RepoMapMutationPhaseV2,
    RepoMapPublishBundleRequestV2, RepoMapQueryRequest, RepoMapQueryResponse,
    RepoMapTerminalReceiptV2, RevisionId, RuntimeMetadataQueryRequest,
    SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponse, SearchPlaneExplainQueryRequest, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SemanticQueryRequest,
    StructuralQueryRequest, SymbolQueryRequest, TextQueryRequest,
    validate_file_owner_projection_v1, validate_hybrid_results_v1,
};

use crate::SdkError;

/// The one response variant a query-plane call accepts (S21-07). Built
/// from the request before the payload moves into the envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpectedQueryResponseV1 {
    ActiveGenerationSnapshot,
    ResolvedLexicalGeneration,
    Text,
    Symbol,
    Semantic,
    Hybrid,
    HybridSeed,
    History,
    Structural,
    RepoMapQuery,
    Explain,
    ClusterMembershipRead,
    RuntimeMetadata,
}

impl ExpectedQueryResponseV1 {
    /// The route's stable kind label, shared with the dispatcher's
    /// response-kind helpers.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::ActiveGenerationSnapshot => "active_generation_snapshot",
            Self::ResolvedLexicalGeneration => "resolved_lexical_generation",
            Self::Text => "text",
            Self::Symbol => "symbol",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
            Self::HybridSeed => "hybrid_seed",
            Self::History => "history",
            Self::Structural => "structural",
            Self::RepoMapQuery => "repomap",
            Self::Explain => "explain",
            Self::ClusterMembershipRead => "cluster_membership_batch_read",
            Self::RuntimeMetadata => "runtime_metadata",
        }
    }
}

/// The one response variant a control-plane call accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExpectedControlResponseV1 {
    SearchCorpusActivationCasAck,
    SearchCorpusRollbackCasAck,
    RepoMapTerminalReceiptV2,
    RepoMapActiveHeadV2,
    CurrentGenerationSnapshot,
    GenerationStatusReport,
    MetricsSnapshot,
    QuarantineInventory,
    QuarantineDiscardAck,
    ProcessReadinessReport,
}

impl ExpectedControlResponseV1 {
    #[must_use]
    pub(crate) const fn kind(self) -> &'static str {
        match self {
            Self::SearchCorpusActivationCasAck => "search_corpus_activation_cas_ack",
            Self::SearchCorpusRollbackCasAck => "search_corpus_rollback_cas_ack",
            Self::RepoMapTerminalReceiptV2 => "repomap_terminal_receipt_v2",
            Self::RepoMapActiveHeadV2 => "repomap_active_head_v2",
            Self::CurrentGenerationSnapshot => "current_generation_snapshot",
            Self::GenerationStatusReport => "generation_status_report",
            Self::MetricsSnapshot => "metrics_snapshot",
            Self::QuarantineInventory => "quarantine_inventory",
            Self::QuarantineDiscardAck => "quarantine_discard_ack",
            Self::ProcessReadinessReport => "process_readiness_report",
        }
    }
}

/// The one response variant an ingest-plane call accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExpectedIngestResponseV1 {
    SearchCorpusReceipt,
    RepoMapTerminalReceiptV2,
    HistoryReceipt,
    RepoCommitRecencyReceipt,
    RepoTopicReceipt,
    FileOwnershipReceipt,
    FileContributorReceipt,
    RepoMetaReceipt,
    RepoDescriptionReceipt,
    DirtyReceipt,
    RuntimeCatalogReceipt,
    StructuralReceipt,
}

impl ExpectedIngestResponseV1 {
    #[must_use]
    pub(crate) const fn kind(self) -> &'static str {
        match self {
            Self::SearchCorpusReceipt => "search_corpus_receipt",
            Self::RepoMapTerminalReceiptV2 => "repomap_terminal_receipt_v2",
            Self::HistoryReceipt => "history_receipt",
            Self::RepoCommitRecencyReceipt => "repo_commit_recency_receipt",
            Self::RepoTopicReceipt => "repo_topic_receipt",
            Self::FileOwnershipReceipt => "file_ownership_receipt",
            Self::FileContributorReceipt => "file_contributor_receipt",
            Self::RepoMetaReceipt => "repo_meta_receipt",
            Self::RepoDescriptionReceipt => "repo_description_receipt",
            Self::DirtyReceipt => "dirty_receipt",
            Self::RuntimeCatalogReceipt => "runtime_catalog_receipt",
            Self::StructuralReceipt => "structural_receipt",
        }
    }
}

/// Whether a text query carries the `rev:at.time(...)` directive.
///
/// Only the lexical planner resolves this timeref and may legally rebind
/// the read to an ancestor revision of the pinned one. Other routes must
/// keep exact response-pin binding even if their text contains the token.
/// This probes the request the SDK itself is sending, never a response.
fn is_rev_at_time_query(query_text: &str) -> bool {
    query_text.contains("rev:at.time")
}

/// Read-identity selection extracted from a request's pin fields.
///
/// An explicit pin and a `Pinned` selector agree on one exact pin; an
/// `Active` selector contributes its repo/revision domain, which a
/// resolved response pin must stay inside.
type ActiveDomain = (RepoId, RevisionId);

fn identity_from(
    generation: Option<GenerationPin>,
    selector: Option<GenerationSelector>,
) -> (Option<GenerationPin>, Option<ActiveDomain>) {
    let mut pin = generation;
    let mut domain = None;
    if let Some(selector) = selector {
        match selector {
            GenerationSelector::Pinned(selector_pin) => pin = Some(selector_pin),
            GenerationSelector::Active {
                repo_id,
                revision_id,
            } => {
                domain = Some((repo_id, revision_id));
            }
        }
    }
    (pin, domain)
}

/// The request context a query response is bound against. Extracted
/// once, before the payload moves into the envelope.
#[derive(Clone, Debug)]
pub(crate) struct QueryCallBinding {
    expected: ExpectedQueryResponseV1,
    resolution_request: Option<CurrentGenerationRequest>,
    pin: Option<GenerationPin>,
    active_domain: Option<ActiveDomain>,
    top_k: Option<u32>,
    history_order: Option<quanta_index_contract::HistoryOrderV1>,
    /// The request's text query carries `rev:at.time(...)`: the plane's
    /// timeref authority may rebind the read to an ancestor revision.
    rev_at_time: bool,
}

impl QueryCallBinding {
    /// Extract the binding from a query request payload. Exhaustive over
    /// the closed request enum: a new wire route fails to compile until
    /// it declares its expected response here.
    pub(crate) fn from_request(request: &SearchPlaneQueryIpcRequest) -> Self {
        match request {
            SearchPlaneQueryIpcRequest::ResolveActiveGeneration(request) => Self {
                expected: ExpectedQueryResponseV1::ActiveGenerationSnapshot,
                resolution_request: Some(request.clone()),
                pin: None,
                active_domain: None,
                top_k: None,
                history_order: None,
                rev_at_time: false,
            },
            SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(request) => {
                let (pin, active_domain) = identity_from(
                    request.generation.clone(),
                    request.generation_selector.clone(),
                );
                Self::ranked(
                    ExpectedQueryResponseV1::ResolvedLexicalGeneration,
                    pin,
                    active_domain,
                    request.top_k,
                    true,
                )
            }
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                generation,
                generation_selector,
                top_k,
                query_text,
                ..
            }) => {
                let (pin, active_domain) =
                    identity_from(generation.clone(), generation_selector.clone());
                let rev_at_time = is_rev_at_time_query(query_text);
                Self::ranked(
                    ExpectedQueryResponseV1::Text,
                    pin,
                    active_domain,
                    *top_k,
                    rev_at_time,
                )
            }
            SearchPlaneQueryIpcRequest::Symbol(SymbolQueryRequest {
                generation,
                generation_selector,
                top_k,
                ..
            }) => {
                let (pin, active_domain) =
                    identity_from(generation.clone(), generation_selector.clone());
                Self::ranked(
                    ExpectedQueryResponseV1::Symbol,
                    pin,
                    active_domain,
                    *top_k,
                    false,
                )
            }
            SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                generation,
                generation_selector,
                top_k,
                ..
            }) => {
                let (pin, active_domain) =
                    identity_from(generation.clone(), generation_selector.clone());
                Self::ranked(
                    ExpectedQueryResponseV1::Semantic,
                    pin,
                    active_domain,
                    *top_k,
                    false,
                )
            }
            SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                generation,
                generation_selector,
                top_k,
                ..
            }) => {
                let (pin, active_domain) =
                    identity_from(generation.clone(), generation_selector.clone());
                Self::ranked(
                    ExpectedQueryResponseV1::Hybrid,
                    pin,
                    active_domain,
                    *top_k,
                    false,
                )
            }
            SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
                generation,
                generation_selector,
                top_k,
                ..
            }) => {
                let (pin, active_domain) =
                    identity_from(generation.clone(), generation_selector.clone());
                Self::ranked(
                    ExpectedQueryResponseV1::HybridSeed,
                    pin,
                    active_domain,
                    *top_k,
                    false,
                )
            }
            SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
                text_query, order, ..
            }) => {
                let (pin, active_domain) = identity_from(
                    text_query.generation.clone(),
                    text_query.generation_selector.clone(),
                );
                Self {
                    expected: ExpectedQueryResponseV1::History,
                    resolution_request: None,
                    pin,
                    active_domain,
                    top_k: Some(text_query.top_k),
                    history_order: Some(*order),
                    rev_at_time: false,
                }
            }
            SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
                text_query,
                ..
            }) => {
                let (pin, active_domain) = identity_from(
                    text_query.generation.clone(),
                    text_query.generation_selector.clone(),
                );
                Self::ranked(
                    ExpectedQueryResponseV1::RuntimeMetadata,
                    pin,
                    active_domain,
                    text_query.top_k,
                    false,
                )
            }
            SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query, ..
            }) => {
                let (pin, active_domain) = identity_from(
                    text_query.generation.clone(),
                    text_query.generation_selector.clone(),
                );
                Self::ranked(
                    ExpectedQueryResponseV1::Structural,
                    pin,
                    active_domain,
                    text_query.top_k,
                    false,
                )
            }
            SearchPlaneQueryIpcRequest::RepoMapQuery(RepoMapQueryRequest {
                repo_id,
                revision_id,
                manifest_generation,
                ..
            }) => Self {
                expected: ExpectedQueryResponseV1::RepoMapQuery,
                resolution_request: None,
                pin: Some(GenerationPin::new(
                    repo_id.clone(),
                    revision_id.clone(),
                    *manifest_generation,
                )),
                active_domain: None,
                top_k: None,
                history_order: None,
                rev_at_time: false,
            },
            SearchPlaneQueryIpcRequest::Explain(SearchPlaneExplainQueryRequest {
                generation,
                ..
            }) => Self {
                expected: ExpectedQueryResponseV1::Explain,
                resolution_request: None,
                pin: Some(generation.clone()),
                active_domain: None,
                top_k: None,
                history_order: None,
                rev_at_time: false,
            },
            SearchPlaneQueryIpcRequest::ClusterMembershipRead(
                ClusterMembershipBatchReadRequestV1 { generation, .. },
            ) => Self {
                expected: ExpectedQueryResponseV1::ClusterMembershipRead,
                resolution_request: None,
                pin: Some(generation.clone()),
                active_domain: None,
                top_k: None,
                history_order: None,
                rev_at_time: false,
            },
        }
    }

    pub(crate) fn with_resolved_lexical_generation(mut self, pin: GenerationPin) -> Self {
        self.pin = Some(pin);
        self.active_domain = None;
        self.rev_at_time = false;
        self
    }

    const fn ranked(
        expected: ExpectedQueryResponseV1,
        pin: Option<GenerationPin>,
        active_domain: Option<ActiveDomain>,
        top_k: u32,
        rev_at_time: bool,
    ) -> Self {
        Self {
            expected,
            resolution_request: None,
            pin,
            active_domain,
            top_k: Some(top_k),
            history_order: None,
            rev_at_time,
        }
    }
}

fn binding_error(
    route: &'static str,
    axis: ResponseBindingAxis,
    expected: &str,
    actual: &str,
) -> SdkError {
    SdkError::Binding {
        route,
        axis,
        expected: expected.to_string(),
        actual: actual.to_string(),
    }
}

/// Check a resolved response pin against the request's identity.
///
/// Exact for a named pin except the lexical planner's unresolved
/// `rev:at.time` ancestor, domain-consistent for an active selector. An
/// unresolved active selector is never equated with a resolved pin by
/// simple equality; the domain check is what an active response must
/// pass.
fn check_pin(binding: &QueryCallBinding, pin: &GenerationPin) -> Result<(), SdkError> {
    if let Some(expected_pin) = &binding.pin {
        let rebinding_legal = binding.rev_at_time && pin.repo_id == expected_pin.repo_id;
        if !rebinding_legal && pin != expected_pin {
            return Err(binding_error(
                binding.expected.kind(),
                ResponseBindingAxis::ReadIdentity,
                "the requested generation pin",
                "a different resolved pin",
            ));
        }
        return Ok(());
    }
    if let Some((repo_id, revision_id)) = &binding.active_domain
        && (&pin.repo_id != repo_id || &pin.revision_id != revision_id)
    {
        return Err(binding_error(
            binding.expected.kind(),
            ResponseBindingAxis::SelectorDomain,
            "the active selector's repo/revision domain",
            "a pin outside that domain",
        ));
    }
    Ok(())
}

/// A borrow of the identity fields every ranked candidate type exposes.
struct CandidateIdentityRef<'a> {
    repo_id: &'a RepoId,
    revision_id: &'a RevisionId,
    manifest_generation: &'a ManifestGeneration,
}

impl<'a> From<&'a quanta_index_contract::LexicalCandidate> for CandidateIdentityRef<'a> {
    fn from(candidate: &'a quanta_index_contract::LexicalCandidate) -> Self {
        Self {
            repo_id: &candidate.repo_id,
            revision_id: &candidate.revision_id,
            manifest_generation: &candidate.manifest_generation,
        }
    }
}

impl<'a> From<&'a quanta_index_contract::SymbolCandidate> for CandidateIdentityRef<'a> {
    fn from(candidate: &'a quanta_index_contract::SymbolCandidate) -> Self {
        Self {
            repo_id: &candidate.repo_id,
            revision_id: &candidate.revision_id,
            manifest_generation: &candidate.manifest_generation,
        }
    }
}

/// Every candidate row on a page must belong to the page's generation.
fn check_candidates<'a, I>(
    binding: &QueryCallBinding,
    pin: &GenerationPin,
    candidates: I,
) -> Result<(), SdkError>
where
    I: IntoIterator<Item = CandidateIdentityRef<'a>>,
{
    for candidate in candidates {
        let foreign = candidate.repo_id != &pin.repo_id
            || candidate.revision_id != &pin.revision_id
            || candidate.manifest_generation != &pin.manifest_generation;
        if foreign {
            return Err(binding_error(
                binding.expected.kind(),
                ResponseBindingAxis::CandidateIdentity,
                "candidates from the page's generation",
                "a candidate from another generation",
            ));
        }
    }
    Ok(())
}

/// Page-window consistency: the window's `returned` count is exactly the
/// row count on the page.
fn check_window(route: &'static str, returned: u32, rows: usize) -> Result<(), SdkError> {
    if u64::try_from(rows).is_ok_and(|row_count| row_count == u64::from(returned)) {
        Ok(())
    } else {
        Err(binding_error(
            route,
            ResponseBindingAxis::Window,
            "a window whose returned count equals the page rows",
            "a window disagreeing with the page",
        ))
    }
}

/// Cap check: a page never returns more rows than the request's cap.
fn check_cap(route: &'static str, top_k: u32, rows: usize) -> Result<(), SdkError> {
    if u64::try_from(rows).is_ok_and(|row_count| row_count <= u64::from(top_k)) {
        Ok(())
    } else {
        Err(binding_error(
            route,
            ResponseBindingAxis::Cardinality,
            "at most the requested top_k rows",
            "more rows than the request cap",
        ))
    }
}

/// Re-check the contract ranking policy on a typed response.
///
/// The wire decoder enforces it for bytes on the socket; typed transports
/// (stubs, in-process peers) skip the decoder, so binding holds the same
/// line. Labels name the failure kind only, never a payload field.
fn check_ranking_order(
    results: &[quanta_index_contract::HybridCandidateV1],
) -> Result<(), SdkError> {
    validate_hybrid_results_v1(results).map_err(|error| {
        let actual = match error {
            HybridCandidatePolicyErrorV1::FusedScoreNotPositiveFinite { .. } => {
                "a row with an invalid fused score"
            }
            HybridCandidatePolicyErrorV1::ContributionCountOutOfRange { .. } => {
                "a row with an invalid lane contribution count"
            }
            HybridCandidatePolicyErrorV1::ContributionsNotInLaneOrder => {
                "a row with unordered lane contributions"
            }
            HybridCandidatePolicyErrorV1::RankIsZero { .. } => "a row with a zero lane rank",
            HybridCandidatePolicyErrorV1::RawScoreNotFinite { .. } => {
                "a row with a non-finite lane score"
            }
            HybridCandidatePolicyErrorV1::CandidateScoreIsNotPreferredLaneScore { .. } => {
                "a row whose score disagrees with its preferred lane score"
            }
            HybridCandidatePolicyErrorV1::ResultsNotInRankingOrder { .. } => {
                "rows outside fused-score ranking order"
            }
            HybridCandidatePolicyErrorV1::DuplicateCandidateId { .. } => {
                "a repeated candidate identity"
            }
        };
        binding_error(
            "hybrid",
            ResponseBindingAxis::RankingOrder,
            "a hybrid ranking in fused-score order with unique candidate identities",
            actual,
        )
    })
}

/// Re-check the file-owner projection pairing on a typed response, for
/// the same typed-transport reason as [`check_ranking_order`].
fn check_projection_pairing(
    results: &[LexicalCandidate],
    file_owner_rows: Option<&[FileOwnerProjectionRow]>,
) -> Result<(), SdkError> {
    validate_file_owner_projection_v1(results, file_owner_rows).map_err(|error| {
        let actual = match error {
            FileOwnerProjectionErrorV1::RowCountMismatch { .. } => {
                "a projection with a different row count"
            }
            FileOwnerProjectionErrorV1::CandidateMismatch { .. } => {
                "a projection row naming another candidate"
            }
        };
        binding_error(
            "text",
            ResponseBindingAxis::ProjectionPairing,
            "owner projection rows pairing one to one with the results",
            actual,
        )
    })
}

fn check_variant(
    binding: &QueryCallBinding,
    expected: ExpectedQueryResponseV1,
) -> Result<(), SdkError> {
    if binding.expected == expected {
        Ok(())
    } else {
        Err(binding_error(
            binding.expected.kind(),
            ResponseBindingAxis::Variant,
            binding.expected.kind(),
            expected.kind(),
        ))
    }
}

/// Bind a query response against its call. Exhaustive over the closed
/// response enum; the `Error` variant never reaches a caller as data
/// (the dispatcher lifts it before binding would need to inspect it).
pub(crate) fn bind_query_response(
    binding: &QueryCallBinding,
    response: &SearchPlaneQueryIpcResponse,
) -> Result<(), SdkError> {
    match response {
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(snapshot) => {
            check_variant(binding, ExpectedQueryResponseV1::ActiveGenerationSnapshot)?;
            let request = binding.resolution_request.as_ref().ok_or_else(|| {
                binding_error(
                    "active_generation_snapshot",
                    ResponseBindingAxis::Variant,
                    "an active-resolution request",
                    "a different request",
                )
            })?;
            if snapshot.repo_id != request.repo_id
                || snapshot.revision_id != request.revision_id
                || snapshot.track != request.track
                || snapshot.manifest_digest.trim().is_empty()
            {
                return Err(binding_error(
                    "active_generation_snapshot",
                    ResponseBindingAxis::ReadIdentity,
                    "the requested domain and track with a sealed manifest",
                    "a different or unsealed active generation",
                ));
            }
            Ok(())
        }
        SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(pin) => {
            check_variant(binding, ExpectedQueryResponseV1::ResolvedLexicalGeneration)?;
            check_pin(binding, pin)
        }
        SearchPlaneQueryIpcResponse::Text(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::Text)?;
            check_pin(binding, &payload.generation)?;
            check_candidates(
                binding,
                &payload.generation,
                payload.results.iter().map(CandidateIdentityRef::from),
            )?;
            check_window("text", payload.window.returned(), payload.results.len())?;
            check_cap(
                "text",
                binding.top_k.unwrap_or(u32::MAX),
                payload.results.len(),
            )?;
            check_projection_pairing(&payload.results, payload.file_owner_rows.as_deref())
        }
        SearchPlaneQueryIpcResponse::Symbol(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::Symbol)?;
            check_pin(binding, &payload.generation)?;
            check_candidates(
                binding,
                &payload.generation,
                payload.results.iter().map(CandidateIdentityRef::from),
            )?;
            check_window("symbol", payload.window.returned(), payload.results.len())?;
            check_cap(
                "symbol",
                binding.top_k.unwrap_or(u32::MAX),
                payload.results.len(),
            )
        }
        SearchPlaneQueryIpcResponse::Semantic(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::Semantic)?;
            check_pin(binding, &payload.generation)?;
            check_candidates(
                binding,
                &payload.generation,
                payload.results.iter().map(CandidateIdentityRef::from),
            )?;
            check_window("semantic", payload.window.returned(), payload.results.len())?;
            check_cap(
                "semantic",
                binding.top_k.unwrap_or(u32::MAX),
                payload.results.len(),
            )
        }
        SearchPlaneQueryIpcResponse::Hybrid(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::Hybrid)?;
            check_pin(binding, &payload.generation)?;
            check_window("hybrid", payload.window.returned(), payload.results.len())?;
            check_cap(
                "hybrid",
                binding.top_k.unwrap_or(u32::MAX),
                payload.results.len(),
            )?;
            check_ranking_order(&payload.results)
        }
        SearchPlaneQueryIpcResponse::HybridSeed(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::HybridSeed)?;
            check_pin(binding, &payload.generation)?;
            let rows = payload.seed_candidates.len();
            check_window("hybrid_seed", payload.window.returned(), rows)?;
            check_cap("hybrid_seed", binding.top_k.unwrap_or(u32::MAX), rows)
        }
        SearchPlaneQueryIpcResponse::History(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::History)?;
            check_pin(binding, &payload.generation)?;
            if let Some(order) = &binding.history_order
                && &payload.order != order
            {
                return Err(binding_error(
                    "history",
                    ResponseBindingAxis::Order,
                    "the requested history order",
                    "a different order",
                ));
            }
            let rows = payload.commits.len().saturating_add(payload.diffs.len());
            check_window("history", payload.window.returned(), rows)
        }
        SearchPlaneQueryIpcResponse::RuntimeMetadata(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::RuntimeMetadata)?;
            check_pin(binding, &payload.generation)?;
            check_candidates(
                binding,
                &payload.generation,
                payload.results.iter().map(CandidateIdentityRef::from),
            )?;
            check_window(
                "runtime_metadata",
                payload.window.returned(),
                payload.results.len(),
            )?;
            check_cap(
                "runtime_metadata",
                binding.top_k.unwrap_or(u32::MAX),
                payload.results.len(),
            )
        }
        SearchPlaneQueryIpcResponse::Structural(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::Structural)?;
            check_pin(binding, &payload.generation)?;
            check_window(
                "structural",
                payload.window.returned(),
                payload.results.len(),
            )
        }
        SearchPlaneQueryIpcResponse::RepoMapQuery(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::RepoMapQuery)?;
            check_repomap_identity(binding, payload)
        }
        SearchPlaneQueryIpcResponse::Explain(payload) => {
            check_variant(binding, ExpectedQueryResponseV1::Explain)?;
            check_pin(binding, &payload.generation)
        }
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
            check_variant(binding, ExpectedQueryResponseV1::ClusterMembershipRead)?;
            // The batch-read contract's own intrinsic validator covers
            // per-item generation identity (it already existed before
            // S21-07); the SDK binds the request's pin through the
            // expected variant, and the payload carries no further
            // page-level identity to double-check here.
            Ok(())
        }
        // A remote refusal is not a binding mismatch: the dispatcher
        // lifts it to `SdkError::Remote` with its typed code intact.
        SearchPlaneQueryIpcResponse::Error(_) => Ok(()),
    }
}

fn check_repomap_identity(
    binding: &QueryCallBinding,
    payload: &RepoMapQueryResponse,
) -> Result<(), SdkError> {
    if let Some(pin) = &binding.pin {
        let foreign = payload.repo_id != pin.repo_id
            || payload.revision_id != pin.revision_id
            || payload.manifest_generation != pin.manifest_generation;
        if foreign {
            return Err(binding_error(
                "repomap",
                ResponseBindingAxis::ReadIdentity,
                "the requested repo/revision/manifest",
                "a different identity",
            ));
        }
    }
    Ok(())
}

/// The request context a control response is bound against.
#[derive(Clone, Debug)]
pub(crate) struct ControlCallBinding {
    expected: ExpectedControlResponseV1,
    inner: ControlCall,
}

#[derive(Clone, Debug)]
enum ControlCall {
    /// A response whose request carries no identity to echo.
    Intrinsic,
    Activate(SearchPlaneActivateSearchCorpusGenerationCasRequest),
    Rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest),
    ActivateRepoMapV2(RepoMapActivateGenerationRequestV2),
    RepoMapActiveHeadV2(RepoMapActiveHeadRequestV2),
    CurrentGeneration {
        repo_id: RepoId,
        revision_id: RevisionId,
    },
    QuarantineDiscard(quanta_index_contract::QuarantineTargetV1),
}

impl ControlCallBinding {
    /// Extract the binding from a control request payload. Exhaustive
    /// over the closed request enum.
    pub(crate) fn from_request(request: &SearchPlaneControlIpcRequest) -> Self {
        match request {
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(payload) => Self {
                expected: ExpectedControlResponseV1::SearchCorpusActivationCasAck,
                inner: ControlCall::Activate(payload.clone()),
            },
            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(payload) => Self {
                expected: ExpectedControlResponseV1::SearchCorpusRollbackCasAck,
                inner: ControlCall::Rollback(payload.clone()),
            },
            SearchPlaneControlIpcRequest::RepoMapActivateV2(payload) => Self {
                expected: ExpectedControlResponseV1::RepoMapTerminalReceiptV2,
                inner: ControlCall::ActivateRepoMapV2(payload.clone()),
            },
            SearchPlaneControlIpcRequest::RepoMapActiveHeadV2(payload) => Self {
                expected: ExpectedControlResponseV1::RepoMapActiveHeadV2,
                inner: ControlCall::RepoMapActiveHeadV2(payload.clone()),
            },
            SearchPlaneControlIpcRequest::CurrentGeneration(
                quanta_index_contract::CurrentGenerationRequest {
                    repo_id,
                    revision_id,
                    ..
                },
            ) => Self {
                expected: ExpectedControlResponseV1::CurrentGenerationSnapshot,
                inner: ControlCall::CurrentGeneration {
                    repo_id: repo_id.clone(),
                    revision_id: revision_id.clone(),
                },
            },
            SearchPlaneControlIpcRequest::GenerationStatus(_) => Self {
                expected: ExpectedControlResponseV1::GenerationStatusReport,
                inner: ControlCall::Intrinsic,
            },
            SearchPlaneControlIpcRequest::MetricsSnapshot(_) => Self {
                expected: ExpectedControlResponseV1::MetricsSnapshot,
                inner: ControlCall::Intrinsic,
            },
            SearchPlaneControlIpcRequest::QuarantineInventory(_) => Self {
                expected: ExpectedControlResponseV1::QuarantineInventory,
                inner: ControlCall::Intrinsic,
            },
            SearchPlaneControlIpcRequest::QuarantineDiscard(payload) => Self {
                expected: ExpectedControlResponseV1::QuarantineDiscardAck,
                inner: ControlCall::QuarantineDiscard(payload.target.clone()),
            },
            SearchPlaneControlIpcRequest::ProcessReadiness(_) => Self {
                expected: ExpectedControlResponseV1::ProcessReadinessReport,
                inner: ControlCall::Intrinsic,
            },
        }
    }
}

fn check_sequence(ack: &RepoMapMutationAck) -> Result<(), SdkError> {
    if ack.terminal_sequence == 0 {
        return Err(binding_error(
            "repomap_mutation_ack",
            ResponseBindingAxis::Sequence,
            "a positive durable terminal sequence",
            "a zero sequence",
        ));
    }
    Ok(())
}

enum RepoMapV2RequestRef<'a> {
    Publish(&'a RepoMapPublishBundleRequestV2),
    Activate(&'a RepoMapActivateGenerationRequestV2),
}

fn check_repo_map_v2_receipt(
    receipt: &RepoMapTerminalReceiptV2,
    request: &RepoMapV2RequestRef<'_>,
) -> Result<(), SdkError> {
    let (phase, identity, snapshot_id, projection_version, authority_digest, source_digest) =
        match request {
            RepoMapV2RequestRef::Publish(request) => {
                let bundle = &request.bundle;
                (
                    RepoMapMutationPhaseV2::Publish,
                    (
                        &bundle.repo_id,
                        &bundle.revision_id,
                        bundle.manifest_generation,
                        &bundle.manifest_digest,
                    ),
                    bundle.snapshot_id.as_str(),
                    bundle.projection_version,
                    bundle.authority_digest.as_str(),
                    request.source_bundle_digest.as_str(),
                )
            }
            RepoMapV2RequestRef::Activate(request) => (
                RepoMapMutationPhaseV2::Activate,
                (
                    &request.repo_id,
                    &request.revision_id,
                    request.manifest_generation,
                    &request.manifest_digest,
                ),
                request.snapshot_id.as_str(),
                request.projection_version,
                request.authority_digest.as_str(),
                request.source_bundle_digest.as_str(),
            ),
        };
    if receipt.phase != phase
        || &receipt.mutation.repo_id != identity.0
        || &receipt.mutation.revision_id != identity.1
        || receipt.mutation.manifest_generation != identity.2
        || receipt.manifest_digest != *identity.3
        || receipt.snapshot_id != snapshot_id
        || receipt.projection_version != projection_version
        || receipt.authority_digest != authority_digest
        || receipt.source_bundle_digest != source_digest
    {
        return Err(binding_error(
            "repomap_terminal_receipt_v2",
            ResponseBindingAxis::TargetIdentity,
            "the exact requested phase and source-bound custody axes",
            "a different terminal receipt",
        ));
    }
    check_sequence(&receipt.mutation)?;
    if let RepoMapV2RequestRef::Activate(request) = request {
        let expected_prior = request
            .expected_active
            .as_ref()
            .map(|head| head.candidate_commitment().to_wire_string());
        let expected_epoch = request.expected_active.as_ref().map_or(Ok(1), |head| {
            head.epoch().get().checked_add(1).ok_or_else(|| {
                binding_error(
                    "repomap_terminal_receipt_v2",
                    ResponseBindingAxis::CasExpectation,
                    "a non-overflowing prior activation epoch",
                    "an exhausted prior activation epoch",
                )
            })
        })?;
        if receipt.mutation.prior_candidate_commitment != expected_prior
            || receipt.mutation.activation_epoch != expected_epoch
        {
            return Err(binding_error(
                "repomap_terminal_receipt_v2",
                ResponseBindingAxis::CasExpectation,
                "the exact requested prior active head",
                "a different prior commitment or epoch",
            ));
        }
    }
    Ok(())
}

/// Bind a control response against its call. Exhaustive over the closed
/// response enum.
pub(crate) fn bind_control_response(
    binding: &ControlCallBinding,
    response: &SearchPlaneControlIpcResponse,
) -> Result<(), SdkError> {
    let route = binding.expected.kind();
    let variant = |actual: &str| {
        binding_error(
            route,
            ResponseBindingAxis::Variant,
            binding.expected.kind(),
            actual,
        )
    };
    match response {
        SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack) => {
            if binding.expected != ExpectedControlResponseV1::SearchCorpusActivationCasAck {
                return Err(variant("search_corpus_activation_cas_ack"));
            }
            if let ControlCall::Activate(request) = &binding.inner {
                if ack.active != request.candidate {
                    return Err(binding_error(
                        route,
                        ResponseBindingAxis::TargetIdentity,
                        "the activation target identity",
                        "a different active identity",
                    ));
                }
                if ack.previous_sealed_active != request.expected_active {
                    return Err(binding_error(
                        route,
                        ResponseBindingAxis::CasExpectation,
                        "the expected prior active identity",
                        "a different prior identity",
                    ));
                }
            }
            Ok(())
        }
        SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(ack) => {
            if binding.expected != ExpectedControlResponseV1::SearchCorpusRollbackCasAck {
                return Err(variant("search_corpus_rollback_cas_ack"));
            }
            if let ControlCall::Rollback(request) = &binding.inner {
                if ack.active != request.target {
                    return Err(binding_error(
                        route,
                        ResponseBindingAxis::TargetIdentity,
                        "the rollback target identity",
                        "a different active identity",
                    ));
                }
                if ack.previous_sealed_active != request.expected_active {
                    return Err(binding_error(
                        route,
                        ResponseBindingAxis::CasExpectation,
                        "the expected prior active identity",
                        "a different prior identity",
                    ));
                }
            }
            Ok(())
        }
        SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(receipt) => {
            if binding.expected != ExpectedControlResponseV1::RepoMapTerminalReceiptV2 {
                return Err(variant("repomap_terminal_receipt_v2"));
            }
            if let ControlCall::ActivateRepoMapV2(request) = &binding.inner {
                check_repo_map_v2_receipt(receipt, &RepoMapV2RequestRef::Activate(request))?;
            }
            Ok(())
        }
        SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(head) => {
            if binding.expected != ExpectedControlResponseV1::RepoMapActiveHeadV2 {
                return Err(variant("repomap_active_head_v2"));
            }
            if let ControlCall::RepoMapActiveHeadV2(request) = &binding.inner
                && (head.repo_id != request.repo_id || head.revision_id != request.revision_id)
            {
                return Err(binding_error(
                    route,
                    ResponseBindingAxis::TargetIdentity,
                    "the requested repo/revision",
                    "a different RepoMap active-head domain",
                ));
            }
            Ok(())
        }
        SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(snapshot) => {
            if binding.expected != ExpectedControlResponseV1::CurrentGenerationSnapshot {
                return Err(variant("current_generation_snapshot"));
            }
            if let ControlCall::CurrentGeneration {
                repo_id,
                revision_id,
            } = &binding.inner
                && (&snapshot.repo_id != repo_id || &snapshot.revision_id != revision_id)
            {
                return Err(binding_error(
                    route,
                    ResponseBindingAxis::TargetIdentity,
                    "the requested repo/revision",
                    "a different identity",
                ));
            }
            Ok(())
        }
        SearchPlaneControlIpcResponse::GenerationStatusReport(_) => {
            if binding.expected != ExpectedControlResponseV1::GenerationStatusReport {
                return Err(variant("generation_status_report"));
            }
            Ok(())
        }
        SearchPlaneControlIpcResponse::MetricsSnapshot(_) => {
            if binding.expected != ExpectedControlResponseV1::MetricsSnapshot {
                return Err(variant("metrics_snapshot"));
            }
            Ok(())
        }
        SearchPlaneControlIpcResponse::QuarantineInventory(_) => {
            if binding.expected != ExpectedControlResponseV1::QuarantineInventory {
                return Err(variant("quarantine_inventory"));
            }
            Ok(())
        }
        SearchPlaneControlIpcResponse::QuarantineDiscardAck(ack) => {
            if binding.expected != ExpectedControlResponseV1::QuarantineDiscardAck {
                return Err(variant("quarantine_discard_ack"));
            }
            if let ControlCall::QuarantineDiscard(target) = &binding.inner
                && &ack.target != target
            {
                return Err(binding_error(
                    route,
                    ResponseBindingAxis::TargetIdentity,
                    "the discard target as requested",
                    "a different target",
                ));
            }
            Ok(())
        }
        SearchPlaneControlIpcResponse::ProcessReadinessReport(_) => {
            if binding.expected != ExpectedControlResponseV1::ProcessReadinessReport {
                return Err(variant("process_readiness_report"));
            }
            Ok(())
        }
        // A remote refusal is not a binding mismatch; the dispatcher
        // lifts it to `SdkError::Remote`.
        SearchPlaneControlIpcResponse::Error(_) => Ok(()),
    }
}

/// The request context an ingest response is bound against.
#[derive(Clone, Debug)]
pub(crate) struct IngestCallBinding {
    expected: ExpectedIngestResponseV1,
    /// The published batch's durable commitment: generation plus digest.
    /// `None` where the route's receipt carries no batch echo to check.
    commitment: Option<(ManifestGeneration, String)>,
    repo_map_v2: Option<RepoMapPublishBundleRequestV2>,
}

impl IngestCallBinding {
    /// Extract the binding from an ingest request payload. Exhaustive
    /// over the closed request enum.
    pub(crate) fn from_request(request: &SearchPlaneIngestIpcRequest) -> Self {
        let (expected, commitment) = match request {
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) => (
                ExpectedIngestResponseV1::SearchCorpusReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => (
                ExpectedIngestResponseV1::HistoryReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(batch) => (
                ExpectedIngestResponseV1::RepoCommitRecencyReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(batch) => (
                ExpectedIngestResponseV1::RepoTopicReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(batch) => (
                ExpectedIngestResponseV1::FileOwnershipReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(batch) => (
                ExpectedIngestResponseV1::FileContributorReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(batch) => (
                ExpectedIngestResponseV1::RepoMetaReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(batch) => (
                ExpectedIngestResponseV1::RepoDescriptionReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => (
                ExpectedIngestResponseV1::DirtyReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(batch) => (
                ExpectedIngestResponseV1::RuntimeCatalogReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => (
                ExpectedIngestResponseV1::StructuralReceipt,
                Some((batch.generation, batch.batch_digest.clone())),
            ),
            SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(_) => {
                (ExpectedIngestResponseV1::RepoMapTerminalReceiptV2, None)
            }
        };
        let repo_map_v2 = match request {
            SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(request) => Some(request.clone()),
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
            | SearchPlaneIngestIpcRequest::PublishHistoryBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(_)
            | SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(_)
            | SearchPlaneIngestIpcRequest::PublishFileContributorBatch(_)
            | SearchPlaneIngestIpcRequest::PublishDirtyBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(_)
            | SearchPlaneIngestIpcRequest::PublishStructuralBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(_) => None,
        };
        Self {
            expected,
            commitment,
            repo_map_v2,
        }
    }
}

/// Bind an ingest response against its call. Exhaustive over the closed
/// response enum: every receipt variant names its kind, and receipts
/// that echo the published batch must echo this client's batch.
pub(crate) fn bind_ingest_response(
    binding: &IngestCallBinding,
    response: &SearchPlaneIngestIpcResponse,
) -> Result<(), SdkError> {
    let route = binding.expected.kind();
    let (actual_kind, receipt): (&str, Option<&BatchPublishReceipt>) = match response {
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt) => {
            ("search_corpus_receipt", Some(receipt))
        }
        SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt) => {
            ("repomap_terminal_receipt_v2", {
                if let Some(request) = &binding.repo_map_v2 {
                    check_repo_map_v2_receipt(receipt, &RepoMapV2RequestRef::Publish(request))?;
                }
                None
            })
        }
        SearchPlaneIngestIpcResponse::HistoryReceipt(receipt) => ("history_receipt", Some(receipt)),
        SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt) => {
            ("repo_commit_recency_receipt", Some(receipt))
        }
        SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt) => {
            ("repo_topic_receipt", Some(receipt))
        }
        SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt) => {
            ("file_ownership_receipt", Some(receipt))
        }
        SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt) => {
            ("file_contributor_receipt", Some(receipt))
        }
        SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt) => {
            ("repo_meta_receipt", Some(receipt))
        }
        SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(receipt) => {
            ("repo_description_receipt", Some(receipt))
        }
        SearchPlaneIngestIpcResponse::DirtyReceipt(receipt) => ("dirty_receipt", Some(receipt)),
        SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(receipt) => {
            ("runtime_catalog_receipt", Some(receipt))
        }
        SearchPlaneIngestIpcResponse::StructuralReceipt(receipt) => {
            ("structural_receipt", Some(receipt))
        }
        // A remote refusal is not a binding mismatch; the dispatcher
        // lifts it to `SdkError::Remote`.
        SearchPlaneIngestIpcResponse::Error(_) => return Ok(()),
    };
    if binding.expected.kind() != actual_kind {
        return Err(binding_error(
            route,
            ResponseBindingAxis::Variant,
            binding.expected.kind(),
            actual_kind,
        ));
    }
    if let (Some((generation, digest)), Some(receipt)) = (&binding.commitment, receipt)
        && (&receipt.generation != generation || &receipt.batch_digest != digest)
    {
        return Err(binding_error(
            route,
            ResponseBindingAxis::BatchCommitment,
            "the published batch's generation and digest",
            "a receipt for a different batch",
        ));
    }
    Ok(())
}

/// One row of the SDK wire-entrypoint coverage inventory (S21-07).
///
/// A row names a route that sends a wire request, the one response
/// variant it accepts, and the contextual axes its responses are bound
/// on. Pure builder setters, getters and local digest helpers are
/// excluded by design and named in `SDK_WIRE_ROUTE_EXCLUSIONS_V1` with
/// their reason.
#[derive(Clone, Copy, Debug)]
pub struct SdkWireRouteV1 {
    /// The route's stable kind label.
    pub route: &'static str,
    /// The IPC plane the route dispatches on.
    pub plane: &'static str,
    /// The one expected response variant, by kind label.
    pub expected_kind: &'static str,
    /// The contextual axes `bind_*_response` enforces for this route.
    pub bound_axes: &'static [&'static str],
}

/// Exported wire entrypoints and their binding semantics. The owner
/// coverage test asserts this table exact-matches the closed expected
/// enums and the SDK dispatch surface.
pub const SDK_WIRE_ROUTES_V1: &[SdkWireRouteV1] = &[
    SdkWireRouteV1 {
        route: "active_generation_snapshot",
        plane: "query",
        expected_kind: "active_generation_snapshot",
        bound_axes: &["variant", "read_identity"],
    },
    SdkWireRouteV1 {
        route: "resolved_lexical_generation",
        plane: "query",
        expected_kind: "resolved_lexical_generation",
        bound_axes: &["variant", "read_identity"],
    },
    SdkWireRouteV1 {
        route: "text",
        plane: "query",
        expected_kind: "text",
        bound_axes: &[
            "variant",
            "read_identity",
            "selector_domain",
            "candidate_identity",
            "window",
            "cardinality",
            "projection_pairing",
        ],
    },
    SdkWireRouteV1 {
        route: "symbol",
        plane: "query",
        expected_kind: "symbol",
        bound_axes: &[
            "variant",
            "read_identity",
            "selector_domain",
            "candidate_identity",
            "window",
            "cardinality",
        ],
    },
    SdkWireRouteV1 {
        route: "semantic",
        plane: "query",
        expected_kind: "semantic",
        bound_axes: &[
            "variant",
            "read_identity",
            "selector_domain",
            "candidate_identity",
            "window",
            "cardinality",
        ],
    },
    SdkWireRouteV1 {
        route: "hybrid",
        plane: "query",
        expected_kind: "hybrid",
        bound_axes: &[
            "variant",
            "read_identity",
            "selector_domain",
            "window",
            "cardinality",
            "ranking_order",
        ],
    },
    SdkWireRouteV1 {
        route: "hybrid_seed",
        plane: "query",
        expected_kind: "hybrid_seed",
        bound_axes: &[
            "variant",
            "read_identity",
            "selector_domain",
            "window",
            "cardinality",
        ],
    },
    SdkWireRouteV1 {
        route: "history",
        plane: "query",
        expected_kind: "history",
        bound_axes: &[
            "variant",
            "read_identity",
            "selector_domain",
            "order",
            "window",
        ],
    },
    SdkWireRouteV1 {
        route: "runtime_metadata",
        plane: "query",
        expected_kind: "runtime_metadata",
        bound_axes: &[
            "variant",
            "read_identity",
            "selector_domain",
            "candidate_identity",
            "window",
            "cardinality",
        ],
    },
    SdkWireRouteV1 {
        route: "structural",
        plane: "query",
        expected_kind: "structural",
        bound_axes: &["variant", "read_identity", "selector_domain", "window"],
    },
    SdkWireRouteV1 {
        route: "repomap",
        plane: "query",
        expected_kind: "repomap",
        bound_axes: &["variant", "read_identity"],
    },
    SdkWireRouteV1 {
        route: "explain",
        plane: "query",
        expected_kind: "explain",
        bound_axes: &["variant", "read_identity"],
    },
    SdkWireRouteV1 {
        route: "cluster_membership_batch_read",
        plane: "query",
        expected_kind: "cluster_membership_batch_read",
        bound_axes: &["variant"],
    },
    SdkWireRouteV1 {
        route: "search_corpus_activation_cas_ack",
        plane: "control",
        expected_kind: "search_corpus_activation_cas_ack",
        bound_axes: &["variant", "target_identity", "cas_expectation"],
    },
    SdkWireRouteV1 {
        route: "search_corpus_rollback_cas_ack",
        plane: "control",
        expected_kind: "search_corpus_rollback_cas_ack",
        bound_axes: &["variant", "target_identity", "cas_expectation"],
    },
    SdkWireRouteV1 {
        route: "repomap_mutation_ack",
        plane: "control",
        expected_kind: "repomap_mutation_ack",
        bound_axes: &["variant", "target_identity", "sequence"],
    },
    SdkWireRouteV1 {
        route: "repomap_terminal_receipt_v2",
        plane: "control",
        expected_kind: "repomap_terminal_receipt_v2",
        bound_axes: &["variant", "target_identity", "sequence"],
    },
    SdkWireRouteV1 {
        route: "current_generation_snapshot",
        plane: "control",
        expected_kind: "current_generation_snapshot",
        bound_axes: &["variant", "target_identity"],
    },
    SdkWireRouteV1 {
        route: "generation_status_report",
        plane: "control",
        expected_kind: "generation_status_report",
        bound_axes: &["variant"],
    },
    SdkWireRouteV1 {
        route: "metrics_snapshot",
        plane: "control",
        expected_kind: "metrics_snapshot",
        bound_axes: &["variant"],
    },
    SdkWireRouteV1 {
        route: "quarantine_inventory",
        plane: "control",
        expected_kind: "quarantine_inventory",
        bound_axes: &["variant"],
    },
    SdkWireRouteV1 {
        route: "quarantine_discard_ack",
        plane: "control",
        expected_kind: "quarantine_discard_ack",
        bound_axes: &["variant", "target_identity"],
    },
    SdkWireRouteV1 {
        route: "process_readiness_report",
        plane: "control",
        expected_kind: "process_readiness_report",
        bound_axes: &["variant"],
    },
    SdkWireRouteV1 {
        route: "search_corpus_receipt",
        plane: "ingest",
        expected_kind: "search_corpus_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "repomap_receipt",
        plane: "ingest",
        expected_kind: "repomap_receipt",
        bound_axes: &["variant"],
    },
    SdkWireRouteV1 {
        route: "repomap_terminal_receipt_v2",
        plane: "ingest",
        expected_kind: "repomap_terminal_receipt_v2",
        bound_axes: &["variant", "target_identity", "sequence"],
    },
    SdkWireRouteV1 {
        route: "history_receipt",
        plane: "ingest",
        expected_kind: "history_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "repo_commit_recency_receipt",
        plane: "ingest",
        expected_kind: "repo_commit_recency_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "repo_topic_receipt",
        plane: "ingest",
        expected_kind: "repo_topic_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "file_ownership_receipt",
        plane: "ingest",
        expected_kind: "file_ownership_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "file_contributor_receipt",
        plane: "ingest",
        expected_kind: "file_contributor_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "repo_meta_receipt",
        plane: "ingest",
        expected_kind: "repo_meta_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "repo_description_receipt",
        plane: "ingest",
        expected_kind: "repo_description_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "dirty_receipt",
        plane: "ingest",
        expected_kind: "dirty_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "runtime_catalog_receipt",
        plane: "ingest",
        expected_kind: "runtime_catalog_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
    SdkWireRouteV1 {
        route: "structural_receipt",
        plane: "ingest",
        expected_kind: "structural_receipt",
        bound_axes: &["variant", "batch_commitment"],
    },
];

/// Exported SDK surface deliberately excluded from the wire coverage
/// inventory, each with its reason (S21-07: pure builder setters,
/// getters and local digest helpers never send a wire request).
pub const SDK_WIRE_ROUTE_EXCLUSIONS_V1: &[(&str, &str)] = &[
    (
        "query builders (LexicalQueryBuilder &c.)",
        "pure builder setters; build a request, never dispatch one",
    ),
    ("ConnectOptions setters", "local configuration"),
    (
        "batch digest helpers (canonical_batch_digest_v1, stamp_batch_digest_v1)",
        "local digest computation, no wire round trip",
    ),
    (
        "namespace accessors (lexical(), history(), reader(), producer(), control(), ...)",
        "return wrappers, no wire round trip",
    ),
];

#[cfg(test)]
mod search_corpus_binding_tests {
    use super::{ControlCallBinding, bind_control_response};
    use crate::{ResponseBindingAxis, SdkError};
    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RevisionId,
        SearchCorpusGenerationIdentityV1, SearchPlaneActivateSearchCorpusGenerationCasRequest,
        SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
        SearchPlaneRollbackSearchCorpusGenerationCasRequest,
        SearchPlaneSearchCorpusActivationCasAck, SearchPlaneSearchCorpusRollbackCasAck,
        SearchPlaneTrackKind, SemanticContentRootsV1,
    };

    fn identity(generation: u64) -> SearchCorpusGenerationIdentityV1 {
        let repo_id = RepoId::new("sdk-binding-roots").expect("valid repo");
        let revision_id = RevisionId::new("rev-1").expect("valid revision");
        let snapshot = |track| GenerationSnapshot {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: "manifest-digest".to_string(),
        };
        SearchCorpusGenerationIdentityV1 {
            lexical: snapshot(SearchPlaneTrackKind::Lexical),
            semantic: snapshot(SearchPlaneTrackKind::Semantic),
            semantic_content: SemanticContentRootsV1 {
                row_root_digest: format!("sha256:{}", "a".repeat(64)),
                membership_root_digest: format!("sha256:{}", "b".repeat(64)),
            },
        }
    }

    #[test]
    fn activation_and_rollback_binding_reject_swapped_semantic_roots() {
        let candidate = identity(7);
        let previous = identity(6);
        let mut swapped = candidate.clone();
        swapped.semantic_content.row_root_digest = format!("sha256:{}", "c".repeat(64));

        let activation = ControlCallBinding::from_request(
            &SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                SearchPlaneActivateSearchCorpusGenerationCasRequest {
                    candidate: candidate.clone(),
                    expected_active: Some(previous.clone()),
                },
            ),
        );
        let activation_ack = SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
            SearchPlaneSearchCorpusActivationCasAck {
                active: swapped.clone(),
                previous_sealed_active: Some(previous.clone()),
            },
        );
        assert!(matches!(
            bind_control_response(&activation, &activation_ack),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::TargetIdentity,
                ..
            })
        ));

        let rollback = ControlCallBinding::from_request(
            &SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
                SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                    expected_active: previous.clone(),
                    target: candidate.clone(),
                },
            ),
        );
        let rollback_ack = SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
            SearchPlaneSearchCorpusRollbackCasAck {
                active: swapped,
                previous_sealed_active: previous.clone(),
            },
        );
        assert!(matches!(
            bind_control_response(&rollback, &rollback_ack),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::TargetIdentity,
                ..
            })
        ));

        let mut wrong_previous = previous.clone();
        wrong_previous.semantic_content.membership_root_digest =
            format!("sha256:{}", "d".repeat(64));
        let rollback_ack = SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
            SearchPlaneSearchCorpusRollbackCasAck {
                active: candidate.clone(),
                previous_sealed_active: wrong_previous,
            },
        );
        assert!(matches!(
            bind_control_response(&rollback, &rollback_ack),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::CasExpectation,
                ..
            })
        ));

        let first_activation = ControlCallBinding::from_request(
            &SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                SearchPlaneActivateSearchCorpusGenerationCasRequest {
                    candidate: candidate.clone(),
                    expected_active: None,
                },
            ),
        );
        let spurious_previous = SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
            SearchPlaneSearchCorpusActivationCasAck {
                active: candidate,
                previous_sealed_active: Some(previous),
            },
        );
        assert!(matches!(
            bind_control_response(&first_activation, &spurious_previous),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::CasExpectation,
                ..
            })
        ));
    }
}

#[cfg(test)]
mod repo_map_v2_binding_tests {
    use super::{
        ControlCallBinding, IngestCallBinding, bind_control_response, bind_ingest_response,
    };
    use crate::{ResponseBindingAxis, SdkError};
    use quanta_index_contract::{
        CandidateCommitmentV1, ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV2,
        RepoMapExactnessSummary, RepoMapExpectedActiveV2, RepoMapGraphCoverage,
        RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapMutationAck,
        RepoMapMutationPhaseV2, RepoMapPublishBundleRequestV2, RepoMapRedactionState,
        RepoMapSourceBundle, RepoMapTerminalReceiptV2, RevisionId, SearchPlaneControlIpcRequest,
        SearchPlaneControlIpcResponse, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    };

    fn publish_request() -> RepoMapPublishBundleRequestV2 {
        let bundle = RepoMapSourceBundle::new(
            RepoId::new("sdk-v2-binding").expect("canonical repo ID"),
            RevisionId::new("rev-1").expect("canonical revision ID"),
            ManifestGeneration::new(7),
            "manifest-digest",
            "snapshot-v2",
            2,
            "authority-digest",
            RepoMapGraphCoverage {
                item_index_availability: RepoMapItemIndexAvailability::Available,
                graph_coverage_class: RepoMapGraphCoverageClass::Full,
            },
            RepoMapExactnessSummary::Exact,
            RepoMapRedactionState::Unredacted,
        );
        RepoMapPublishBundleRequestV2::new(bundle).expect("canonical fixture bundle")
    }

    fn receipt(
        request: &RepoMapPublishBundleRequestV2,
        phase: RepoMapMutationPhaseV2,
    ) -> RepoMapTerminalReceiptV2 {
        RepoMapTerminalReceiptV2 {
            phase,
            mutation: RepoMapMutationAck {
                repo_id: request.bundle.repo_id.clone(),
                revision_id: request.bundle.revision_id.clone(),
                manifest_generation: request.bundle.manifest_generation,
                prior_candidate_commitment: None,
                new_candidate_commitment: format!("sha256:{}", "ab".repeat(32)),
                activation_epoch: u64::from(phase == RepoMapMutationPhaseV2::Activate),
                terminal_sequence: 1,
                replayed: false,
            },
            manifest_digest: request.bundle.manifest_digest.clone(),
            snapshot_id: request.bundle.snapshot_id.clone(),
            projection_version: request.bundle.projection_version,
            authority_digest: request.bundle.authority_digest.clone(),
            source_bundle_digest: request.source_bundle_digest.clone(),
        }
    }

    #[test]
    fn publish_receipt_binds_phase_source_and_sequence() {
        let request = publish_request();
        let binding = IngestCallBinding::from_request(
            &SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(request.clone()),
        );
        let valid = receipt(&request, RepoMapMutationPhaseV2::Publish);
        assert!(
            bind_ingest_response(
                &binding,
                &SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(valid.clone())
            )
            .is_ok()
        );

        let mut foreign = valid.clone();
        foreign.source_bundle_digest.push('0');
        assert!(matches!(
            bind_ingest_response(
                &binding,
                &SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(foreign)
            ),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::TargetIdentity,
                ..
            })
        ));
        let mut zero_sequence = valid.clone();
        zero_sequence.mutation.terminal_sequence = 0;
        assert!(matches!(
            bind_ingest_response(
                &binding,
                &SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(zero_sequence)
            ),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::Sequence,
                ..
            })
        ));
        let wrong_phase = RepoMapTerminalReceiptV2 {
            phase: RepoMapMutationPhaseV2::Activate,
            ..valid
        };
        assert!(matches!(
            bind_ingest_response(
                &binding,
                &SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(wrong_phase)
            ),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::TargetIdentity,
                ..
            })
        ));
    }

    #[test]
    fn activate_receipt_binds_every_custody_axis() {
        let publish = publish_request();
        let request = RepoMapActivateGenerationRequestV2::for_bundle(&publish.bundle)
            .expect("canonical fixture bundle");
        let binding = ControlCallBinding::from_request(
            &SearchPlaneControlIpcRequest::RepoMapActivateV2(request),
        );
        let valid = receipt(&publish, RepoMapMutationPhaseV2::Activate);
        assert!(
            bind_control_response(
                &binding,
                &SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(valid.clone())
            )
            .is_ok()
        );
        let mut foreign = valid;
        foreign.authority_digest.push('0');
        assert!(matches!(
            bind_control_response(
                &binding,
                &SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(foreign)
            ),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::TargetIdentity,
                ..
            })
        ));
    }

    #[test]
    fn activate_receipt_binds_the_exact_prior_head() {
        let publish = publish_request();
        let prior = RepoMapExpectedActiveV2::new(
            std::num::NonZeroU64::new(7).expect("positive epoch"),
            CandidateCommitmentV1::from_bytes([0xcd; 32]),
        );
        let request = RepoMapActivateGenerationRequestV2::for_bundle(&publish.bundle)
            .expect("canonical fixture bundle")
            .with_expected_active(prior.clone());
        let binding = ControlCallBinding::from_request(
            &SearchPlaneControlIpcRequest::RepoMapActivateV2(request),
        );
        let mut valid = receipt(&publish, RepoMapMutationPhaseV2::Activate);
        valid.mutation.prior_candidate_commitment =
            Some(prior.candidate_commitment().to_wire_string());
        valid.mutation.activation_epoch = 8;
        assert!(
            bind_control_response(
                &binding,
                &SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(valid.clone()),
            )
            .is_ok()
        );

        let mut wrong_commitment = valid.clone();
        wrong_commitment.mutation.prior_candidate_commitment =
            Some(CandidateCommitmentV1::from_bytes([0xef; 32]).to_wire_string());
        assert!(matches!(
            bind_control_response(
                &binding,
                &SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(wrong_commitment)
            ),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::CasExpectation,
                ..
            })
        ));

        let mut wrong_epoch = valid;
        wrong_epoch.mutation.activation_epoch = 9;
        assert!(matches!(
            bind_control_response(
                &binding,
                &SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(wrong_epoch)
            ),
            Err(SdkError::Binding {
                axis: ResponseBindingAxis::CasExpectation,
                ..
            })
        ));
    }
}
