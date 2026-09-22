//! SDK contextual response binding (S21-07).
//!
//! Every wire entrypoint the SDK exposes declares, before its payload
//! moves into an envelope, exactly one closed expected-response variant
//! plus the request context that produced it. After the response's
//! request id is confirmed, the dispatcher checks the intrinsic shape
//! (already enforced by the contract decoders) and then binds the
//! response contextually: the read identity a response reports must be
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

use core::fmt;

use quanta_index_contract::{
    BatchPublishReceipt, ClusterMembershipBatchReadRequestV1, GenerationPin, GenerationSelector,
    HistoryQueryRequest, HybridQueryRequest, HybridSeedQueryRequest, ManifestGeneration, RepoId,
    RepoMapActivateGenerationRequest, RepoMapMutationAck, RepoMapQueryRequest,
    RepoMapQueryResponse, RevisionId, RuntimeMetadataQueryRequest,
    SearchCorpusGenerationIdentityV1, SearchPlaneActivateSearchCorpusGenerationCasRequest,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneExplainQueryRequest,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SemanticQueryRequest, StructuralQueryRequest, SymbolQueryRequest, TextQueryRequest,
};

use crate::SdkError;

/// The one response variant a query-plane call accepts (S21-07). Built
/// from the request before the payload moves into the envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExpectedQueryResponseV1 {
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
    RepoMapMutationAck,
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
            Self::RepoMapMutationAck => "repomap_mutation_ack",
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
#[expect(
    clippy::enum_variant_names,
    reason = "mirrors the closed wire response variant names one-to-one; renaming here would desync the SDK table from the contract"
)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExpectedIngestResponseV1 {
    SearchCorpusReceipt,
    RepoMapReceipt,
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
            Self::RepoMapReceipt => "repomap_receipt",
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

/// Which contextual axis a response failed to bind on. The error carries
/// only the route, this axis and kind labels — never a payload field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseBindingAxis {
    /// The response variant is not the one this call declared.
    Variant,
    /// A pinned response read identity differs from the requested pin.
    ReadIdentity,
    /// An active-selector response resolved outside the requested
    /// repo/revision domain.
    SelectorDomain,
    /// A candidate row belongs to another generation than the page's.
    CandidateIdentity,
    /// The page window disagrees with the rows it describes.
    Window,
    /// The response order differs from the requested order.
    Order,
    /// The returned row count exceeds the request cap.
    Cardinality,
    /// A receipt digest or generation differs from the published batch.
    BatchCommitment,
    /// An ACK's target identity differs from the requested target.
    TargetIdentity,
    /// A CAS ACK's prior-state commitment differs from the expectation
    /// the request carried.
    CasExpectation,
    /// A mutation ACK's durable sequence is not positive.
    Sequence,
}

impl ResponseBindingAxis {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Variant => "variant",
            Self::ReadIdentity => "read_identity",
            Self::SelectorDomain => "selector_domain",
            Self::CandidateIdentity => "candidate_identity",
            Self::Window => "window",
            Self::Order => "order",
            Self::Cardinality => "cardinality",
            Self::BatchCommitment => "batch_commitment",
            Self::TargetIdentity => "target_identity",
            Self::CasExpectation => "cas_expectation",
            Self::Sequence => "sequence",
        }
    }
}

impl fmt::Display for ResponseBindingAxis {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether a text query carries the `rev:at.time(...)` directive.
///
/// Its timeref resolution the search plane owns: such a query may
/// legally rebind the read to an ancestor revision of the pinned one.
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
                query_text,
                ..
            }) => {
                let (pin, active_domain) =
                    identity_from(generation.clone(), generation_selector.clone());
                let rev_at_time = is_rev_at_time_query(query_text);
                Self::ranked(
                    ExpectedQueryResponseV1::Symbol,
                    pin,
                    active_domain,
                    *top_k,
                    rev_at_time,
                )
            }
            SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
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
                    ExpectedQueryResponseV1::Semantic,
                    pin,
                    active_domain,
                    *top_k,
                    rev_at_time,
                )
            }
            SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                generation,
                generation_selector,
                top_k,
                text_query,
                ..
            }) => {
                let (pin, active_domain) =
                    identity_from(generation.clone(), generation_selector.clone());
                let rev_at_time = is_rev_at_time_query(&text_query.query_text);
                Self::ranked(
                    ExpectedQueryResponseV1::Hybrid,
                    pin,
                    active_domain,
                    *top_k,
                    rev_at_time,
                )
            }
            SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
                generation,
                generation_selector,
                top_k,
                text_query,
                ..
            }) => {
                let (pin, active_domain) =
                    identity_from(generation.clone(), generation_selector.clone());
                let rev_at_time = is_rev_at_time_query(&text_query.query_text);
                Self::ranked(
                    ExpectedQueryResponseV1::HybridSeed,
                    pin,
                    active_domain,
                    *top_k,
                    rev_at_time,
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
                    pin,
                    active_domain,
                    top_k: Some(text_query.top_k),
                    history_order: Some(*order),
                    rev_at_time: is_rev_at_time_query(&text_query.query_text),
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
                let rev_at_time = is_rev_at_time_query(&text_query.query_text);
                Self::ranked(
                    ExpectedQueryResponseV1::RuntimeMetadata,
                    pin,
                    active_domain,
                    text_query.top_k,
                    rev_at_time,
                )
            }
            SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query, ..
            }) => {
                let (pin, active_domain) = identity_from(
                    text_query.generation.clone(),
                    text_query.generation_selector.clone(),
                );
                let rev_at_time = is_rev_at_time_query(&text_query.query_text);
                Self::ranked(
                    ExpectedQueryResponseV1::Structural,
                    pin,
                    active_domain,
                    text_query.top_k,
                    rev_at_time,
                )
            }
            SearchPlaneQueryIpcRequest::RepoMapQuery(RepoMapQueryRequest {
                repo_id,
                revision_id,
                manifest_generation,
                ..
            }) => Self {
                expected: ExpectedQueryResponseV1::RepoMapQuery,
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
                pin: Some(generation.clone()),
                active_domain: None,
                top_k: None,
                history_order: None,
                rev_at_time: false,
            },
        }
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
/// Exact for a named pin, domain-consistent for an active selector. An
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
            )
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
            )
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
    ActivateRepoMap(RepoMapActivateGenerationRequest),
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
            SearchPlaneControlIpcRequest::RepoMapActivate(payload) => Self {
                expected: ExpectedControlResponseV1::RepoMapMutationAck,
                inner: ControlCall::ActivateRepoMap(payload.clone()),
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

/// Composite corpus identity compared field-wise over its snapshots:
/// repo, revision, manifest generation and manifest digest per track.
fn same_identity(
    left: &SearchCorpusGenerationIdentityV1,
    right: &SearchCorpusGenerationIdentityV1,
) -> bool {
    fn snap_eq(
        left: &quanta_index_contract::GenerationSnapshot,
        right: &quanta_index_contract::GenerationSnapshot,
    ) -> bool {
        left.repo_id == right.repo_id
            && left.revision_id == right.revision_id
            && left.track == right.track
            && left.manifest_generation == right.manifest_generation
            && left.manifest_digest == right.manifest_digest
    }
    snap_eq(&left.lexical, &right.lexical) && snap_eq(&left.semantic, &right.semantic)
}

fn check_repomap_ack(
    ack: &RepoMapMutationAck,
    request: &RepoMapActivateGenerationRequest,
) -> Result<(), SdkError> {
    let foreign = ack.repo_id != request.repo_id
        || ack.revision_id != request.revision_id
        || ack.manifest_generation != request.manifest_generation;
    if foreign {
        return Err(binding_error(
            "repomap_mutation_ack",
            ResponseBindingAxis::TargetIdentity,
            "the requested repo/revision/manifest",
            "a different identity",
        ));
    }
    Ok(())
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
                if !same_identity(&ack.active, &request.candidate) {
                    return Err(binding_error(
                        route,
                        ResponseBindingAxis::TargetIdentity,
                        "the activation target identity",
                        "a different active identity",
                    ));
                }
                if let Some(expected_prior) = &request.expected_active
                    && ack.previous_sealed_active.as_ref() != Some(expected_prior)
                {
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
                if !same_identity(&ack.active, &request.target) {
                    return Err(binding_error(
                        route,
                        ResponseBindingAxis::TargetIdentity,
                        "the rollback target identity",
                        "a different active identity",
                    ));
                }
                if !same_identity(&ack.previous_sealed_active, &request.expected_active) {
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
        SearchPlaneControlIpcResponse::RepoMapMutationAck(ack) => {
            if binding.expected != ExpectedControlResponseV1::RepoMapMutationAck {
                return Err(variant("repomap_mutation_ack"));
            }
            if let ControlCall::ActivateRepoMap(request) = &binding.inner {
                check_repomap_ack(ack, request)?;
            }
            check_sequence(ack)
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
            SearchPlaneIngestIpcRequest::PublishRepoMapBundle(bundle) => (
                ExpectedIngestResponseV1::RepoMapReceipt,
                Some((bundle.manifest_generation, bundle.manifest_digest.clone())),
            ),
        };
        Self {
            expected,
            commitment,
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
        SearchPlaneIngestIpcResponse::RepoMapReceipt(_) => ("repomap_receipt", None),
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
