//! Blocking, opt-in native-index UDS client. Async callers must provide their
//! own bounded blocking executor; no synchronous socket I/O occurs in a poll.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde::{
    Serialize,
    ser::{SerializeSeq, SerializeStruct, SerializeStructVariant},
};

use quanta_index_contract::{
    SemanticQueryRequest, TextQueryRequest,
    index::{
        CatalogView, CoverageStatus, Digest, IndexInstanceId, IndexProfile, IngressRef,
        MAX_SOURCE_PATH_BYTES, MapRoot, ObjectRef, OperationKey, OperationReply, OperationStatus,
        OwnerKey, OwnerMutation, OwnerMutationSet, ProducerId, ProjectionCertificate, ProjectionId,
        ProjectionOwnerContribution, SourceInput, SourceTextAdmission, StableOwnerId,
        SubmitOutcome, UpdateRequest, ViewSelection,
    },
    native_protocol::{
        MAX_NATIVE_SEMANTIC_OWNER_SCAN, MAX_NATIVE_SEMANTIC_QUERY_BYTES, MAX_NATIVE_SEMANTIC_TOP_K,
        MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES, MAX_NATIVE_TEXT_TOP_K, NativeEffortDone,
        NativeEffortPreference, NativeGraphQuery, NativeGraphResult, NativeIndexErrorCode,
        NativeIndexRequest, NativeIndexRequestEnvelope, NativeIndexResponse,
        NativeIndexResponseEnvelope, NativeIndexWireError, NativeInteractionPublisherId,
        NativeLiteralPage, NativeOverlayCorpusStamp, NativeOverlayEffortContext,
        NativeOverlayReply, NativeOverlayRequest, NativeOverlaySaveIntent,
        NativeOverlaySaveReceipt, NativeOverlaySemanticResult, NativeOverlaySession,
        NativeOverlayTextQuery, NativeOverlayTextResult, NativeOverlayTextSource, NativePathPage,
        NativeProducerSourcePage, NativeProjectionOwnerLookup, NativeProjectionRead,
        NativeSemanticResult, NativeSourceOwnerKeys, NativeSourceOwnerLookup, NativeStubReply,
        NativeStubRequest, NativeTextResult,
    },
};
use quanta_index_ipc::{
    AdmittedClientResponseBody, IpcError, MAX_MULTIFRAME_REQUEST_BODY_BYTES, MemoryLease,
    MemoryLifetime, MemoryScope, RequestBudgetV1, send_request_body_with_budget_admitted,
    send_request_with_budget, send_stream_request,
};

mod admitted_reply_v1;
mod read_session;
mod repomap;
mod stub_reply;

/// One canonical Native publication response with its memory charges.
///
/// Borrow the payload while this owner is alive. Cloning an owned response
/// needs a separate memory admission.
#[derive(Debug)]
pub struct AdmittedNativePublicationReply {
    response: NativeIndexResponse,
    _body: AdmittedClientResponseBody,
    _decoded: MemoryLease,
}

impl AdmittedNativePublicationReply {
    #[must_use]
    pub fn response(&self) -> &NativeIndexResponse {
        &self.response
    }
}

/// A validated Stub response whose raw and decoded memory charges remain live
/// for every scoped borrow of the prepared edits.
#[derive(Debug)]
pub struct AdmittedNativeStubReply {
    request: NativeStubRequest,
    reply: NativeStubReply,
    _body: AdmittedClientResponseBody,
    _decoded: MemoryLease,
}

impl AdmittedNativeStubReply {
    #[must_use]
    pub fn request(&self) -> &NativeStubRequest {
        &self.request
    }

    #[must_use]
    pub fn reply(&self) -> &NativeStubReply {
        &self.reply
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NativeUdsError {
    #[error("native index response is malformed or mismatched: {0}")]
    Protocol(String),
    #[error("native index refused: {0:?}")]
    Refused(NativeIndexWireError),
    #[error("native index transport failed: {0}")]
    Transport(String),
    #[error("native write outcome unknown; reconcile original key {0:?}")]
    Unknown(OperationKey),
}

pub struct NativeEffortStreamQuery {
    pub overlay: Option<NativeOverlayEffortContext>,
    pub selection: ViewSelection,
    pub text_instance: IndexInstanceId,
    pub semantic_instance: IndexInstanceId,
    pub text_query: TextQueryRequest,
    pub semantic_query: SemanticQueryRequest,
    pub preference: NativeEffortPreference,
}

pub enum NativeEffortStreamStage<'a> {
    Fast(&'a NativeTextResult),
    Deep(&'a NativeSemanticResult),
    OverlayFast(&'a NativeOverlayTextResult),
    OverlayDeep(&'a NativeOverlaySemanticResult),
}

/// A source item borrowed for one bounded Native `StageSources` request. The
/// caller retains its path and bytes until the request and ACK finish.
pub struct BorrowedSourceInput<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub text: SourceTextAdmission,
}

/// Maximum number of source items in one `StageSources` request.
pub const MAX_NATIVE_STAGE_SOURCES_INPUTS: usize = 32;
/// Maximum sum of each item's path bytes, source bytes, and 256-byte wire
/// allowance. The IPC codec applies its separate encoded-body limit.
pub const MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES: usize = 8 * 1024 * 1024;

trait SourceInputView {
    fn path(&self) -> &str;
    fn bytes(&self) -> &[u8];
    fn text(&self) -> SourceTextAdmission;
}

impl SourceInputView for SourceInput {
    fn path(&self) -> &str {
        &self.path
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    fn text(&self) -> SourceTextAdmission {
        self.text
    }
}

impl SourceInputView for BorrowedSourceInput<'_> {
    fn path(&self) -> &str {
        self.path
    }
    fn bytes(&self) -> &[u8] {
        self.bytes
    }
    fn text(&self) -> SourceTextAdmission {
        self.text
    }
}

struct StageSourcesEnvelope<'a, T> {
    request_id: u64,
    ingress: IngressRef,
    inputs: &'a [T],
}

impl<T: SourceInputView> Serialize for StageSourcesEnvelope<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_struct("NativeIndexRequestEnvelope", 2)?;
        envelope.serialize_field("request_id", &self.request_id)?;
        envelope.serialize_field(
            "payload",
            &StageSourcesPayload {
                ingress: self.ingress,
                inputs: self.inputs,
            },
        )?;
        envelope.end()
    }
}

struct StageSourcesPayload<'a, T> {
    ingress: IngressRef,
    inputs: &'a [T],
}

impl<T: SourceInputView> Serialize for StageSourcesPayload<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut payload =
            serializer.serialize_struct_variant("NativeIndexRequest", 20, "StageSources", 2)?;
        payload.serialize_field("ingress", &self.ingress)?;
        payload.serialize_field("inputs", &SourceInputsWire(self.inputs))?;
        payload.end()
    }
}

struct SourceInputsWire<'a, T>(&'a [T]);

impl<T: SourceInputView> Serialize for SourceInputsWire<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for input in self.0 {
            sequence.serialize_element(&SourceInputWire(input))?;
        }
        sequence.end()
    }
}

struct SourceInputWire<'a, T>(&'a T);

impl<T: SourceInputView> Serialize for SourceInputWire<'_, T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut input = serializer.serialize_struct("SourceInput", 3)?;
        input.serialize_field("path", self.0.path())?;
        input.serialize_field("bytes", self.0.bytes())?;
        input.serialize_field("text", &self.0.text())?;
        input.end()
    }
}

struct BorrowedSubmitEnvelope<'a> {
    request_id: u64,
    request: &'a UpdateRequest,
}

impl Serialize for BorrowedSubmitEnvelope<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_struct("NativeIndexRequestEnvelope", 2)?;
        envelope.serialize_field("request_id", &self.request_id)?;
        envelope.serialize_field("payload", &BorrowedSubmitPayload(self.request))?;
        envelope.end()
    }
}

struct BorrowedSubmitPayload<'a>(&'a UpdateRequest);

impl Serialize for BorrowedSubmitPayload<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexRequest", 6, "Submit", self.0)
    }
}

struct BorrowedMutationPageEnvelope<'a> {
    request_id: u64,
    ingress: IngressRef,
    base: MapRoot,
    mutations: &'a [OwnerMutation],
}

impl Serialize for BorrowedMutationPageEnvelope<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_struct("NativeIndexRequestEnvelope", 2)?;
        envelope.serialize_field("request_id", &self.request_id)?;
        envelope.serialize_field("payload", &BorrowedMutationPagePayload(self))?;
        envelope.end()
    }
}

struct BorrowedMutationPagePayload<'a, 'b>(&'a BorrowedMutationPageEnvelope<'b>);

impl Serialize for BorrowedMutationPagePayload<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut payload = serializer.serialize_struct_variant(
            "NativeIndexRequest",
            15,
            "StageMutationPage",
            3,
        )?;
        payload.serialize_field("ingress", &self.0.ingress)?;
        payload.serialize_field("base", &self.0.base)?;
        payload.serialize_field("mutations", &self.0.mutations)?;
        payload.end()
    }
}

pub struct NativeUdsClient<P = PathBuf> {
    socket: P,
    next_request_id: AtomicU64,
}

fn allocate_request_id(counter: &AtomicU64) -> Result<u64, NativeUdsError> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let next = current
            .checked_add(1)
            .ok_or_else(|| NativeUdsError::Protocol("request id exhausted".into()))?;
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(current),
            Err(observed) => current = observed,
        }
    }
}

impl<P: AsRef<Path>> NativeUdsClient<P> {
    #[must_use]
    pub fn new(socket: P) -> Self {
        Self {
            socket,
            next_request_id: AtomicU64::new(1),
        }
    }

    /// Read successive validated stage frames from one native effort intent.
    pub fn query_effort_stream(
        &self,
        query: NativeEffortStreamQuery,
        budget: &RequestBudgetV1,
        observe: &mut dyn FnMut(NativeEffortStreamStage<'_>) -> Result<(), NativeUdsError>,
    ) -> Result<NativeEffortDone, NativeUdsError> {
        if !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&query.text_query.top_k)
            || !(1..=MAX_NATIVE_SEMANTIC_TOP_K).contains(&query.semantic_query.top_k)
            || query.semantic_query.query_text.len() > MAX_NATIVE_SEMANTIC_QUERY_BYTES
        {
            return Err(NativeUdsError::Protocol(
                "native effort query exceeds request bounds".into(),
            ));
        }
        stream_budget_checkpoint(budget)?;
        let request_id = allocate_request_id(&self.next_request_id)?;
        let overlay = query.overlay;
        let selection = query.selection;
        let text_instance = query.text_instance;
        let semantic_instance = query.semantic_instance;
        let text_top_k = native_text_result_limit(&query.text_query)?;
        let semantic_top_k = query.semantic_query.top_k;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::QueryEffortStream {
                overlay,
                selection,
                text_instance,
                semantic_instance,
                text_query: query.text_query,
                semantic_query: query.semantic_query,
                preference: query.preference,
            },
        };
        let mut stream = send_stream_request(self.socket.as_ref(), &envelope, budget)
            .map_err(|error| classify_stream_error(budget, &error))?;
        let mut fast_view = None;
        let mut fast_overlay = None;
        let mut deep_seen = false;
        loop {
            stream_budget_checkpoint(budget)?;
            let reply: NativeIndexResponseEnvelope = stream
                .next_response()
                .map_err(|error| classify_stream_error(budget, &error))?;
            stream_budget_checkpoint(budget)?;
            if reply.request_id != request_id {
                return Err(NativeUdsError::Protocol("stream request id differs".into()));
            }
            match reply.payload {
                NativeIndexResponse::TextRanked(result)
                    if overlay.is_none() && fast_view.is_none() =>
                {
                    validate_text_result(selection, text_instance, text_top_k, &result)?;
                    fast_view = Some(result.view);
                    stream_budget_checkpoint(budget)?;
                    observe(NativeEffortStreamStage::Fast(&result))?;
                    stream_budget_checkpoint(budget)?;
                }
                NativeIndexResponse::OverlayTextRanked(result)
                    if overlay.is_some() && fast_view.is_none() =>
                {
                    let context = overlay.ok_or_else(|| {
                        NativeUdsError::Protocol("overlay effort context missing".into())
                    })?;
                    validate_overlay_text_result(
                        selection,
                        text_instance,
                        context.session,
                        context.expected_corpus,
                        text_top_k,
                        &result,
                    )?;
                    fast_view = Some(result.view);
                    stream_budget_checkpoint(budget)?;
                    observe(NativeEffortStreamStage::OverlayFast(&result))?;
                    stream_budget_checkpoint(budget)?;
                    fast_overlay = Some(result);
                }
                NativeIndexResponse::SemanticRanked(result)
                    if overlay.is_none() && fast_view.is_some() && !deep_seen =>
                {
                    validate_semantic_result(
                        selection,
                        semantic_instance,
                        semantic_top_k,
                        &result,
                    )?;
                    if fast_view != Some(result.view) {
                        return Err(NativeUdsError::Protocol("stream Deep changed view".into()));
                    }
                    deep_seen = true;
                    stream_budget_checkpoint(budget)?;
                    observe(NativeEffortStreamStage::Deep(&result))?;
                    stream_budget_checkpoint(budget)?;
                }
                NativeIndexResponse::OverlaySemanticRanked(result)
                    if overlay.is_some() && fast_overlay.is_some() && !deep_seen =>
                {
                    let context = overlay.ok_or_else(|| {
                        NativeUdsError::Protocol("overlay effort context missing".into())
                    })?;
                    validate_overlay_semantic_result(
                        selection,
                        semantic_instance,
                        context.session,
                        context.expected_corpus,
                        semantic_top_k,
                        &result,
                    )?;
                    if fast_view != Some(result.view)
                        || fast_overlay
                            .as_ref()
                            .map(|fast: &NativeOverlayTextResult| &fast.corpus)
                            != Some(&result.corpus)
                    {
                        return Err(NativeUdsError::Protocol(
                            "overlay effort Deep changed selected view or corpus".into(),
                        ));
                    }
                    deep_seen = true;
                    stream_budget_checkpoint(budget)?;
                    observe(NativeEffortStreamStage::OverlayDeep(&result))?;
                    stream_budget_checkpoint(budget)?;
                }
                NativeIndexResponse::EffortDone(done)
                    if fast_view == Some(done.view)
                        && deep_seen == done.deep_delivered
                        && match overlay {
                            None => done.corpus.is_none(),
                            Some(_) => fast_overlay.as_ref().is_some_and(
                                |fast: &NativeOverlayTextResult| {
                                    done.corpus.as_ref() == Some(&fast.corpus)
                                },
                            ),
                        } =>
                {
                    stream_budget_checkpoint(budget)?;
                    return Ok(done);
                }
                NativeIndexResponse::Error(error) => return Err(NativeUdsError::Refused(error)),
                NativeIndexResponse::Created(_)
                | NativeIndexResponse::InputOpened(_)
                | NativeIndexResponse::Staged(_)
                | NativeIndexResponse::StagedSources(_)
                | NativeIndexResponse::MutationPageStaged(_)
                | NativeIndexResponse::MutationsStaged(_)
                | NativeIndexResponse::InputDiscarded(_)
                | NativeIndexResponse::LiteralPage(_)
                | NativeIndexResponse::PathPage(_)
                | NativeIndexResponse::OverlayTextRanked(_)
                | NativeIndexResponse::OverlaySemanticRanked(_)
                | NativeIndexResponse::TextRanked(_)
                | NativeIndexResponse::SemanticRanked(_)
                | NativeIndexResponse::EffortDone(_)
                | NativeIndexResponse::InteractionBegun(_)
                | NativeIndexResponse::InteractionAccepted(_)
                | NativeIndexResponse::Submitted(_)
                | NativeIndexResponse::View(_)
                | NativeIndexResponse::Projection(_)
                | NativeIndexResponse::Stub(_)
                | NativeIndexResponse::Overlay(_)
                | NativeIndexResponse::Graph(_)
                | NativeIndexResponse::RepoMapRanked(_)
                | NativeIndexResponse::SourceOwners(_)
                | NativeIndexResponse::ProjectionOwners(_)
                | NativeIndexResponse::ProducerSourceOwners(_)
                | NativeIndexResponse::ProjectionSourceDiff(_)
                | NativeIndexResponse::Status(_)
                | NativeIndexResponse::ReadyWork(_)
                | NativeIndexResponse::Claimed(_)
                | NativeIndexResponse::Renewed(_)
                | NativeIndexResponse::ReadSession(_)
                | NativeIndexResponse::WorkStatus(_) => {
                    return Err(NativeUdsError::Protocol(
                        "stream stage order, binding or terminal invalid".into(),
                    ));
                }
            }
            stream_budget_checkpoint(budget)?;
        }
    }

    /// Sends one envelope under the original request budget.
    ///
    /// Durable writes should use the operation-scoped methods: cancellation
    /// after send can hide an accepted write, so the
    /// original operation key must be reconciled even if this raw call returns
    /// a typed cancellation refusal.
    pub fn request(
        &self,
        payload: NativeIndexRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeIndexResponse, NativeUdsError> {
        if matches!(payload, NativeIndexRequest::QueryEffortStream { .. }) {
            return Err(NativeUdsError::Protocol(
                "progressive effort requires the streaming client API".into(),
            ));
        }
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload,
        };
        self.request_wire(request_id, &envelope, budget)
    }

    /// Admit a one-shot Native publication reply under the caller's original
    /// memory scope. The returned owner retains both raw and decoded charges.
    /// Error replies remain canonical `NativeIndexResponse::Error` values so
    /// their bounded message stays charged until this owner is dropped.
    pub fn request_publication_admitted(
        &self,
        payload: NativeIndexRequest,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        if !matches!(
            &payload,
            NativeIndexRequest::BeginInput(_)
                | NativeIndexRequest::StageSource { .. }
                | NativeIndexRequest::StageSources { .. }
                | NativeIndexRequest::StageProfile { .. }
                | NativeIndexRequest::StageProjectionOutput { .. }
                | NativeIndexRequest::StageProjectionOwner { .. }
                | NativeIndexRequest::StageProjectionCertificate { .. }
                | NativeIndexRequest::StageMutationPage { .. }
                | NativeIndexRequest::SealMutations { .. }
                | NativeIndexRequest::DiscardInput(_)
                | NativeIndexRequest::Submit(_)
                | NativeIndexRequest::Resume(_)
                | NativeIndexRequest::Status(_)
                | NativeIndexRequest::ReadSession(_)
                | NativeIndexRequest::ReadView(_)
                | NativeIndexRequest::ReadProjection { .. }
                | NativeIndexRequest::LookupProjectionOwners { .. }
                | NativeIndexRequest::DiffProjectionSourceOwners { .. }
                | NativeIndexRequest::LookupSourceOwners { .. }
        ) {
            return Err(NativeUdsError::Protocol(
                "request is outside the admitted Native publication route".into(),
            ));
        }
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        let operation = publication_request_key(&payload);
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload,
        };
        let reply = self.request_publication_wire_admitted(request_id, &envelope, budget, memory);
        match (operation, reply) {
            (Some(key), Err(_error)) => Err(NativeUdsError::Unknown(key)),
            (None, Err(error)) => Err(error),
            (Some(key), Ok(reply))
                if matches!(
                    reply.response(),
                    NativeIndexResponse::Error(error)
                        if error.code == NativeIndexErrorCode::ResponseTooLarge
                            || (error.code == NativeIndexErrorCode::Cancelled
                                && budget.is_cancelled())
                ) =>
            {
                Err(NativeUdsError::Unknown(key))
            }
            (key, Ok(reply))
                if publication_response_matches_request(&envelope.payload, reply.response()) =>
            {
                Ok(reply)
            }
            (Some(key), Ok(_reply)) => Err(NativeUdsError::Unknown(key)),
            (None, Ok(_reply)) => Err(NativeUdsError::Protocol(
                "unexpected Native publication response variant".into(),
            )),
        }
    }

    /// Send borrowed source bodies while the original caller owns their bytes.
    /// The response may be `StagedSources` or a retained `Error`; the caller
    /// must classify an error before making another durable operation attempt.
    pub fn stage_sources_borrowed_admitted(
        &self,
        ingress: IngressRef,
        inputs: &[BorrowedSourceInput<'_>],
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        validate_stage_sources_inputs(inputs)?;
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = StageSourcesEnvelope {
            request_id,
            ingress,
            inputs,
        };
        let reply = self
            .request_publication_wire_admitted(request_id, &envelope, budget, memory)
            .map_err(|_error| NativeUdsError::Unknown(ingress.operation))?;
        match reply.response() {
            NativeIndexResponse::StagedSources(references) if references.len() == inputs.len() => {
                Ok(reply)
            }
            NativeIndexResponse::Error(error)
                if error.code == NativeIndexErrorCode::ResponseTooLarge
                    || (error.code == NativeIndexErrorCode::Cancelled && budget.is_cancelled()) =>
            {
                Err(NativeUdsError::Unknown(ingress.operation))
            }
            NativeIndexResponse::Error(_) => Ok(reply),
            NativeIndexResponse::Created(_)
            | NativeIndexResponse::InputOpened(_)
            | NativeIndexResponse::Staged(_)
            | NativeIndexResponse::StagedSources(_)
            | NativeIndexResponse::MutationPageStaged(_)
            | NativeIndexResponse::MutationsStaged(_)
            | NativeIndexResponse::InputDiscarded(_)
            | NativeIndexResponse::LiteralPage(_)
            | NativeIndexResponse::PathPage(_)
            | NativeIndexResponse::OverlayTextRanked(_)
            | NativeIndexResponse::OverlaySemanticRanked(_)
            | NativeIndexResponse::TextRanked(_)
            | NativeIndexResponse::SemanticRanked(_)
            | NativeIndexResponse::EffortDone(_)
            | NativeIndexResponse::InteractionBegun(_)
            | NativeIndexResponse::InteractionAccepted(_)
            | NativeIndexResponse::Submitted(_)
            | NativeIndexResponse::View(_)
            | NativeIndexResponse::Projection(_)
            | NativeIndexResponse::Stub(_)
            | NativeIndexResponse::Overlay(_)
            | NativeIndexResponse::Graph(_)
            | NativeIndexResponse::RepoMapRanked(_)
            | NativeIndexResponse::SourceOwners(_)
            | NativeIndexResponse::ProjectionOwners(_)
            | NativeIndexResponse::ProducerSourceOwners(_)
            | NativeIndexResponse::ProjectionSourceDiff(_)
            | NativeIndexResponse::Status(_)
            | NativeIndexResponse::ReadyWork(_)
            | NativeIndexResponse::Claimed(_)
            | NativeIndexResponse::Renewed(_)
            | NativeIndexResponse::ReadSession(_)
            | NativeIndexResponse::WorkStatus(_) => Err(NativeUdsError::Unknown(ingress.operation)),
        }
    }

    /// Stage one caller-owned sorted mutation page without allocating a
    /// second Vec. The caller retains the page through the admitted ACK.
    pub fn stage_mutation_page_borrowed_admitted(
        &self,
        ingress: IngressRef,
        base: MapRoot,
        mutations: &[OwnerMutation],
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        if mutations.is_empty() || mutations.len() > 128 {
            return Err(NativeUdsError::Protocol(
                "mutation page count exceeds native bounds".into(),
            ));
        }
        if mutations
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.owner >= right.owner))
        {
            return Err(NativeUdsError::Protocol(
                "mutation page must be strictly sorted by owner".into(),
            ));
        }
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = BorrowedMutationPageEnvelope {
            request_id,
            ingress,
            base,
            mutations,
        };
        let reply = self
            .request_publication_wire_admitted(request_id, &envelope, budget, memory)
            .map_err(|_error| NativeUdsError::Unknown(ingress.operation))?;
        match reply.response() {
            NativeIndexResponse::Error(error)
                if error.code == NativeIndexErrorCode::ResponseTooLarge
                    || (error.code == NativeIndexErrorCode::Cancelled && budget.is_cancelled()) =>
            {
                Err(NativeUdsError::Unknown(ingress.operation))
            }
            NativeIndexResponse::MutationPageStaged(_) | NativeIndexResponse::Error(_) => Ok(reply),
            NativeIndexResponse::Created(_)
            | NativeIndexResponse::InputOpened(_)
            | NativeIndexResponse::Staged(_)
            | NativeIndexResponse::StagedSources(_)
            | NativeIndexResponse::MutationsStaged(_)
            | NativeIndexResponse::InputDiscarded(_)
            | NativeIndexResponse::LiteralPage(_)
            | NativeIndexResponse::PathPage(_)
            | NativeIndexResponse::OverlayTextRanked(_)
            | NativeIndexResponse::OverlaySemanticRanked(_)
            | NativeIndexResponse::TextRanked(_)
            | NativeIndexResponse::SemanticRanked(_)
            | NativeIndexResponse::EffortDone(_)
            | NativeIndexResponse::InteractionBegun(_)
            | NativeIndexResponse::InteractionAccepted(_)
            | NativeIndexResponse::Submitted(_)
            | NativeIndexResponse::View(_)
            | NativeIndexResponse::Projection(_)
            | NativeIndexResponse::Stub(_)
            | NativeIndexResponse::Overlay(_)
            | NativeIndexResponse::Graph(_)
            | NativeIndexResponse::RepoMapRanked(_)
            | NativeIndexResponse::SourceOwners(_)
            | NativeIndexResponse::ProjectionOwners(_)
            | NativeIndexResponse::ProducerSourceOwners(_)
            | NativeIndexResponse::ProjectionSourceDiff(_)
            | NativeIndexResponse::Status(_)
            | NativeIndexResponse::ReadyWork(_)
            | NativeIndexResponse::Claimed(_)
            | NativeIndexResponse::Renewed(_)
            | NativeIndexResponse::ReadSession(_)
            | NativeIndexResponse::WorkStatus(_) => Err(NativeUdsError::Unknown(ingress.operation)),
        }
    }

    /// Submit the exact immutable request without copying it into a new wire
    /// request. `Submitted` and bounded `Error` keep the original reply charge.
    pub fn submit_borrowed_admitted(
        &self,
        request: &UpdateRequest,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        let key = request.operation;
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = BorrowedSubmitEnvelope {
            request_id,
            request,
        };
        let reply = self
            .request_publication_wire_admitted(request_id, &envelope, budget, memory)
            .map_err(|_error| NativeUdsError::Unknown(key))?;
        match reply.response() {
            NativeIndexResponse::Submitted(response)
                if validate_submit_reply(key, response.clone()).is_ok() =>
            {
                Ok(reply)
            }
            NativeIndexResponse::Error(error)
                if error.code == NativeIndexErrorCode::ResponseTooLarge
                    || (error.code == NativeIndexErrorCode::Cancelled && budget.is_cancelled()) =>
            {
                Err(NativeUdsError::Unknown(key))
            }
            NativeIndexResponse::Error(_) => Ok(reply),
            NativeIndexResponse::Created(_)
            | NativeIndexResponse::InputOpened(_)
            | NativeIndexResponse::Staged(_)
            | NativeIndexResponse::StagedSources(_)
            | NativeIndexResponse::MutationPageStaged(_)
            | NativeIndexResponse::MutationsStaged(_)
            | NativeIndexResponse::InputDiscarded(_)
            | NativeIndexResponse::LiteralPage(_)
            | NativeIndexResponse::PathPage(_)
            | NativeIndexResponse::OverlayTextRanked(_)
            | NativeIndexResponse::OverlaySemanticRanked(_)
            | NativeIndexResponse::TextRanked(_)
            | NativeIndexResponse::SemanticRanked(_)
            | NativeIndexResponse::EffortDone(_)
            | NativeIndexResponse::InteractionBegun(_)
            | NativeIndexResponse::InteractionAccepted(_)
            | NativeIndexResponse::Submitted(_)
            | NativeIndexResponse::View(_)
            | NativeIndexResponse::Projection(_)
            | NativeIndexResponse::Stub(_)
            | NativeIndexResponse::Overlay(_)
            | NativeIndexResponse::Graph(_)
            | NativeIndexResponse::RepoMapRanked(_)
            | NativeIndexResponse::SourceOwners(_)
            | NativeIndexResponse::ProjectionOwners(_)
            | NativeIndexResponse::ProducerSourceOwners(_)
            | NativeIndexResponse::ProjectionSourceDiff(_)
            | NativeIndexResponse::Status(_)
            | NativeIndexResponse::ReadyWork(_)
            | NativeIndexResponse::Claimed(_)
            | NativeIndexResponse::Renewed(_)
            | NativeIndexResponse::ReadSession(_)
            | NativeIndexResponse::WorkStatus(_) => Err(NativeUdsError::Unknown(key)),
        }
    }

    fn request_publication_wire_admitted<E: serde::Serialize>(
        &self,
        request_id: u64,
        envelope: &E,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        self.request_wire_admitted(request_id, envelope, budget, memory, false)
    }

    fn request_wire_admitted<E: serde::Serialize>(
        &self,
        request_id: u64,
        envelope: &E,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
        stub: bool,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        let body =
            send_request_body_with_budget_admitted(self.socket.as_ref(), envelope, budget, memory)
                .map_err(|error| {
                    if budget.is_cancelled() {
                        cancelled_error()
                    } else {
                        NativeUdsError::Transport(error.to_string())
                    }
                })?;
        // The type-specific CBOR preflight rejects indefinite text and caps
        // every nested canonical map/enum identifier to a known <=32-byte
        // token before Ciborium copies it. The temporary charge covers one
        // live identifier per bounded recursion level plus one diagnostic.
        let _parser = memory
            .reserve(
                admitted_reply_v1::parser_temporary_limit()?,
                MemoryLifetime::Temporary,
            )
            .map_err(|error| NativeUdsError::Transport(error.to_string()))?;
        let decoded_limit = if stub {
            admitted_reply_v1::stub_decoded_retained_limit()?
        } else {
            admitted_reply_v1::decoded_retained_limit()?
        };
        let decoded = memory
            .reserve(decoded_limit, MemoryLifetime::Retained)
            .map_err(|error| NativeUdsError::Transport(error.to_string()))?;
        let reply = if stub {
            admitted_reply_v1::decode_stub(body.as_slice())?
        } else {
            admitted_reply_v1::decode(body.as_slice())?
        };
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        if reply.request_id != request_id {
            return Err(NativeUdsError::Protocol(
                "response request id differs".into(),
            ));
        }
        Ok(AdmittedNativePublicationReply {
            response: reply.payload,
            _body: body,
            _decoded: decoded,
        })
    }

    fn request_wire<E: serde::Serialize>(
        &self,
        request_id: u64,
        envelope: &E,
        budget: &RequestBudgetV1,
    ) -> Result<NativeIndexResponse, NativeUdsError> {
        let reply: NativeIndexResponseEnvelope =
            send_request_with_budget(self.socket.as_ref(), envelope, budget).map_err(|error| {
                if budget.is_cancelled() {
                    cancelled_error()
                } else {
                    NativeUdsError::Transport(error.to_string())
                }
            })?;
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        if reply.request_id != request_id {
            return Err(NativeUdsError::Protocol(
                "response request id differs".into(),
            ));
        }
        match reply.payload {
            NativeIndexResponse::Error(error) => Err(NativeUdsError::Refused(error)),
            NativeIndexResponse::EffortDone(_) => Err(NativeUdsError::Protocol(
                "unexpected stream terminal on one-shot request".into(),
            )),
            response @ (NativeIndexResponse::Created(_)
            | NativeIndexResponse::InputOpened(_)
            | NativeIndexResponse::Staged(_)
            | NativeIndexResponse::StagedSources(_)
            | NativeIndexResponse::MutationPageStaged(_)
            | NativeIndexResponse::MutationsStaged(_)
            | NativeIndexResponse::InputDiscarded(_)
            | NativeIndexResponse::LiteralPage(_)
            | NativeIndexResponse::PathPage(_)
            | NativeIndexResponse::OverlayTextRanked(_)
            | NativeIndexResponse::OverlaySemanticRanked(_)
            | NativeIndexResponse::TextRanked(_)
            | NativeIndexResponse::SemanticRanked(_)
            | NativeIndexResponse::InteractionBegun(_)
            | NativeIndexResponse::InteractionAccepted(_)
            | NativeIndexResponse::Submitted(_)
            | NativeIndexResponse::View(_)
            | NativeIndexResponse::Projection(_)
            | NativeIndexResponse::Stub(_)
            | NativeIndexResponse::Overlay(_)
            | NativeIndexResponse::Graph(_)
            | NativeIndexResponse::RepoMapRanked(_)
            | NativeIndexResponse::SourceOwners(_)
            | NativeIndexResponse::ProjectionOwners(_)
            | NativeIndexResponse::ProducerSourceOwners(_)
            | NativeIndexResponse::ProjectionSourceDiff(_)
            | NativeIndexResponse::Status(_)
            | NativeIndexResponse::ReadyWork(_)
            | NativeIndexResponse::Claimed(_)
            | NativeIndexResponse::Renewed(_)
            | NativeIndexResponse::ReadSession(_)
            | NativeIndexResponse::WorkStatus(_)) => Ok(response),
        }
    }

    /// Acquire a finite foreground publisher session. Begin alone does not
    /// report user activity; an actual event must follow before Auto has an
    /// interaction idle clock. Abrupt publisher exit becomes Unknown at lease
    /// expiry because each native UDS request has a separate connection.
    pub fn begin_interaction_publisher(
        &self,
        window: Duration,
        budget: &RequestBudgetV1,
    ) -> Result<NativeInteractionPublisherId, NativeUdsError> {
        let millis = u64::try_from(window.as_millis()).map_err(|_overflow| {
            NativeUdsError::Protocol("interaction publisher window overflow".into())
        })?;
        if !(1..=60_000).contains(&millis) || Duration::from_millis(millis) != window {
            return Err(NativeUdsError::Protocol(
                "interaction publisher window must be exact 1..=60000 ms".into(),
            ));
        }
        let NativeIndexResponse::InteractionBegun(publisher) = self.request(
            NativeIndexRequest::InteractionBegin {
                window_millis: millis,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected interaction begin reply".into(),
            ));
        };
        if publisher.0 == [0; 32] {
            return Err(NativeUdsError::Protocol(
                "interaction begin returned a zero publisher identity".into(),
            ));
        }
        Ok(publisher)
    }

    /// Report a real foreground interaction under the original request budget.
    pub fn report_interaction_activity(
        &self,
        publisher: NativeInteractionPublisherId,
        sequence: u64,
        budget: &RequestBudgetV1,
    ) -> Result<(), NativeUdsError> {
        self.interaction_update(
            NativeIndexRequest::InteractionActivity {
                publisher,
                sequence,
            },
            publisher,
            sequence,
            budget,
        )
    }

    /// Extend a live publisher lease without inventing an activity event.
    pub fn renew_interaction_publisher(
        &self,
        publisher: NativeInteractionPublisherId,
        sequence: u64,
        budget: &RequestBudgetV1,
    ) -> Result<(), NativeUdsError> {
        self.interaction_update(
            NativeIndexRequest::InteractionRenew {
                publisher,
                sequence,
            },
            publisher,
            sequence,
            budget,
        )
    }

    /// Explicitly invalidate a publisher, immediately restoring Unknown.
    pub fn end_interaction_publisher(
        &self,
        publisher: NativeInteractionPublisherId,
        sequence: u64,
        budget: &RequestBudgetV1,
    ) -> Result<(), NativeUdsError> {
        self.interaction_update(
            NativeIndexRequest::InteractionEnd {
                publisher,
                sequence,
            },
            publisher,
            sequence,
            budget,
        )
    }

    fn interaction_update(
        &self,
        request: NativeIndexRequest,
        publisher: NativeInteractionPublisherId,
        sequence: u64,
        budget: &RequestBudgetV1,
    ) -> Result<(), NativeUdsError> {
        if sequence == 0 || publisher.0 == [0; 32] {
            return Err(NativeUdsError::Protocol(
                "interaction update requires a nonzero publisher and sequence".into(),
            ));
        }
        let NativeIndexResponse::InteractionAccepted(accepted) = self.request(request, budget)?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected interaction update reply".into(),
            ));
        };
        if accepted != publisher {
            return Err(NativeUdsError::Protocol(
                "interaction update ACK changed publisher identity".into(),
            ));
        }
        Ok(())
    }

    fn write_stage(
        &self,
        key: OperationKey,
        payload: NativeIndexRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeIndexResponse, NativeUdsError> {
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        classify_write_stage_result(key, budget, self.request(payload, budget))
    }

    /// Opens a durable operation-scoped input stage. An ambiguous ACK keeps
    /// the same operation identity; callers must not allocate a replacement.
    pub fn begin_input(
        &self,
        key: OperationKey,
        budget: &RequestBudgetV1,
    ) -> Result<IngressRef, NativeUdsError> {
        let response = self.write_stage(key, NativeIndexRequest::BeginInput(key), budget)?;
        let NativeIndexResponse::InputOpened(ingress) = response else {
            return Err(NativeUdsError::Unknown(key));
        };
        if ingress.operation != key {
            return Err(NativeUdsError::Unknown(key));
        }
        Ok(ingress)
    }

    /// Stage one source with the caller's explicit text admission.
    pub fn stage_source(
        &self,
        ingress: IngressRef,
        path: String,
        bytes: &[u8],
        text: SourceTextAdmission,
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        let bytes = copy_stage_bytes(bytes, budget)?;
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageSource {
                    ingress,
                    path,
                    bytes,
                    text,
                },
                budget,
            )?,
            ingress.operation,
        )
    }

    /// Uploads one bounded child-first source batch without a per-file UDS fallback.
    pub fn stage_sources(
        &self,
        ingress: IngressRef,
        inputs: Vec<SourceInput>,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<ObjectRef>, NativeUdsError> {
        let result = self.stage_sources_view(ingress, &inputs, budget);
        drop(inputs);
        result
    }

    /// Stage a source batch from caller-owned paths and bytes. The caller must
    /// retain both until this method returns; the IPC frame and response are
    /// still materialized by the common transport under the original budget.
    pub fn stage_sources_borrowed(
        &self,
        ingress: IngressRef,
        inputs: &[BorrowedSourceInput<'_>],
        budget: &RequestBudgetV1,
    ) -> Result<Vec<ObjectRef>, NativeUdsError> {
        self.stage_sources_view(ingress, inputs, budget)
    }

    fn stage_sources_view<T: SourceInputView>(
        &self,
        ingress: IngressRef,
        inputs: &[T],
        budget: &RequestBudgetV1,
    ) -> Result<Vec<ObjectRef>, NativeUdsError> {
        validate_stage_sources_inputs(inputs)?;
        let count = inputs.len();
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = StageSourcesEnvelope {
            request_id,
            ingress,
            inputs,
        };
        let response = classify_write_stage_result(
            ingress.operation,
            budget,
            self.request_wire(request_id, &envelope, budget),
        )?;
        let NativeIndexResponse::StagedSources(references) = response else {
            return Err(NativeUdsError::Unknown(ingress.operation));
        };
        if references.len() != count {
            return Err(NativeUdsError::Unknown(ingress.operation));
        }
        Ok(references)
    }

    pub fn stage_profile(
        &self,
        ingress: IngressRef,
        profile: IndexProfile,
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageProfile { ingress, profile },
                budget,
            )?,
            ingress.operation,
        )
    }

    pub fn stage_projection_output(
        &self,
        ingress: IngressRef,
        bytes: &[u8],
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        self.stage_projection_output_with_children(ingress, bytes, &[], budget)
    }

    pub fn stage_projection_output_with_children(
        &self,
        ingress: IngressRef,
        bytes: &[u8],
        children: &[ObjectRef],
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        if children.len()
            > quanta_index_contract::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN
            || children
                .windows(2)
                .any(|pair| matches!(pair, [left, right] if left >= right))
        {
            return Err(NativeUdsError::Protocol(
                "projection output children exceed bounds or canonical order".into(),
            ));
        }
        stream_budget_checkpoint(budget)?;
        let mut copied = Vec::new();
        copied.try_reserve_exact(children.len()).map_err(|_error| {
            NativeUdsError::Protocol("projection child allocation refused".into())
        })?;
        copied.extend_from_slice(children);
        let bytes = copy_stage_bytes(bytes, budget)?;
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageProjectionOutput {
                    ingress,
                    bytes,
                    children:
                        quanta_index_contract::native_protocol::NativeProjectionOutputChildren::new(
                            copied,
                        )
                        .map_err(|cause| NativeUdsError::Protocol(cause.into()))?,
                },
                budget,
            )?,
            ingress.operation,
        )
    }

    pub fn stage_projection_owner(
        &self,
        ingress: IngressRef,
        contribution: ProjectionOwnerContribution,
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageProjectionOwner {
                    ingress,
                    contribution,
                },
                budget,
            )?,
            ingress.operation,
        )
    }

    pub fn stage_projection_certificate(
        &self,
        ingress: IngressRef,
        certificate: ProjectionCertificate,
        budget: &RequestBudgetV1,
    ) -> Result<ObjectRef, NativeUdsError> {
        staged_reference(
            &self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageProjectionCertificate {
                    ingress,
                    certificate,
                },
                budget,
            )?,
            ingress.operation,
        )
    }

    /// Streams at most 128 owner mutations per UDS request. Each page is
    /// canonicalized locally; the persistent map applies absent-CAS across
    /// pages and its sealed root defines the complete sorted mutation set.
    /// The caller owns source capture and source-body sizing.
    pub fn stage_mutations<I>(
        &self,
        ingress: IngressRef,
        mutations: I,
        budget: &RequestBudgetV1,
    ) -> Result<OwnerMutationSet, NativeUdsError>
    where
        I: IntoIterator<Item = OwnerMutation>,
    {
        self.stage_mutations_fallible(
            ingress,
            mutations
                .into_iter()
                .map(Ok::<OwnerMutation, NativeUdsError>),
            budget,
        )
    }

    /// Streams fallible source-backed mutations without sealing a partial
    /// mutation root when the producer fails between bounded pages. Input
    /// order need not match owner-key order across pages.
    pub fn stage_mutations_fallible<I>(
        &self,
        ingress: IngressRef,
        mutations: I,
        budget: &RequestBudgetV1,
    ) -> Result<OwnerMutationSet, NativeUdsError>
    where
        I: IntoIterator<Item = Result<OwnerMutation, NativeUdsError>>,
    {
        let mut input = mutations.into_iter();
        let mut root = MapRoot(None);
        let mut count = 0u64;
        loop {
            let mut page: Vec<_> = input.by_ref().take(128).collect::<Result<_, _>>()?;
            if page.is_empty() {
                break;
            }
            page.sort_unstable_by_key(|mutation| mutation.owner);
            if page
                .windows(2)
                .any(|pair| matches!(pair, [left, right] if left.owner == right.owner))
            {
                return Err(NativeUdsError::Protocol(
                    "source mutation page has duplicate owner".into(),
                ));
            }
            count = count
                .checked_add(u64::try_from(page.len()).map_err(|_error| {
                    NativeUdsError::Protocol("mutation page length overflow".into())
                })?)
                .ok_or_else(|| NativeUdsError::Protocol("mutation count overflow".into()))?;
            let response = self.write_stage(
                ingress.operation,
                NativeIndexRequest::StageMutationPage {
                    ingress,
                    base: root,
                    mutations: page,
                },
                budget,
            )?;
            let NativeIndexResponse::MutationPageStaged(next) = response else {
                return Err(NativeUdsError::Unknown(ingress.operation));
            };
            root = next;
        }
        let response = self.write_stage(
            ingress.operation,
            NativeIndexRequest::SealMutations {
                ingress,
                root,
                count,
            },
            budget,
        )?;
        let NativeIndexResponse::MutationsStaged(set) = response else {
            return Err(NativeUdsError::Unknown(ingress.operation));
        };
        if set.root != root || set.count != count {
            return Err(NativeUdsError::Unknown(ingress.operation));
        }
        Ok(set)
    }

    pub fn discard_input(
        &self,
        ingress: IngressRef,
        budget: &RequestBudgetV1,
    ) -> Result<(), NativeUdsError> {
        let response = self.write_stage(
            ingress.operation,
            NativeIndexRequest::DiscardInput(ingress),
            budget,
        )?;
        let NativeIndexResponse::InputDiscarded(ack) = response else {
            return Err(NativeUdsError::Unknown(ingress.operation));
        };
        if ack != ingress {
            return Err(NativeUdsError::Unknown(ingress.operation));
        }
        Ok(())
    }

    /// On transport loss the original operation may already be admitted.
    /// The caller must reconcile this exact key with `status`.
    pub fn submit(
        &self,
        request: UpdateRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        let result = self.submit_borrowed(&request, budget);
        drop(request);
        result
    }

    /// Sends the exact caller-retained immutable request. On an uncertain
    /// response, the caller must retain it and reconcile the original key.
    pub fn submit_borrowed(
        &self,
        request: &UpdateRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        let key = request.operation;
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = BorrowedSubmitEnvelope {
            request_id,
            request,
        };
        classify_submit_result(
            self.request_wire(request_id, &envelope, budget),
            key,
            budget,
        )
    }

    /// Replays only the admitted immutable request associated with this key.
    /// An uncertain response must be reconciled with `status` or retried with
    /// the same key; constructing a new operation changes its identity.
    pub fn resume(
        &self,
        key: OperationKey,
        budget: &RequestBudgetV1,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        self.submit_like(NativeIndexRequest::Resume(key), key, budget)
    }

    fn submit_like(
        &self,
        request: NativeIndexRequest,
        key: OperationKey,
        budget: &RequestBudgetV1,
    ) -> Result<SubmitOutcome, NativeUdsError> {
        if budget.is_cancelled() {
            return Err(cancelled_error());
        }
        classify_submit_result(self.request(request, budget), key, budget)
    }

    pub fn status(
        &self,
        key: OperationKey,
        budget: &RequestBudgetV1,
    ) -> Result<OperationStatus, NativeUdsError> {
        match self.request(NativeIndexRequest::Status(key), budget)? {
            NativeIndexResponse::Status(reply) => validate_status_reply(key, reply),
            NativeIndexResponse::Created(_)
            | NativeIndexResponse::InputOpened(_)
            | NativeIndexResponse::Staged(_)
            | NativeIndexResponse::StagedSources(_)
            | NativeIndexResponse::MutationPageStaged(_)
            | NativeIndexResponse::MutationsStaged(_)
            | NativeIndexResponse::InputDiscarded(_)
            | NativeIndexResponse::LiteralPage(_)
            | NativeIndexResponse::PathPage(_)
            | NativeIndexResponse::OverlayTextRanked(_)
            | NativeIndexResponse::OverlaySemanticRanked(_)
            | NativeIndexResponse::TextRanked(_)
            | NativeIndexResponse::SemanticRanked(_)
            | NativeIndexResponse::Submitted(_)
            | NativeIndexResponse::View(_)
            | NativeIndexResponse::Projection(_)
            | NativeIndexResponse::Stub(_)
            | NativeIndexResponse::Overlay(_)
            | NativeIndexResponse::Graph(_)
            | NativeIndexResponse::RepoMapRanked(_)
            | NativeIndexResponse::SourceOwners(_)
            | NativeIndexResponse::ProjectionOwners(_)
            | NativeIndexResponse::ProducerSourceOwners(_)
            | NativeIndexResponse::ProjectionSourceDiff(_)
            | NativeIndexResponse::ReadyWork(_)
            | NativeIndexResponse::Claimed(_)
            | NativeIndexResponse::Renewed(_)
            | NativeIndexResponse::ReadSession(_)
            | NativeIndexResponse::WorkStatus(_)
            | NativeIndexResponse::EffortDone(_)
            | NativeIndexResponse::InteractionBegun(_)
            | NativeIndexResponse::InteractionAccepted(_)
            | NativeIndexResponse::Error(_) => {
                Err(NativeUdsError::Protocol("unexpected status reply".into()))
            }
        }
    }

    pub fn query_graph(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: NativeGraphQuery,
        budget: &RequestBudgetV1,
    ) -> Result<NativeGraphResult, NativeUdsError> {
        if query.max_edges == 0
            || query.max_edges > quanta_index_contract::native_protocol::MAX_NATIVE_GRAPH_EDGES
            || query.max_supporters == 0
            || usize::try_from(query.max_supporters).map_or(true, |count| {
                count > quanta_index_contract::native_protocol::MAX_NATIVE_GRAPH_ITEMS
            })
        {
            return Err(NativeUdsError::Protocol(
                "graph query exceeds request bounds".into(),
            ));
        }
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::QueryGraph {
                selection,
                instance,
                query,
            },
        };
        let NativeIndexResponse::Graph(result) =
            self.request_wire(request_id, &envelope, budget)?
        else {
            return Err(NativeUdsError::Protocol("unexpected graph reply".into()));
        };
        let NativeIndexRequest::QueryGraph { query, .. } = &envelope.payload else {
            return Err(NativeUdsError::Protocol(
                "graph request context changed".into(),
            ));
        };
        let row_count = usize::try_from(query.max_edges)
            .map_err(|_error| NativeUdsError::Protocol("graph edge bound overflows".into()))?;
        if !selection_matches(selection, result.view)
            || result.instance != instance
            || !result.exhaustive
            || result.edges.len() > row_count
            || result.source_supporters > query.max_supporters
            || result
                .source
                .as_ref()
                .is_some_and(|source| source.identity != query.source)
            || (result.source.is_none()
                && (result.source_supporters != 0 || !result.edges.is_empty()))
            || (result.source.is_some() && result.source_supporters == 0)
            || result.edges.iter().any(|row| {
                row.supporters == 0
                    || row.supporters > query.max_supporters
                    || query.direction.endpoint(&row.edge) != &query.source
            })
        {
            return Err(NativeUdsError::Protocol(
                "graph reply differs from selected request".into(),
            ));
        }
        if result.edges.iter().enumerate().any(|(ordinal, row)| {
            result
                .edges
                .iter()
                .take(ordinal)
                .any(|prior| prior.edge == row.edge)
        }) {
            return Err(NativeUdsError::Protocol(
                "graph reply contains duplicate adjacency".into(),
            ));
        }
        Ok(result)
    }

    pub fn bind_overlay_save(
        &self,
        intent: NativeOverlaySaveIntent,
        budget: &RequestBudgetV1,
    ) -> Result<NativeOverlayReply, NativeUdsError> {
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::BindOverlaySave(intent),
        };
        let NativeIndexResponse::Overlay(reply) =
            self.request_wire(request_id, &envelope, budget)?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected save binding reply".into(),
            ));
        };
        let NativeIndexRequest::BindOverlaySave(intent) = &envelope.payload else {
            return Err(NativeUdsError::Protocol("save request binding lost".into()));
        };
        if !matches!(&reply, NativeOverlayReply::SaveBound {document,revision,operation}
            if *document == intent.document && *revision == intent.revision && *operation == intent.operation())
        {
            return Err(NativeUdsError::Protocol(
                "save reply differs from original request/revision".into(),
            ));
        }
        Ok(reply)
    }

    pub fn complete_overlay_save(
        &self,
        receipt: &NativeOverlaySaveReceipt,
        budget: &RequestBudgetV1,
    ) -> Result<NativeOverlayReply, NativeUdsError> {
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::CompleteOverlaySave(*receipt),
        };
        let NativeIndexResponse::Overlay(reply) =
            self.request_wire(request_id, &envelope, budget)?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected save completion reply".into(),
            ));
        };
        if !matches!(&reply, NativeOverlayReply::Saved {document, revision, selected}
            if *document == receipt.document && *revision == receipt.revision && *selected == receipt.selected)
        {
            return Err(NativeUdsError::Protocol(
                "save completion differs from authenticated receipt".into(),
            ));
        }
        Ok(reply)
    }

    /// Certify or preflight against the caller's authenticated source body.
    /// `exact_source` must be resolved from `binding.source` under the caller's
    /// selected-source custody. The source owner must still compare the full
    /// binding and source immediately before an atomic save/CAS.
    pub fn stub(
        &self,
        request: NativeStubRequest,
        exact_source: &str,
        budget: &RequestBudgetV1,
    ) -> Result<NativeStubReply, NativeUdsError> {
        stream_budget_checkpoint(budget)?;
        stub_reply::validate_request(&request, exact_source)?;
        stream_budget_checkpoint(budget)?;
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::Stub(request),
        };
        let response = self.request_wire(request_id, &envelope, budget)?;
        let NativeIndexResponse::Stub(reply) = response else {
            return Err(NativeUdsError::Protocol("unexpected Stub reply".into()));
        };
        let NativeIndexRequest::Stub(request) = &envelope.payload else {
            return Err(NativeUdsError::Protocol("Stub request binding lost".into()));
        };
        stub_reply::validate_reply(request, &reply, exact_source)?;
        stream_budget_checkpoint(budget)?;
        Ok(reply)
    }

    /// Funded Stub preflight. The caller supplies the selected source's exact
    /// authenticated body; both request and response are checked against it.
    /// Retain the returned owner through consumer validation and Delta issue.
    pub fn stub_admitted(
        &self,
        request: NativeStubRequest,
        exact_source: &str,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativeStubReply, NativeUdsError> {
        stream_budget_checkpoint(budget)?;
        if !matches!(&request, NativeStubRequest::Preflight { .. }) {
            return Err(NativeUdsError::Protocol(
                "funded Stub entry requires Preflight".into(),
            ));
        }
        stub_reply::validate_request(&request, exact_source)?;
        stream_budget_checkpoint(budget)?;
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::Stub(request),
        };
        let owner = self.request_wire_admitted(request_id, &envelope, budget, memory, true)?;
        if let NativeIndexResponse::Error(error) = owner.response() {
            return Err(NativeUdsError::Refused(error.clone()));
        }
        let NativeIndexResponse::Stub(reply) = owner.response() else {
            return Err(NativeUdsError::Protocol("unexpected Stub reply".into()));
        };
        let NativeIndexRequest::Stub(request) = envelope.payload else {
            return Err(NativeUdsError::Protocol("Stub request binding lost".into()));
        };
        stub_reply::validate_reply(&request, reply, exact_source)?;
        stream_budget_checkpoint(budget)?;
        let AdmittedNativePublicationReply {
            response,
            _body: body,
            _decoded: decoded,
        } = owner;
        let NativeIndexResponse::Stub(reply) = response else {
            return Err(NativeUdsError::Protocol("unexpected Stub reply".into()));
        };
        Ok(AdmittedNativeStubReply {
            request,
            reply,
            _body: body,
            _decoded: decoded,
        })
    }

    pub fn overlay(
        &self,
        request: NativeOverlayRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeOverlayReply, NativeUdsError> {
        let request_id = allocate_request_id(&self.next_request_id)?;
        let envelope = NativeIndexRequestEnvelope {
            request_id,
            payload: NativeIndexRequest::Overlay(request),
        };
        let NativeIndexResponse::Overlay(reply) =
            self.request_wire(request_id, &envelope, budget)?
        else {
            return Err(NativeUdsError::Protocol("unexpected overlay reply".into()));
        };
        let NativeIndexRequest::Overlay(request) = &envelope.payload else {
            return Err(NativeUdsError::Protocol(
                "overlay request context changed".into(),
            ));
        };
        let matches = match (request, &reply) {
            (
                NativeOverlayRequest::Masks { session },
                NativeOverlayReply::Masks {
                    session: actual,
                    masks,
                },
            ) => {
                session == actual
                    && masks.len() <= 256
                    && masks.iter().all(|row| row.revision > 0)
                    && masks
                        .iter()
                        .zip(masks.iter().skip(1))
                        .all(|(left, right)| left.owner < right.owner)
            }
            (NativeOverlayRequest::Begin { session }, NativeOverlayReply::Begun(actual))
            | (NativeOverlayRequest::Close { session }, NativeOverlayReply::Closed(actual)) => {
                session == actual
            }
            (
                NativeOverlayRequest::Update {
                    document, revision, ..
                },
                NativeOverlayReply::Updated {
                    document: actual,
                    revision: actual_revision,
                },
            ) => document == actual && revision == actual_revision,
            (
                NativeOverlayRequest::Snapshot { document },
                NativeOverlayReply::Snapshot {
                    document: actual,
                    overlay,
                },
            ) => {
                document == actual
                    && overlay.as_ref().is_none_or(|item| {
                        item.revision > 0
                            && item
                                .text
                                .as_ref()
                                .is_none_or(|text| text.len() <= 1024 * 1024)
                            && item
                                .path
                                .as_ref()
                                .is_none_or(|path| valid_native_source_path(path))
                            && (item.text.is_some() || item.path.is_none())
                    })
            }
            _ => false,
        };
        if !matches {
            return Err(NativeUdsError::Protocol(
                "overlay reply identity or revision differs".into(),
            ));
        }
        Ok(reply)
    }

    /// Update one overlay revision with an exact source path for unsaved owners.
    pub fn update_overlay_at_path(
        &self,
        document: quanta_index_contract::native_protocol::NativeOverlayDocument,
        expected_revision: Option<u64>,
        revision: u64,
        text: Option<String>,
        path: Option<String>,
        budget: &RequestBudgetV1,
    ) -> Result<NativeOverlayReply, NativeUdsError> {
        self.overlay(
            NativeOverlayRequest::Update {
                document,
                expected_revision,
                revision,
                text,
                path,
            },
            budget,
        )
    }

    pub fn read_view(
        &self,
        selection: ViewSelection,
        budget: &RequestBudgetV1,
    ) -> Result<CatalogView, NativeUdsError> {
        let NativeIndexResponse::View(view) =
            self.request(NativeIndexRequest::ReadView(selection), budget)?
        else {
            return Err(NativeUdsError::Protocol("unexpected view reply".into()));
        };
        let digest = view
            .identity_digest()
            .map_err(|error| NativeUdsError::Protocol(format!("view digest: {error}")))?;
        if view.identity.digest != digest {
            return Err(NativeUdsError::Protocol(
                "view identity digest mismatch".into(),
            ));
        }
        let selected = match selection {
            ViewSelection::Exact(reference) => view.identity == reference,
            ViewSelection::Latest { workspace, epoch } => {
                view.identity.workspace == workspace && view.identity.epoch == epoch
            }
        };
        if !selected {
            return Err(NativeUdsError::Protocol("view selection mismatch".into()));
        }
        Ok(view)
    }

    /// Obtain the authenticated CAS stamp for a producer projection delta.
    /// An absent binding is a first-FULL input, never a fabricated empty stamp.
    pub fn read_projection_binding(
        &self,
        selection: ViewSelection,
        projection: ProjectionId,
        budget: &RequestBudgetV1,
    ) -> Result<NativeProjectionRead, NativeUdsError> {
        let NativeIndexResponse::Projection(result) = self.request(
            NativeIndexRequest::ReadProjection {
                selection,
                projection,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected projection binding reply".into(),
            ));
        };
        if !projection_read_matches_request(selection, projection, &result) {
            return Err(NativeUdsError::Protocol(
                "projection binding view or key mismatch".into(),
            ));
        }
        Ok(result)
    }

    /// Resolve prior source-record references from one selected immutable view.
    /// Callers must preserve the returned view and source cut for recovery CAS.
    pub fn lookup_source_owners(
        &self,
        selection: ViewSelection,
        owners: Vec<OwnerKey>,
        budget: &RequestBudgetV1,
    ) -> Result<NativeSourceOwnerLookup, NativeUdsError> {
        let owners = NativeSourceOwnerKeys::new(owners)
            .map_err(|message| NativeUdsError::Protocol(message.into()))?;
        let NativeIndexResponse::SourceOwners(result) = self.request(
            NativeIndexRequest::LookupSourceOwners {
                selection,
                owners: owners.clone(),
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected source-owner lookup reply".into(),
            ));
        };
        if !source_owner_lookup_matches_request(selection, &owners, &result) {
            return Err(NativeUdsError::Protocol(
                "source-owner lookup view or owner binding mismatch".into(),
            ));
        }
        Ok(result)
    }

    /// Resolve exact projection contribution stamps, including absent owners.
    /// The response is bound to the requested view, projection and key order.
    pub fn lookup_projection_owners(
        &self,
        selection: ViewSelection,
        projection: ProjectionId,
        owners: Vec<OwnerKey>,
        budget: &RequestBudgetV1,
    ) -> Result<NativeProjectionOwnerLookup, NativeUdsError> {
        let owners = NativeSourceOwnerKeys::new(owners)
            .map_err(|message| NativeUdsError::Protocol(message.into()))?;
        let NativeIndexResponse::ProjectionOwners(result) = self.request(
            NativeIndexRequest::LookupProjectionOwners {
                selection,
                projection,
                owners: owners.clone(),
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected projection-owner lookup reply".into(),
            ));
        };
        if !projection_owner_lookup_matches_request(selection, projection, &owners, &result) {
            return Err(NativeUdsError::Protocol(
                "projection-owner lookup view or key mismatch".into(),
            ));
        }
        Ok(result)
    }

    /// Page one producer's complete known-owner baseline at a pinned view.
    /// Continue with `Exact(page.view)` and `page.next_after()`; a moving
    /// `Latest` selection is only permitted on the first page.
    pub fn list_producer_source_owners(
        &self,
        selection: ViewSelection,
        producer: ProducerId,
        after: Option<StableOwnerId>,
        limit: u32,
        budget: &RequestBudgetV1,
    ) -> Result<NativeProducerSourcePage, NativeUdsError> {
        if !(1..=128).contains(&limit)
            || (after.is_some() && !matches!(selection, ViewSelection::Exact(_)))
        {
            return Err(NativeUdsError::Protocol(
                "producer source page requires limit 1..=128 and Exact continuation".into(),
            ));
        }
        let NativeIndexResponse::ProducerSourceOwners(page) = self.request(
            NativeIndexRequest::ListProducerSourceOwners {
                selection,
                producer,
                after,
                limit,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected producer source page reply".into(),
            ));
        };
        let rows = page.rows.as_slice();
        let within_limit = usize::try_from(limit).is_ok_and(|bound| rows.len() <= bound);
        let ordered_after = rows.iter().all(|row| {
            row.owner.producer == producer
                && after.is_none_or(|cursor| row.owner.stable_id > cursor)
        });
        if !selection_matches(selection, page.view)
            || page.producer != producer
            || !within_limit
            || !ordered_after
            || (!rows.is_empty() && page.source_epoch.is_none())
            || (page.has_more && rows.is_empty())
        {
            return Err(NativeUdsError::Protocol(
                "producer source page binding or pagination mismatch".into(),
            ));
        }
        Ok(page)
    }

    pub fn search_literal(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        literal: &[u8],
        after: Option<&[u8]>,
        limit: u32,
        budget: &RequestBudgetV1,
    ) -> Result<NativeLiteralPage, NativeUdsError> {
        if literal.is_empty()
            || literal.len() > 4096
            || after.is_some_and(|cursor| cursor.len() != 32)
            || !(1..=128).contains(&limit)
        {
            return Err(NativeUdsError::Protocol(
                "literal query exceeds native request bounds".into(),
            ));
        }
        let page_limit = usize::try_from(limit)
            .map_err(|_conversion_error| NativeUdsError::Protocol("page limit overflow".into()))?;
        let NativeIndexResponse::LiteralPage(page) = self.request(
            NativeIndexRequest::SearchLiteral {
                selection,
                instance,
                literal: literal.to_vec(),
                after: after.map(<[u8]>::to_vec),
                limit,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected literal query reply".into(),
            ));
        };
        if !selection_matches(selection, page.view) || page.hits.len() > page_limit {
            return Err(NativeUdsError::Protocol(
                "literal query page binding or limit mismatch".into(),
            ));
        }
        Ok(page)
    }

    /// Queries one selected native text member. The canonical request IR is
    /// reused; this route has its own bounded result cap.
    pub fn search_text(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextResult, NativeUdsError> {
        if !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&query.top_k) {
            return Err(NativeUdsError::Protocol(
                "text query exceeds native result bounds".into(),
            ));
        }
        let requested_top_k = native_text_result_limit(&query)?;
        let NativeIndexResponse::TextRanked(result) = self.request(
            NativeIndexRequest::SearchText {
                selection,
                instance,
                query,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected text query reply".into(),
            ));
        };
        validate_text_result(selection, instance, requested_top_k, &result)?;
        Ok(result)
    }

    /// Query one atomic unsaved corpus over a pinned durable view.
    pub fn search_overlay_text(
        &self,
        request: NativeOverlayTextQuery,
        budget: &RequestBudgetV1,
    ) -> Result<NativeOverlayTextResult, NativeUdsError> {
        let limit = native_text_result_limit(&request.query)?;
        let (selection, instance, session, expected) = (
            request.selection,
            request.instance,
            request.session,
            request.expected_corpus,
        );
        let NativeIndexResponse::OverlayTextRanked(result) =
            self.request(NativeIndexRequest::SearchOverlayText(request), budget)?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected overlay text reply".into(),
            ));
        };
        validate_overlay_text_result(selection, instance, session, expected, limit, &result)?;
        Ok(result)
    }

    /// Queries one selected native semantic member. Partial coverage and a
    /// capped scan can return fewer than `top_k` hits; the full coverage
    /// binding and scan counters remain visible to the caller.
    pub fn search_semantic(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeSemanticResult, NativeUdsError> {
        if !(1..=MAX_NATIVE_SEMANTIC_TOP_K).contains(&query.top_k)
            || query.query_text.len() > MAX_NATIVE_SEMANTIC_QUERY_BYTES
        {
            return Err(NativeUdsError::Protocol(
                "semantic query exceeds native request bounds".into(),
            ));
        }
        let requested_top_k = query.top_k;
        let NativeIndexResponse::SemanticRanked(result) = self.request(
            NativeIndexRequest::SearchSemantic {
                selection,
                instance,
                query,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected semantic query reply".into(),
            ));
        };
        validate_semantic_result(selection, instance, requested_top_k, &result)?;
        Ok(result)
    }

    pub fn query_paths(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        prefix: &str,
        after: Option<&str>,
        limit: u32,
        budget: &RequestBudgetV1,
    ) -> Result<NativePathPage, NativeUdsError> {
        if prefix.len() > MAX_SOURCE_PATH_BYTES
            || after.is_some_and(|cursor| cursor.len() > MAX_SOURCE_PATH_BYTES)
            || !(1..=128).contains(&limit)
        {
            return Err(NativeUdsError::Protocol(
                "path query exceeds native request bounds".into(),
            ));
        }
        let page_limit = usize::try_from(limit)
            .map_err(|_conversion_error| NativeUdsError::Protocol("page limit overflow".into()))?;
        let NativeIndexResponse::PathPage(page) = self.request(
            NativeIndexRequest::QueryPaths {
                selection,
                instance,
                prefix: prefix.into(),
                after: after.map(str::to_owned),
                limit,
            },
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected path query reply".into(),
            ));
        };
        if !selection_matches(selection, page.view) || page.entries.len() > page_limit {
            return Err(NativeUdsError::Protocol(
                "path query page binding or limit mismatch".into(),
            ));
        }
        Ok(page)
    }
}

fn publication_request_key(request: &NativeIndexRequest) -> Option<OperationKey> {
    match request {
        NativeIndexRequest::BeginInput(key)
        | NativeIndexRequest::Resume(key)
        | NativeIndexRequest::Status(key) => Some(*key),
        NativeIndexRequest::StageSource { ingress, .. }
        | NativeIndexRequest::StageSources { ingress, .. }
        | NativeIndexRequest::StageProfile { ingress, .. }
        | NativeIndexRequest::StageProjectionOutput { ingress, .. }
        | NativeIndexRequest::StageProjectionOwner { ingress, .. }
        | NativeIndexRequest::StageProjectionCertificate { ingress, .. }
        | NativeIndexRequest::StageMutationPage { ingress, .. }
        | NativeIndexRequest::SealMutations { ingress, .. }
        | NativeIndexRequest::DiscardInput(ingress) => Some(ingress.operation),
        NativeIndexRequest::Submit(request) => Some(request.operation),
        NativeIndexRequest::CreateWorkspace { .. }
        | NativeIndexRequest::ReadSession(_)
        | NativeIndexRequest::ReadView(_)
        | NativeIndexRequest::ReadProjection { .. }
        | NativeIndexRequest::LookupProjectionOwners { .. }
        | NativeIndexRequest::DiffProjectionSourceOwners { .. }
        | NativeIndexRequest::Stub(_)
        | NativeIndexRequest::BindOverlaySave(_)
        | NativeIndexRequest::CompleteOverlaySave(_)
        | NativeIndexRequest::Overlay(_)
        | NativeIndexRequest::QueryGraph { .. }
        | NativeIndexRequest::QueryRepoMap { .. }
        | NativeIndexRequest::ReadyWork { .. }
        | NativeIndexRequest::ClaimWork { .. }
        | NativeIndexRequest::RenewWork { .. }
        | NativeIndexRequest::WorkStatus(_)
        | NativeIndexRequest::SearchLiteral { .. }
        | NativeIndexRequest::QueryPaths { .. }
        | NativeIndexRequest::SearchOverlayText(_)
        | NativeIndexRequest::SearchText { .. }
        | NativeIndexRequest::SearchSemantic { .. }
        | NativeIndexRequest::QueryEffortStream { .. }
        | NativeIndexRequest::InteractionBegin { .. }
        | NativeIndexRequest::InteractionActivity { .. }
        | NativeIndexRequest::InteractionRenew { .. }
        | NativeIndexRequest::InteractionEnd { .. }
        | NativeIndexRequest::LookupSourceOwners { .. }
        | NativeIndexRequest::ListProducerSourceOwners { .. } => None,
    }
}

fn publication_response_matches_request(
    request: &NativeIndexRequest,
    response: &NativeIndexResponse,
) -> bool {
    match (request, response) {
        (NativeIndexRequest::BeginInput(key), NativeIndexResponse::InputOpened(ingress)) => {
            ingress.operation == *key
        }
        (
            NativeIndexRequest::StageSource { .. }
            | NativeIndexRequest::StageProfile { .. }
            | NativeIndexRequest::StageProjectionOutput { .. }
            | NativeIndexRequest::StageProjectionOwner { .. }
            | NativeIndexRequest::StageProjectionCertificate { .. },
            NativeIndexResponse::Staged(_),
        )
        | (
            NativeIndexRequest::StageMutationPage { .. },
            NativeIndexResponse::MutationPageStaged(_),
        )
        | (_, NativeIndexResponse::Error(_)) => true,
        (
            NativeIndexRequest::StageSources { inputs, .. },
            NativeIndexResponse::StagedSources(refs),
        ) => refs.len() == inputs.len(),
        (
            NativeIndexRequest::SealMutations { root, count, .. },
            NativeIndexResponse::MutationsStaged(set),
        ) => set.root == *root && set.count == *count,
        (
            NativeIndexRequest::DiscardInput(expected),
            NativeIndexResponse::InputDiscarded(actual),
        ) => actual == expected,
        (NativeIndexRequest::Submit(request), NativeIndexResponse::Submitted(reply)) => {
            validate_submit_reply(request.operation, reply.clone()).is_ok()
        }
        (NativeIndexRequest::Resume(key), NativeIndexResponse::Submitted(reply)) => {
            validate_submit_reply(*key, reply.clone()).is_ok()
        }
        (NativeIndexRequest::Status(key), NativeIndexResponse::Status(reply)) => {
            validate_status_reply(*key, reply.clone()).is_ok()
        }
        (NativeIndexRequest::ReadView(selection), NativeIndexResponse::View(view)) => {
            selection_matches(*selection, view.identity)
        }
        (
            NativeIndexRequest::ReadProjection {
                selection,
                projection,
            },
            NativeIndexResponse::Projection(result),
        ) => projection_read_matches_request(*selection, *projection, result),
        (
            NativeIndexRequest::LookupSourceOwners { selection, owners },
            NativeIndexResponse::SourceOwners(result),
        ) => source_owner_lookup_matches_request(*selection, owners, result),
        (
            NativeIndexRequest::LookupProjectionOwners {
                selection,
                projection,
                owners,
            },
            NativeIndexResponse::ProjectionOwners(result),
        ) => projection_owner_lookup_matches_request(*selection, *projection, owners, result),
        (
            NativeIndexRequest::DiffProjectionSourceOwners {
                selection,
                projection,
                producer,
                after,
                limit,
            },
            NativeIndexResponse::ProjectionSourceDiff(page),
        ) => projection_source_diff_matches_request(
            *selection,
            *projection,
            *producer,
            *after,
            *limit,
            page,
        ),
        _ => false,
    }
}

fn projection_source_diff_matches_request(
    selection: ViewSelection,
    projection: ProjectionId,
    producer: ProducerId,
    after: Option<StableOwnerId>,
    limit: u32,
    page: &quanta_index_contract::native_protocol::NativeProjectionSourceDiff,
) -> bool {
    (1..=128).contains(&limit)
        && (after.is_none() || matches!(selection, ViewSelection::Exact(_)))
        && selection_matches(selection, page.view)
        && page.projection == projection
        && page.producer == producer
        && page.validate().is_ok()
        && usize::try_from(limit).is_ok_and(|limit| page.rows.as_slice().len() <= limit)
        && page
            .rows
            .as_slice()
            .iter()
            .all(|row| after.is_none_or(|after| row.owner.stable_id > after))
}

fn projection_read_matches_request(
    selection: ViewSelection,
    projection: ProjectionId,
    result: &NativeProjectionRead,
) -> bool {
    selection_matches(selection, result.view)
        && result
            .binding
            .is_none_or(|binding| binding.projection == projection)
}

fn source_owner_lookup_matches_request(
    selection: ViewSelection,
    owners: &NativeSourceOwnerKeys,
    result: &NativeSourceOwnerLookup,
) -> bool {
    selection_matches(selection, result.view)
        && result.rows.as_slice().len() == owners.as_slice().len()
        && result
            .rows
            .as_slice()
            .iter()
            .map(|row| row.owner)
            .eq(owners.as_slice().iter().copied())
}

fn projection_owner_lookup_matches_request(
    selection: ViewSelection,
    projection: ProjectionId,
    owners: &NativeSourceOwnerKeys,
    result: &NativeProjectionOwnerLookup,
) -> bool {
    selection_matches(selection, result.view)
        && result.projection == projection
        && result
            .binding
            .is_none_or(|binding| binding.projection == projection)
        && (result.binding.is_some()
            || result
                .rows
                .as_slice()
                .iter()
                .all(|row| row.contribution.is_none()))
        && result.rows.as_slice().len() == owners.as_slice().len()
        && result
            .rows
            .as_slice()
            .iter()
            .map(|row| row.owner)
            .eq(owners.as_slice().iter().copied())
}

fn validate_stage_sources_inputs<T: SourceInputView>(inputs: &[T]) -> Result<(), NativeUdsError> {
    if inputs.is_empty() || inputs.len() > MAX_NATIVE_STAGE_SOURCES_INPUTS {
        return Err(NativeUdsError::Protocol(
            "source batch count exceeds native bounds".into(),
        ));
    }
    let mut bytes = 0usize;
    for input in inputs {
        if input.path().len() > MAX_SOURCE_PATH_BYTES {
            return Err(NativeUdsError::Protocol(
                "source batch path exceeds native bounds".into(),
            ));
        }
        bytes = bytes
            .checked_add(input.path().len())
            .and_then(|total| total.checked_add(input.bytes().len()))
            .and_then(|total| total.checked_add(256))
            .ok_or_else(|| NativeUdsError::Protocol("source batch size overflow".into()))?;
        if bytes > MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES {
            return Err(NativeUdsError::Protocol(
                "source batch size exceeds native bounds".into(),
            ));
        }
    }
    Ok(())
}

fn classify_write_stage_result(
    key: OperationKey,
    budget: &RequestBudgetV1,
    result: Result<NativeIndexResponse, NativeUdsError>,
) -> Result<NativeIndexResponse, NativeUdsError> {
    match result {
        Ok(response) => Ok(response),
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::ResponseTooLarge =>
        {
            Err(NativeUdsError::Unknown(key))
        }
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::Cancelled && budget.is_cancelled() =>
        {
            Err(NativeUdsError::Unknown(key))
        }
        Err(error @ NativeUdsError::Refused(_)) => Err(error),
        Err(
            NativeUdsError::Transport(_) | NativeUdsError::Protocol(_) | NativeUdsError::Unknown(_),
        ) => Err(NativeUdsError::Unknown(key)),
    }
}

fn staged_reference(
    response: &NativeIndexResponse,
    key: OperationKey,
) -> Result<ObjectRef, NativeUdsError> {
    let NativeIndexResponse::Staged(reference) = response else {
        return Err(NativeUdsError::Unknown(key));
    };
    Ok(*reference)
}

fn cancelled_error() -> NativeUdsError {
    NativeUdsError::Refused(NativeIndexWireError {
        code: NativeIndexErrorCode::Cancelled,
        message: "native request cancelled".into(),
    })
}

// A raw byte slice larger than the IPC decoded-request cap cannot fit once
// the CBOR envelope is added. The codec makes the exact final decision.
fn check_stage_bytes_before_copy(
    raw_len: usize,
    decoded_cap: usize,
    budget: &RequestBudgetV1,
) -> Result<(), NativeUdsError> {
    if budget.is_cancelled() {
        return Err(cancelled_error());
    }
    if budget.remaining().is_zero() {
        return Err(NativeUdsError::Transport(
            "native request deadline elapsed".into(),
        ));
    }
    if raw_len > decoded_cap {
        return Err(NativeUdsError::Protocol(
            "native stage raw bytes exceed IPC decoded-request cap".into(),
        ));
    }
    Ok(())
}

fn copy_stage_bytes(bytes: &[u8], budget: &RequestBudgetV1) -> Result<Vec<u8>, NativeUdsError> {
    check_stage_bytes_before_copy(bytes.len(), MAX_MULTIFRAME_REQUEST_BODY_BYTES, budget)?;
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len()).map_err(|error| {
        NativeUdsError::Transport(format!("native stage byte allocation failed: {error}"))
    })?;
    copy.extend_from_slice(bytes);
    Ok(copy)
}

fn stream_budget_checkpoint(budget: &RequestBudgetV1) -> Result<(), NativeUdsError> {
    if budget.is_cancelled() {
        return Err(cancelled_error());
    }
    if budget.remaining().is_zero() {
        return Err(NativeUdsError::Transport(
            "native effort stream deadline elapsed".into(),
        ));
    }
    Ok(())
}

fn classify_stream_error(budget: &RequestBudgetV1, error: &IpcError) -> NativeUdsError {
    if let Err(interruption) = stream_budget_checkpoint(budget) {
        return interruption;
    }
    if matches!(
        error,
        IpcError::Truncated | IpcError::Oversized(_) | IpcError::EmptyFrame | IpcError::Decode(_)
    ) {
        NativeUdsError::Protocol(error.to_string())
    } else {
        NativeUdsError::Transport(error.to_string())
    }
}

// Consume the same canonical Native grammar as the daemon. A count bound is
// request authority; a reply cannot silently lower the cardinality expectation.
fn native_text_result_limit(query: &TextQueryRequest) -> Result<u32, NativeUdsError> {
    if query.syntax != quanta_index_contract::TextQuerySyntax::Native {
        return Ok(query.top_k);
    }
    let parsed = quanta_index_contract::parse_native_content_query(&query.query_text)
        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
    match parsed.options.count {
        None => Ok(query.top_k),
        Some(quanta_index_contract::LqCountBound::Bounded(count)) if count > 0 => {
            Ok(query.top_k.min(count))
        }
        Some(_) => Err(NativeUdsError::Protocol(
            "native text count must be positive and bounded".into(),
        )),
    }
}

fn validate_overlay_corpus_stamp(
    corpus: &NativeOverlayCorpusStamp,
    session: NativeOverlaySession,
    expected: Option<Digest>,
) -> Result<(), NativeUdsError> {
    if corpus.session != session
        || expected.is_some_and(|identity| identity != corpus.identity)
        || corpus.revisions.len() > 256
        || corpus
            .revisions
            .iter()
            .any(|row| row.revision == 0 || row.retired && !row.deleted)
        || corpus
            .revisions
            .iter()
            .zip(corpus.revisions.iter().skip(1))
            .any(|(prior, next)| prior.owner >= next.owner)
    {
        return Err(NativeUdsError::Protocol(
            "overlay corpus session, revision, or identity invalid".into(),
        ));
    }
    Ok(())
}

fn validate_overlay_text_result(
    selection: ViewSelection,
    instance: IndexInstanceId,
    session: NativeOverlaySession,
    expected: Option<Digest>,
    limit: u32,
    result: &NativeOverlayTextResult,
) -> Result<(), NativeUdsError> {
    validate_overlay_corpus_stamp(&result.corpus, session, expected)?;
    if !selection_matches(selection, result.view)
        || result.instance != instance
        || result.view.workspace != session.workspace
        || !result.exhaustive
        || result.hits.len()
            > usize::try_from(limit)
                .map_err(|_error| NativeUdsError::Protocol("overlay limit overflow".into()))?
        || u64::try_from(result.hits.len())
            .map_err(|_error| NativeUdsError::Protocol("overlay hits overflow".into()))?
            != result.matched.min(u64::from(limit))
    {
        return Err(NativeUdsError::Protocol(
            "overlay query binding or cardinality differs".into(),
        ));
    }
    for (position, hit) in result.hits.iter().enumerate() {
        if !valid_native_source_path(&hit.path)
            || result
                .hits
                .iter()
                .take(position)
                .any(|prior| prior.owner == hit.owner)
        {
            return Err(NativeUdsError::Protocol(
                "overlay hit path or owner differs".into(),
            ));
        }
        let revision = result
            .corpus
            .revisions
            .iter()
            .find(|row| row.owner == hit.owner);
        let valid = match hit.source {
            NativeOverlayTextSource::Durable(_) => revision.is_none_or(|row| row.retired),
            NativeOverlayTextSource::Overlay {
                revision: hit_revision,
            } => revision
                .is_some_and(|row| !row.retired && !row.deleted && row.revision == hit_revision),
        };
        if !valid {
            return Err(NativeUdsError::Protocol(
                "overlay hit source origin differs from corpus".into(),
            ));
        }
    }
    for (prior, next) in result.hits.iter().zip(result.hits.iter().skip(1)) {
        let order = prior.score.get().total_cmp(&next.score.get());
        if order.is_lt() || order.is_eq() && prior.owner >= next.owner {
            return Err(NativeUdsError::Protocol(
                "overlay rank order differs".into(),
            ));
        }
    }
    Ok(())
}

fn validate_text_result(
    selection: ViewSelection,
    instance: IndexInstanceId,
    requested_top_k: u32,
    result: &NativeTextResult,
) -> Result<(), NativeUdsError> {
    let limit = usize::try_from(requested_top_k)
        .map_err(|_conversion_error| NativeUdsError::Protocol("text limit overflow".into()))?;
    let hit_count = u64::try_from(result.hits.len())
        .map_err(|_conversion_error| NativeUdsError::Protocol("hit count overflow".into()))?;
    if !selection_matches(selection, result.view)
        || result.instance != instance
        || !result.exhaustive
        || result.hits.len() > limit
        || result.matched < hit_count
        || hit_count != result.matched.min(u64::from(requested_top_k))
    {
        return Err(NativeUdsError::Protocol(
            "text query binding or cardinality mismatch".into(),
        ));
    }
    let mut owners = BTreeSet::new();
    for hit in &result.hits {
        if !valid_native_source_path(&hit.path) || !owners.insert(hit.owner) {
            return Err(NativeUdsError::Protocol(
                "text query hit path or owner invalid".into(),
            ));
        }
    }
    for (prior, next) in result.hits.iter().zip(result.hits.iter().skip(1)) {
        let score_order = prior.score.get().total_cmp(&next.score.get());
        if score_order.is_lt() || (score_order.is_eq() && prior.owner >= next.owner) {
            return Err(NativeUdsError::Protocol(
                "text query hits are not in canonical rank order".into(),
            ));
        }
    }
    Ok(())
}

fn validate_semantic_result(
    selection: ViewSelection,
    instance: IndexInstanceId,
    top_k: u32,
    result: &NativeSemanticResult,
) -> Result<(), NativeUdsError> {
    let limit = usize::try_from(top_k).map_err(|_conversion_error| {
        NativeUdsError::Protocol("semantic result limit overflow".into())
    })?;
    let complete = matches!(result.coverage.status, CoverageStatus::Complete);
    if !selection_matches(selection, result.view)
        || result.instance != instance
        || result.coverage.instance != instance
        || (complete && result.coverage.covered != Some(result.coverage.desired))
        || (result.exhaustive && !complete)
        || result
            .traversal
            .certifies_exhaustive(&result.coverage, result.skipped_stale)
            != result.exhaustive
        || (result.coverage.owners.0.is_some() && result.coverage.searchable.is_none())
        || result.scanned_owners > MAX_NATIVE_SEMANTIC_OWNER_SCAN
        || result.skipped_stale > result.scanned_owners
        || (result.coverage.owners == MapRoot(None)
            && (result.scanned_owners != 0 || result.skipped_stale != 0 || !result.hits.is_empty()))
        || result.hits.len() > limit
    {
        return Err(NativeUdsError::Protocol(
            "semantic query binding, coverage, or scan mismatch".into(),
        ));
    }
    let mut identities = BTreeSet::new();
    let mut hit_owners = BTreeSet::new();
    for hit in &result.hits {
        if !(1..=MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES).contains(&hit.vector_id.len())
            || !(-1.0..=1.0).contains(&hit.score.get())
            || !identities.insert((hit.owner, hit.vector_id.as_slice()))
        {
            return Err(NativeUdsError::Protocol(
                "semantic hit vector identity or score invalid".into(),
            ));
        }
        if let Some(chunk) = hit.chunk {
            chunk
                .validate_binding(&hit.vector_id, result.coverage.desired.policy)
                .map_err(|error| {
                    NativeUdsError::Protocol(format!("semantic hit chunk binding invalid: {error}"))
                })?;
        }
        let _new_owner = hit_owners.insert(hit.owner);
    }
    let fresh_scanned_owners = result
        .scanned_owners
        .checked_sub(result.skipped_stale)
        .ok_or_else(|| NativeUdsError::Protocol("semantic scan counters invalid".into()))?;
    if u64::try_from(hit_owners.len()).map_err(|_conversion_error| {
        NativeUdsError::Protocol("semantic hit owner count overflow".into())
    })? > u64::from(fresh_scanned_owners)
    {
        return Err(NativeUdsError::Protocol(
            "semantic hits exceed fresh scanned owners".into(),
        ));
    }
    for (prior, next) in result.hits.iter().zip(result.hits.iter().skip(1)) {
        let score_order = prior.score.get().total_cmp(&next.score.get());
        if score_order.is_lt()
            || (score_order.is_eq()
                && (prior.owner > next.owner
                    || (prior.owner == next.owner && prior.vector_id >= next.vector_id)))
        {
            return Err(NativeUdsError::Protocol(
                "semantic hits are not in canonical rank order".into(),
            ));
        }
    }
    Ok(())
}

fn validate_overlay_semantic_result(
    selection: ViewSelection,
    instance: IndexInstanceId,
    session: NativeOverlaySession,
    expected: Option<Digest>,
    top_k: u32,
    result: &NativeOverlaySemanticResult,
) -> Result<(), NativeUdsError> {
    validate_overlay_corpus_stamp(&result.corpus, session, expected)?;
    let limit = usize::try_from(top_k).map_err(|_error| {
        NativeUdsError::Protocol("overlay semantic result limit overflow".into())
    })?;
    let complete = matches!(result.coverage.status, CoverageStatus::Complete);
    if !selection_matches(selection, result.view)
        || result.view.workspace != session.workspace
        || result.instance != instance
        || result.coverage.instance != instance
        || (complete && result.coverage.covered != Some(result.coverage.desired))
        || (result.exhaustive && !complete)
        || result
            .traversal
            .certifies_exhaustive(&result.coverage, result.skipped_stale)
            != result.exhaustive
        || (result.coverage.owners.0.is_some() && result.coverage.searchable.is_none())
        || result.scanned_owners > MAX_NATIVE_SEMANTIC_OWNER_SCAN
        || result.skipped_stale > result.scanned_owners
        || result.hits.len() > limit
    {
        return Err(NativeUdsError::Protocol(
            "overlay semantic binding, coverage, or scan invalid".into(),
        ));
    }
    let mut identities = BTreeSet::new();
    let mut hit_owners = BTreeSet::new();
    for hit in &result.hits {
        if !(1..=MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES).contains(&hit.vector_id.len())
            || !(-1.0..=1.0).contains(&hit.score.get())
            || !identities.insert((hit.owner, hit.vector_id.as_slice()))
        {
            return Err(NativeUdsError::Protocol(
                "overlay semantic vector identity or score invalid".into(),
            ));
        }
        let revision = result
            .corpus
            .revisions
            .iter()
            .find(|row| row.owner == hit.owner);
        if let Some(origin) = hit.overlay {
            if hit.chunk.is_some()
                || !revision.is_some_and(|row| {
                    !row.retired && !row.deleted && row.revision == origin.revision
                })
                || origin.chunk.is_some_and(|chunk| {
                    chunk.start > chunk.end
                        || (chunk.start == chunk.end && chunk.start != 0)
                        || chunk.id.0.as_slice() != hit.vector_id.as_slice()
                })
                || (origin.chunk.is_none() && hit.vector_id.as_slice() != b"whole-file")
            {
                return Err(NativeUdsError::Protocol(
                    "overlay semantic unsaved origin invalid".into(),
                ));
            }
        } else {
            if !revision.is_none_or(|row| row.retired) {
                return Err(NativeUdsError::Protocol(
                    "overlay semantic durable origin is masked".into(),
                ));
            }
            if let Some(chunk) = hit.chunk {
                chunk
                    .validate_binding(&hit.vector_id, result.coverage.desired.policy)
                    .map_err(|error| {
                        NativeUdsError::Protocol(format!(
                            "overlay semantic durable chunk binding invalid: {error}"
                        ))
                    })?;
            }
        }
        let _new_owner = hit_owners.insert(hit.owner);
    }
    let fresh_scanned = result
        .scanned_owners
        .checked_sub(result.skipped_stale)
        .ok_or_else(|| NativeUdsError::Protocol("overlay scan counters invalid".into()))?;
    if u64::try_from(hit_owners.len())
        .map_err(|_error| NativeUdsError::Protocol("overlay owner count overflow".into()))?
        > u64::from(fresh_scanned)
    {
        return Err(NativeUdsError::Protocol(
            "overlay semantic hits exceed scanned owners".into(),
        ));
    }
    for (prior, next) in result.hits.iter().zip(result.hits.iter().skip(1)) {
        let order = prior.score.get().total_cmp(&next.score.get());
        if order.is_lt()
            || (order.is_eq()
                && (prior.owner > next.owner
                    || (prior.owner == next.owner && prior.vector_id >= next.vector_id)))
        {
            return Err(NativeUdsError::Protocol(
                "overlay semantic rank order invalid".into(),
            ));
        }
    }
    Ok(())
}

fn selection_matches(
    selection: ViewSelection,
    view: quanta_index_contract::index::ViewRef,
) -> bool {
    match selection {
        ViewSelection::Exact(expected) => expected == view,
        ViewSelection::Latest { workspace, epoch } => {
            view.workspace == workspace && view.epoch == epoch
        }
    }
}

fn valid_native_source_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_SOURCE_PATH_BYTES
        && !path.starts_with('/')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn classify_submit_result(
    response: Result<NativeIndexResponse, NativeUdsError>,
    key: OperationKey,
    budget: &RequestBudgetV1,
) -> Result<SubmitOutcome, NativeUdsError> {
    match response {
        Ok(NativeIndexResponse::Submitted(reply)) => validate_submit_reply(key, reply),
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::ResponseTooLarge =>
        {
            Err(NativeUdsError::Unknown(key))
        }
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::Cancelled && budget.is_cancelled() =>
        {
            Err(NativeUdsError::Unknown(key))
        }
        Err(NativeUdsError::Refused(error)) => Err(NativeUdsError::Refused(error)),
        Ok(_) | Err(_) => Err(NativeUdsError::Unknown(key)),
    }
}

fn validate_submit_reply(
    key: OperationKey,
    reply: OperationReply<SubmitOutcome>,
) -> Result<SubmitOutcome, NativeUdsError> {
    if reply.operation != key {
        return Err(NativeUdsError::Unknown(key));
    }
    let outcome = reply.payload;
    let bound = match outcome {
        SubmitOutcome::Accepted(attempt) => attempt.operation == key,
        SubmitOutcome::Unknown(operation) => operation == key,
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => {
            view.workspace == key.workspace && view.epoch == key.epoch
        }
        SubmitOutcome::Terminal(OperationStatus::Refused(_)) => true,
        SubmitOutcome::Terminal(OperationStatus::Pending(_) | OperationStatus::Unknown) => false,
    };
    if bound {
        Ok(outcome)
    } else {
        Err(NativeUdsError::Unknown(key))
    }
}

fn validate_status_reply(
    key: OperationKey,
    reply: OperationReply<OperationStatus>,
) -> Result<OperationStatus, NativeUdsError> {
    if reply.operation != key {
        return Err(NativeUdsError::Protocol("status operation mismatch".into()));
    }
    let status = reply.payload;
    let bound = match status {
        OperationStatus::Pending(attempt) => attempt.operation == key,
        OperationStatus::Published(view) => {
            view.workspace == key.workspace && view.epoch == key.epoch
        }
        OperationStatus::Unknown | OperationStatus::Refused(_) => true,
    };
    if bound {
        Ok(status)
    } else {
        Err(NativeUdsError::Protocol("status operation mismatch".into()))
    }
}

#[cfg(test)]
mod tests {
    #[path = "headless_tests.rs"]
    mod headless_tests;
    #[path = "projection_tests.rs"]
    mod projection_tests;
    #[path = "repomap_tests.rs"]
    mod repomap_tests;
    #[path = "stub_tests.rs"]
    mod stub_tests;
    use super::*;
    use quanta_index_contract::{
        FiniteScore, QueryConstraintSetV1, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
        index::{
            CollectionId, CoverageBinding, CoverageStatus, Digest, InputBinding, OwnerKey,
            ProducerId, SchemaId, SlotId, SourceCut, SourceEvent, SourceTransition, StableOwnerId,
            StreamId, TenantId, UpdateKind, ViewRef, WorkspaceKey, WorktreeId,
        },
        native_protocol::{
            NativeProducerSourceOwner, NativeProducerSourceRows, NativeSemanticHit,
            NativeSemanticResult, NativeSourceOwnerBinding, NativeSourceOwnerRows,
            NativeTextCoverage, NativeTextHit, NativeTextResult,
        },
    };
    use std::{
        io::Write,
        os::unix::net::UnixListener,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering as AtomicOrdering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    struct PublicationChargeOwner(Arc<AtomicU64>);

    impl quanta_index_ipc::MemoryLeaseOwner for PublicationChargeOwner {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_sub(bytes, AtomicOrdering::SeqCst);
        }
    }

    struct PublicationChargeScope {
        active: Arc<AtomicU64>,
        admissions: Arc<AtomicU64>,
        deny: bool,
    }

    impl quanta_index_ipc::MemoryScopePort for PublicationChargeScope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, quanta_index_ipc::IndexError> {
            let _previous = self.admissions.fetch_add(1, AtomicOrdering::SeqCst);
            if self.deny {
                return Err(quanta_index_ipc::IndexError::ResourceLimited);
            }
            let _previous = self.active.fetch_add(bytes, AtomicOrdering::SeqCst);
            Ok(MemoryLease::new(
                bytes,
                Arc::new(PublicationChargeOwner(Arc::clone(&self.active))),
            ))
        }
    }

    fn publication_scope(deny: bool) -> (MemoryScope, Arc<AtomicU64>, Arc<AtomicU64>) {
        let active = Arc::new(AtomicU64::new(0));
        let admissions = Arc::new(AtomicU64::new(0));
        (
            MemoryScope::new(Arc::new(PublicationChargeScope {
                active: Arc::clone(&active),
                admissions: Arc::clone(&admissions),
                deny,
            })),
            active,
            admissions,
        )
    }

    #[test]
    fn request_id_allocator_uses_last_nonwrapping_id_once() {
        const LAST_NONWRAPPING_ID: u64 = 18_446_744_073_709_551_614;
        let counter = AtomicU64::new(LAST_NONWRAPPING_ID);
        let first = allocate_request_id(&counter);
        assert!(
            matches!(first, Ok(value) if value == LAST_NONWRAPPING_ID),
            "last nonwrapping request identity differs: {first:?}"
        );
        assert_eq!(counter.load(Ordering::Acquire), u64::MAX);
        for _attempt in 0..2 {
            assert!(matches!(
                allocate_request_id(&counter),
                Err(NativeUdsError::Protocol(message)) if message == "request id exhausted"
            ));
            assert_eq!(counter.load(Ordering::Acquire), u64::MAX);
        }
    }

    fn effort_query() -> NativeEffortStreamQuery {
        let fast = text_result();
        NativeEffortStreamQuery {
            overlay: None,
            selection: ViewSelection::Exact(fast.view),
            text_instance: fast.instance,
            semantic_instance: semantic_result().instance,
            text_query: text_query(1),
            semantic_query: semantic_query(1),
            preference: NativeEffortPreference::Deep,
        }
    }

    fn overlay_effort_fixtures() -> (
        NativeEffortStreamQuery,
        NativeOverlayTextResult,
        NativeOverlaySemanticResult,
        NativeEffortDone,
    ) {
        use quanta_index_contract::native_protocol::{
            NativeOverlayRevision, NativeOverlaySemanticHit, NativeOverlaySemanticHitOrigin,
            NativeOverlayTextHit,
        };

        let text = text_result();
        let semantic = semantic_result();
        let owner = text.hits.first().expect("fixed text hit").owner;
        let session = NativeOverlaySession {
            workspace: text.view.workspace,
            publisher: NativeInteractionPublisherId([35; 32]),
            epoch: 1,
        };
        let corpus = NativeOverlayCorpusStamp {
            session,
            identity: Digest([50; 32]),
            revisions: vec![NativeOverlayRevision {
                owner,
                revision: 1,
                deleted: false,
                retired: false,
            }],
        };
        let fast = NativeOverlayTextResult {
            view: text.view,
            instance: text.instance,
            artifact: text.artifact,
            matched: 1,
            exhaustive: true,
            corpus: corpus.clone(),
            hits: vec![NativeOverlayTextHit {
                owner,
                source: NativeOverlayTextSource::Overlay { revision: 1 },
                path: "src/lib.rs".into(),
                score: FiniteScore::new(1.25).expect("finite score"),
            }],
        };
        let deep = NativeOverlaySemanticResult {
            view: semantic.view,
            instance: semantic.instance,
            coverage: semantic.coverage,
            traversal: semantic.traversal,
            exhaustive: semantic.exhaustive,
            scanned_owners: semantic.scanned_owners,
            skipped_stale: semantic.skipped_stale,
            corpus: corpus.clone(),
            hits: vec![NativeOverlaySemanticHit {
                owner,
                vector_id: b"whole-file".to_vec(),
                score: FiniteScore::new(0.75).expect("finite score"),
                chunk: None,
                overlay: Some(NativeOverlaySemanticHitOrigin {
                    revision: 1,
                    chunk: None,
                }),
            }],
        };
        let done = NativeEffortDone {
            view: text.view,
            reason: quanta_index_contract::native_protocol::NativeEffortReason::ExplicitDeep,
            deep_delivered: true,
            corpus: Some(corpus.clone()),
        };
        let mut query = effort_query();
        query.overlay = Some(NativeOverlayEffortContext {
            session,
            expected_corpus: Some(corpus.identity),
        });
        (query, fast, deep, done)
    }

    #[test]
    fn overlay_effort_stream_binds_fast_deep_and_terminal_to_one_corpus() {
        let (query, fast, deep, done) = overlay_effort_fixtures();
        let expected = fast.corpus.clone();
        let (outcome, connected) = scripted_effort_stream(
            vec![
                NativeIndexResponse::OverlayTextRanked(fast),
                NativeIndexResponse::OverlaySemanticRanked(deep),
                NativeIndexResponse::EffortDone(done.clone()),
            ],
            None,
            |client, budget| {
                let mut stages = Vec::new();
                let result = client.query_effort_stream(query, budget, &mut |stage| {
                    match stage {
                        NativeEffortStreamStage::OverlayFast(result) => {
                            assert_eq!(result.corpus, expected);
                            stages.push(1_u8);
                        }
                        NativeEffortStreamStage::OverlayDeep(result) => {
                            assert_eq!(result.corpus, expected);
                            stages.push(2_u8);
                        }
                        NativeEffortStreamStage::Fast(_) | NativeEffortStreamStage::Deep(_) => {
                            return Err(NativeUdsError::Protocol(
                                "overlay effort delivered durable stage".into(),
                            ));
                        }
                    }
                    Ok(())
                });
                (result, stages)
            },
        );
        assert!(connected);
        assert_eq!(outcome.0.expect("overlay effort terminal"), done);
        assert_eq!(outcome.1, [1, 2]);
    }

    #[test]
    fn overlay_effort_stream_rejects_cross_stage_and_terminal_forgery() {
        let (query, fast, deep, done) = overlay_effort_fixtures();
        let mut changed_deep = deep.clone();
        changed_deep
            .corpus
            .revisions
            .first_mut()
            .expect("fixed row")
            .revision = 2;
        let mut changed_origin = deep.clone();
        changed_origin.hits.first_mut().expect("fixed hit").overlay = Some(
            quanta_index_contract::native_protocol::NativeOverlaySemanticHitOrigin {
                revision: 2,
                chunk: None,
            },
        );
        let mut changed_done = done;
        changed_done.corpus.as_mut().expect("fixed corpus").identity = Digest([51; 32]);
        let cases = [
            (
                vec![
                    NativeIndexResponse::OverlayTextRanked(fast.clone()),
                    NativeIndexResponse::OverlaySemanticRanked(changed_deep),
                ],
                1_u8,
            ),
            (
                vec![
                    NativeIndexResponse::OverlayTextRanked(fast.clone()),
                    NativeIndexResponse::OverlaySemanticRanked(changed_origin),
                ],
                1_u8,
            ),
            (
                vec![
                    NativeIndexResponse::OverlayTextRanked(fast),
                    NativeIndexResponse::OverlaySemanticRanked(deep),
                    NativeIndexResponse::EffortDone(changed_done),
                ],
                2_u8,
            ),
            (vec![NativeIndexResponse::TextRanked(text_result())], 0_u8),
        ];
        for (frames, expected_stages) in cases {
            let (outcome, connected) = scripted_effort_stream(frames, None, |client, budget| {
                let mut stages = 0_u8;
                let result = client.query_effort_stream(
                    NativeEffortStreamQuery {
                        overlay: query.overlay,
                        selection: query.selection,
                        text_instance: query.text_instance,
                        semantic_instance: query.semantic_instance,
                        text_query: text_query(1),
                        semantic_query: semantic_query(1),
                        preference: query.preference,
                    },
                    budget,
                    &mut |_stage| {
                        stages = stages.saturating_add(1);
                        Ok(())
                    },
                );
                (result, stages)
            });
            assert!(connected);
            assert!(matches!(outcome.0, Err(NativeUdsError::Protocol(_))));
            assert_eq!(outcome.1, expected_stages);
        }
    }

    #[test]
    fn overlay_effort_chunk_coordinate_and_whole_file_identity_match_runtime() {
        use quanta_index_contract::native_protocol::NativeOverlaySemanticChunk;

        let (_, _, base, _) = overlay_effort_fixtures();
        let mut empty_zero = base.clone();
        let hit = empty_zero.hits.first_mut().expect("fixed overlay hit");
        hit.vector_id = vec![42; 32];
        hit.overlay.as_mut().expect("fixed overlay origin").chunk =
            Some(NativeOverlaySemanticChunk {
                id: Digest([42; 32]),
                input_digest: Digest([
                    0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99,
                    0x6f, 0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95,
                    0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
                ]),
                start: 0,
                end: 0,
            });
        let mut empty_nonzero = empty_zero.clone();
        empty_nonzero
            .hits
            .first_mut()
            .and_then(|hit| hit.overlay.as_mut())
            .and_then(|origin| origin.chunk.as_mut())
            .expect("fixed chunk")
            .start = 1;
        empty_nonzero
            .hits
            .first_mut()
            .and_then(|hit| hit.overlay.as_mut())
            .and_then(|origin| origin.chunk.as_mut())
            .expect("fixed chunk")
            .end = 1;
        let mut reversed = empty_zero.clone();
        reversed
            .hits
            .first_mut()
            .and_then(|hit| hit.overlay.as_mut())
            .and_then(|origin| origin.chunk.as_mut())
            .expect("fixed chunk")
            .start = 2;
        reversed
            .hits
            .first_mut()
            .and_then(|hit| hit.overlay.as_mut())
            .and_then(|origin| origin.chunk.as_mut())
            .expect("fixed chunk")
            .end = 1;
        let mut foreign_whole_file = base;
        foreign_whole_file
            .hits
            .first_mut()
            .expect("fixed overlay hit")
            .vector_id = b"foreign".to_vec();

        for (deep, accepted) in [
            (empty_zero, true),
            (empty_nonzero, false),
            (reversed, false),
            (foreign_whole_file, false),
        ] {
            let (query, fast, _, done) = overlay_effort_fixtures();
            let mut frames = vec![
                NativeIndexResponse::OverlayTextRanked(fast),
                NativeIndexResponse::OverlaySemanticRanked(deep),
            ];
            if accepted {
                frames.push(NativeIndexResponse::EffortDone(done.clone()));
            }
            let (outcome, connected) = scripted_effort_stream(frames, None, |client, budget| {
                let mut seen_deep = false;
                let result = client.query_effort_stream(query, budget, &mut |stage| {
                    if matches!(stage, NativeEffortStreamStage::OverlayDeep(_)) {
                        seen_deep = true;
                    }
                    Ok(())
                });
                (result, seen_deep)
            });
            assert!(connected);
            assert_eq!(outcome.1, accepted);
            if accepted {
                assert_eq!(outcome.0.expect("valid empty UTF-8 span"), done);
            } else {
                assert!(matches!(outcome.0, Err(NativeUdsError::Protocol(_))));
            }
        }
    }

    fn scripted_effort_stream<T>(
        frames: Vec<NativeIndexResponse>,
        after_fast: Option<mpsc::Receiver<()>>,
        call: impl FnOnce(&NativeUdsClient, &RequestBudgetV1) -> T,
    ) -> (T, bool) {
        scripted_effort_stream_with_budget(frames, after_fast, Duration::from_secs(4), call)
    }

    fn scripted_effort_stream_with_budget<T>(
        frames: Vec<NativeIndexResponse>,
        after_fast: Option<mpsc::Receiver<()>>,
        budget_duration: Duration,
        call: impl FnOnce(&NativeUdsClient, &RequestBudgetV1) -> T,
    ) -> (T, bool) {
        let directory = tempfile::tempdir().expect("socket directory");
        let socket = directory.path().join("native.sock");
        let listener = UnixListener::bind(&socket).expect("bind scripted native socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let peer = std::thread::spawn(move || {
            let started = Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && started.elapsed() < Duration::from_secs(3) =>
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return false,
                }
            };
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("native effort request");
            assert!(matches!(
                request.payload,
                NativeIndexRequest::QueryEffortStream { .. }
            ));
            for (index, payload) in frames.into_iter().enumerate() {
                let reply = NativeIndexResponseEnvelope {
                    request_id: request.request_id,
                    payload,
                };
                stream
                    .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode frame"))
                    .expect("send effort frame");
                if index == 0
                    && let Some(receiver) = &after_fast
                {
                    receiver
                        .recv_timeout(Duration::from_secs(2))
                        .expect("Fast callback must run before Deep frame");
                }
            }
            true
        });
        let client = NativeUdsClient::new(socket);
        let budget = RequestBudgetV1::for_duration(budget_duration);
        let result = call(&client, &budget);
        (result, peer.join().expect("scripted peer"))
    }

    fn held_after_fast(
        cancel_after_fast: bool,
    ) -> (Result<NativeEffortDone, NativeUdsError>, bool) {
        let directory = tempfile::tempdir().expect("socket directory");
        let socket = directory.path().join("native.sock");
        let listener = UnixListener::bind(&socket).expect("bind held native socket");
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept effort client");
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("effort request");
            assert!(matches!(
                request.payload,
                NativeIndexRequest::QueryEffortStream { .. }
            ));
            let frame = quanta_index_ipc::encode_response(&NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: NativeIndexResponse::TextRanked(text_result()),
            })
            .expect("encode Fast frame");
            stream.write_all(&frame).expect("send Fast frame");
            // Enter the next frame header but never complete it. A client
            // must exit on its budget while the peer keeps the socket open.
            stream
                .write_all(&[4_u8])
                .expect("send partial terminal header");
            let _released = release_rx.recv_timeout(Duration::from_secs(4));
        });
        let budget = RequestBudgetV1::for_duration(if cancel_after_fast {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(1)
        });
        let client_budget = budget.clone();
        let (fast_tx, fast_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let client = NativeUdsClient::new(socket);
        let worker = std::thread::spawn(move || {
            let mut saw_fast = false;
            let result = client.query_effort_stream(effort_query(), &client_budget, &mut |stage| {
                if matches!(stage, NativeEffortStreamStage::Fast(_)) {
                    saw_fast = true;
                    fast_tx.send(()).expect("signal delivered Fast");
                }
                Ok(())
            });
            result_tx
                .send((result, saw_fast))
                .expect("return held outcome");
        });
        fast_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("Fast must be delivered while peer is live");
        if cancel_after_fast {
            // The peer retains an incomplete terminal frame. The IPC crate's
            // read-loop regression proves the exact in-decode cancellation.
            budget.cancel_handle().cancel();
        }
        let (result, saw_fast) = result_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("stream must stop before the held peer closes");
        drop(release_tx);
        peer.join().expect("held peer");
        worker.join().expect("held stream client");
        (result, saw_fast)
    }

    #[test]
    fn effort_stream_delivers_fast_before_deep_and_terminal() {
        let fast = text_result();
        let deep = semantic_result();
        let done = NativeEffortDone {
            corpus: None,
            view: fast.view,
            reason: quanta_index_contract::native_protocol::NativeEffortReason::ExplicitDeep,
            deep_delivered: true,
        };
        let (fast_seen_tx, fast_seen_rx) = mpsc::channel();
        let (result, connected) = scripted_effort_stream(
            vec![
                NativeIndexResponse::TextRanked(fast.clone()),
                NativeIndexResponse::SemanticRanked(deep),
                NativeIndexResponse::EffortDone(done.clone()),
            ],
            Some(fast_seen_rx),
            |client, budget| {
                let mut seen = Vec::new();
                let result = client.query_effort_stream(effort_query(), budget, &mut |stage| {
                    match stage {
                        NativeEffortStreamStage::Fast(result) => {
                            seen.push((1_u8, result.view));
                            fast_seen_tx.send(()).expect("release scripted Deep");
                        }
                        NativeEffortStreamStage::Deep(result) => seen.push((2_u8, result.view)),
                        NativeEffortStreamStage::OverlayFast(_)
                        | NativeEffortStreamStage::OverlayDeep(_) => {
                            return Err(NativeUdsError::Protocol(
                                "durable effort received an overlay stage".into(),
                            ));
                        }
                    }
                    Ok(())
                });
                (result, seen)
            },
        );
        assert!(connected);
        assert_eq!(result.0.expect("terminal frame"), done);
        assert_eq!(result.1, vec![(1_u8, fast.view), (2_u8, fast.view)]);
    }

    #[test]
    fn effort_stream_truncated_after_fast_is_never_a_final_success() {
        let fast = text_result();
        let (result, connected) = scripted_effort_stream(
            vec![NativeIndexResponse::TextRanked(fast)],
            None,
            |client, budget| {
                let mut fast_seen = false;
                let result = client.query_effort_stream(effort_query(), budget, &mut |stage| {
                    fast_seen = matches!(stage, NativeEffortStreamStage::Fast(_));
                    Ok(())
                });
                (result, fast_seen)
            },
        );
        assert!(connected);
        assert!(result.1);
        assert!(matches!(result.0, Err(NativeUdsError::Protocol(_))));
    }

    #[test]
    fn effort_stream_deep_callback_cannot_outlive_original_deadline() {
        let fast = text_result();
        let deep = semantic_result();
        let done = NativeEffortDone {
            corpus: None,
            view: fast.view,
            reason: quanta_index_contract::native_protocol::NativeEffortReason::ExplicitDeep,
            deep_delivered: true,
        };
        let (result, connected) = scripted_effort_stream_with_budget(
            vec![
                NativeIndexResponse::TextRanked(fast),
                NativeIndexResponse::SemanticRanked(deep),
                NativeIndexResponse::EffortDone(done),
            ],
            None,
            Duration::from_secs(1),
            |client, budget| {
                let mut saw_deep = false;
                let result = client.query_effort_stream(effort_query(), budget, &mut |stage| {
                    if matches!(stage, NativeEffortStreamStage::Deep(_)) {
                        saw_deep = true;
                        std::thread::sleep(budget.remaining());
                    }
                    Ok(())
                });
                (result, saw_deep)
            },
        );
        assert!(connected);
        assert!(result.1);
        assert!(matches!(result.0, Err(NativeUdsError::Transport(_))));
    }

    #[test]
    fn effort_stream_deadline_ends_a_live_peer_without_terminal() {
        let (result, saw_fast) = held_after_fast(false);
        assert!(saw_fast);
        assert!(matches!(result, Err(NativeUdsError::Transport(_))));
    }

    #[test]
    fn effort_stream_external_cancel_ends_a_live_peer_without_terminal() {
        let (result, saw_fast) = held_after_fast(true);
        assert!(saw_fast);
        assert!(matches!(
            result,
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            }))
        ));
    }

    #[test]
    fn one_shot_request_rejects_stream_before_connecting() {
        let directory = tempfile::tempdir().expect("socket directory");
        let missing_socket = directory.path().join("absent.sock");
        let client = NativeUdsClient::new(missing_socket);
        let query = effort_query();
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let result = client.request(
            NativeIndexRequest::QueryEffortStream {
                overlay: query.overlay,
                selection: query.selection,
                text_instance: query.text_instance,
                semantic_instance: query.semantic_instance,
                text_query: query.text_query,
                semantic_query: query.semantic_query,
                preference: query.preference,
            },
            &budget,
        );
        assert!(matches!(result, Err(NativeUdsError::Protocol(_))));
    }

    #[test]
    fn effort_stream_rejects_mismatched_terminal_and_stage_order() {
        let fast = text_result();
        let deep = semantic_result();
        let cases = [
            vec![
                NativeIndexResponse::TextRanked(fast.clone()),
                NativeIndexResponse::EffortDone(NativeEffortDone {
                    corpus: None,
                    view: fast.view,
                    reason:
                        quanta_index_contract::native_protocol::NativeEffortReason::ExplicitDeep,
                    deep_delivered: true,
                }),
            ],
            vec![NativeIndexResponse::SemanticRanked(deep)],
            vec![
                NativeIndexResponse::TextRanked(fast.clone()),
                NativeIndexResponse::EffortDone(NativeEffortDone {
                    corpus: None,
                    view: ViewRef {
                        sequence: fast.view.sequence + 1,
                        ..fast.view
                    },
                    reason:
                        quanta_index_contract::native_protocol::NativeEffortReason::ExplicitFast,
                    deep_delivered: false,
                }),
            ],
        ];
        for frames in cases {
            let (result, connected) = scripted_effort_stream(frames, None, |client, budget| {
                client.query_effort_stream(effort_query(), budget, &mut |_stage| Ok(()))
            });
            assert!(connected);
            assert!(matches!(result, Err(NativeUdsError::Protocol(_))));
        }
    }

    fn scripted_reply<T>(
        response: NativeIndexResponse,
        call: impl FnOnce(&NativeUdsClient, &RequestBudgetV1) -> T,
    ) -> T {
        let directory = tempfile::tempdir().expect("socket directory");
        let socket = directory.path().join("native.sock");
        let listener = UnixListener::bind(&socket).expect("bind scripted native socket");
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("native client connection");
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("native request");
            let reply = NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: response,
            };
            stream
                .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode reply"))
                .expect("send native reply");
        });
        let client = NativeUdsClient::new(socket);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let result = call(&client, &budget);
        peer.join().expect("scripted native peer");
        result
    }

    #[test]
    fn stage_source_preserves_explicit_text_admission_on_wire() {
        let ingress = IngressRef {
            operation: key(1),
            fence: 7,
        };
        let reference = ObjectRef(Digest([9; 32]));
        for text in [
            SourceTextAdmission::Excluded,
            SourceTextAdmission::Utf8 {
                policy: Digest([3; 32]),
            },
        ] {
            let directory = tempfile::tempdir().expect("socket directory");
            let socket = directory.path().join("native.sock");
            let listener = UnixListener::bind(&socket).expect("bind scripted native socket");
            let peer = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("native client connection");
                let request: NativeIndexRequestEnvelope =
                    quanta_index_ipc::decode_request(&mut stream).expect("native request");
                let NativeIndexRequest::StageSource {
                    ingress: wire_ingress,
                    path,
                    bytes,
                    text: wire_text,
                } = request.payload
                else {
                    panic!("expected a single-source stage request");
                };
                assert_eq!(wire_ingress, ingress);
                assert_eq!(path, "sample.rs");
                assert_eq!(bytes, b"source body");
                assert_eq!(wire_text, text);
                let reply = NativeIndexResponseEnvelope {
                    request_id: request.request_id,
                    payload: NativeIndexResponse::Staged(reference),
                };
                stream
                    .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode reply"))
                    .expect("send native reply");
            });
            let client = NativeUdsClient::new(socket);
            let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
            let staged = client
                .stage_source(ingress, "sample.rs".into(), b"source body", text, &budget)
                .expect("source stage reply");
            peer.join().expect("scripted native peer");
            assert_eq!(staged, reference);
        }
    }

    #[test]
    fn producer_source_page_sdk_rejects_mismatches_and_preflights_before_connect()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = ViewRef {
            workspace: key(1).workspace,
            epoch: 1,
            sequence: 4,
            digest: Digest([4; 32]),
        };
        let producer = ProducerId([5; 16]);
        let owner = |id| OwnerKey {
            producer,
            stable_id: StableOwnerId([id; 16]),
        };
        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let rows = |ids: &[u8]| -> Result<NativeProducerSourceRows, &'static str> {
            NativeProducerSourceRows::new(
                ids.iter()
                    .map(|id| NativeProducerSourceOwner {
                        owner: owner(*id),
                        source: ObjectRef(Digest([*id; 32])),
                    })
                    .collect(),
            )
        };
        let valid = NativeProducerSourcePage {
            view,
            source,
            producer,
            source_epoch: Some(1),
            rows: rows(&[2])?,
            has_more: true,
        };
        let returned = scripted_reply(
            NativeIndexResponse::ProducerSourceOwners(valid.clone()),
            |client, budget| {
                client.list_producer_source_owners(
                    ViewSelection::Exact(view),
                    producer,
                    Some(StableOwnerId([1; 16])),
                    1,
                    budget,
                )
            },
        )?;
        if returned != valid {
            return Err("valid producer source page was rejected or changed".into());
        }

        for invalid in [
            NativeProducerSourcePage {
                view: ViewRef {
                    sequence: 5,
                    ..view
                },
                ..valid.clone()
            },
            NativeProducerSourcePage {
                producer: ProducerId([6; 16]),
                rows: NativeProducerSourceRows::new(vec![NativeProducerSourceOwner {
                    owner: OwnerKey {
                        producer: ProducerId([6; 16]),
                        stable_id: StableOwnerId([2; 16]),
                    },
                    source: ObjectRef(Digest([2; 32])),
                }])?,
                ..valid.clone()
            },
            NativeProducerSourcePage {
                rows: rows(&[1])?,
                ..valid.clone()
            },
            NativeProducerSourcePage {
                rows: rows(&[2, 3])?,
                ..valid
            },
        ] {
            let result = scripted_reply(
                NativeIndexResponse::ProducerSourceOwners(invalid),
                |client, budget| {
                    client.list_producer_source_owners(
                        ViewSelection::Exact(view),
                        producer,
                        Some(StableOwnerId([1; 16])),
                        1,
                        budget,
                    )
                },
            );
            if !matches!(result, Err(NativeUdsError::Protocol(_))) {
                return Err("mismatched producer source page accepted".into());
            }
        }
        let empty = NativeProducerSourcePage {
            view,
            source,
            producer: ProducerId([9; 16]),
            source_epoch: None,
            rows: rows(&[])?,
            has_more: false,
        };
        let returned = scripted_reply(
            NativeIndexResponse::ProducerSourceOwners(empty.clone()),
            |client, budget| {
                client.list_producer_source_owners(
                    ViewSelection::Exact(view),
                    ProducerId([9; 16]),
                    None,
                    64,
                    budget,
                )
            },
        )?;
        if returned != empty {
            return Err("unknown producer empty page was rejected".into());
        }

        let disconnected = NativeUdsClient::new(PathBuf::from("/nonexistent/producer-source-page"));
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        for (selection, after, limit) in [
            (ViewSelection::Exact(view), None, 0),
            (ViewSelection::Exact(view), None, 129),
            (
                ViewSelection::Latest {
                    workspace: view.workspace,
                    epoch: view.epoch,
                },
                Some(StableOwnerId([1; 16])),
                1,
            ),
        ] {
            if !matches!(
                disconnected
                    .list_producer_source_owners(selection, producer, after, limit, &budget),
                Err(NativeUdsError::Protocol(_))
            ) {
                return Err("producer source preflight connected or accepted invalid input".into());
            }
        }
        Ok(())
    }

    #[test]
    fn interaction_sdk_binds_window_session_and_update_ack() {
        let publisher = NativeInteractionPublisherId([7; 32]);
        let wrong = NativeInteractionPublisherId([8; 32]);
        let missing_socket = NativeUdsClient::new(PathBuf::from("/nonexistent/native-interaction"));
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        for window in [
            Duration::ZERO,
            Duration::from_nanos(1),
            Duration::from_secs(61),
        ] {
            assert!(matches!(
                missing_socket.begin_interaction_publisher(window, &budget),
                Err(NativeUdsError::Protocol(_))
            ));
        }
        assert!(matches!(
            missing_socket.report_interaction_activity(publisher, 0, &budget),
            Err(NativeUdsError::Protocol(_))
        ));
        assert!(matches!(
            missing_socket.renew_interaction_publisher(
                NativeInteractionPublisherId([0; 32]),
                1,
                &budget
            ),
            Err(NativeUdsError::Protocol(_))
        ));
        assert_eq!(
            scripted_reply(
                NativeIndexResponse::InteractionBegun(publisher),
                |client, budget| {
                    client.begin_interaction_publisher(Duration::from_millis(1_000), budget)
                }
            )
            .expect("interaction begin ACK"),
            publisher
        );
        assert!(matches!(
            scripted_reply(
                NativeIndexResponse::InteractionBegun(NativeInteractionPublisherId([0; 32])),
                |client, budget| {
                    client.begin_interaction_publisher(Duration::from_millis(1_000), budget)
                }
            ),
            Err(NativeUdsError::Protocol(_))
        ));
        assert!(
            scripted_reply(
                NativeIndexResponse::InteractionAccepted(publisher),
                |client, budget| client.report_interaction_activity(publisher, 1, budget)
            )
            .is_ok()
        );
        assert!(
            scripted_reply(
                NativeIndexResponse::InteractionAccepted(publisher),
                |client, budget| client.renew_interaction_publisher(publisher, 2, budget)
            )
            .is_ok()
        );
        assert!(
            scripted_reply(
                NativeIndexResponse::InteractionAccepted(publisher),
                |client, budget| client.end_interaction_publisher(publisher, 3, budget)
            )
            .is_ok()
        );
        assert!(matches!(
            scripted_reply(
                NativeIndexResponse::InteractionAccepted(wrong),
                |client, budget| { client.report_interaction_activity(publisher, 4, budget) }
            ),
            Err(NativeUdsError::Protocol(_))
        ));
    }

    fn key(sequence: u64) -> OperationKey {
        OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence,
        }
    }

    #[test]
    fn admitted_publication_reply_retains_original_scope_until_drop() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket = directory.path().join("native-admitted.sock");
        let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
        let operation = key(74);
        let ingress = IngressRef {
            operation,
            fence: 19,
        };
        let peer = std::thread::spawn(move || -> Result<usize, String> {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).map_err(|error| error.to_string())?;
            if request.payload != NativeIndexRequest::BeginInput(operation) {
                return Err("admitted client changed the BeginInput request".into());
            }
            let frame = quanta_index_ipc::encode_response(&NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: NativeIndexResponse::InputOpened(ingress),
            })
            .map_err(|error| error.to_string())?;
            let body_bytes = frame
                .len()
                .checked_sub(4)
                .ok_or_else(|| "response frame lacks header".to_owned())?;
            stream
                .write_all(&frame)
                .map_err(|error| error.to_string())?;
            Ok(body_bytes)
        });
        let (memory, active, admissions) = publication_scope(false);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let client = NativeUdsClient::new(socket);
        let reply = client
            .request_publication_admitted(
                NativeIndexRequest::BeginInput(operation),
                &budget,
                &memory,
            )
            .map_err(|error| error.to_string());
        let body_bytes = match peer.join() {
            Ok(result) => result?,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        let reply = reply?;
        if reply.response() != &NativeIndexResponse::InputOpened(ingress) {
            return Err("admitted BeginInput reply differs".into());
        }
        let live_bytes = u64::try_from(body_bytes)
            .map_err(|error| error.to_string())?
            .checked_add(
                admitted_reply_v1::decoded_retained_limit().map_err(|error| error.to_string())?,
            )
            .ok_or_else(|| "admitted response charge overflow".to_owned())?;
        if active.load(AtomicOrdering::SeqCst) != live_bytes
            || admissions.load(AtomicOrdering::SeqCst) < 3
        {
            return Err("raw or decoded reply charge did not retain original scope".into());
        }
        drop(reply);
        if active.load(AtomicOrdering::SeqCst) != 0 {
            return Err("admitted response charge survived owner drop".into());
        }
        Ok(())
    }

    #[test]
    fn admitted_publication_scope_refusal_precedes_socket_connect() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket = directory.path().join("native-denied.sock");
        let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let (memory, active, admissions) = publication_scope(true);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let operation = key(75);
        let client = NativeUdsClient::new(socket);
        let result = client.request_publication_admitted(
            NativeIndexRequest::BeginInput(operation),
            &budget,
            &memory,
        );
        if !matches!(result, Err(NativeUdsError::Unknown(key)) if key == operation)
            || admissions.load(AtomicOrdering::SeqCst) == 0
            || active.load(AtomicOrdering::SeqCst) != 0
            || !matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        {
            return Err("denied original scope contacted the Native UDS peer".into());
        }
        Ok(())
    }

    #[test]
    fn borrowed_mutation_page_ack_loss_keeps_original_operation() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket = directory.path().join("native-mutation-ack-loss.sock");
        let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
        let operation = key(76);
        let ingress = IngressRef {
            operation,
            fence: 23,
        };
        let base = MapRoot(None);
        let mutation = OwnerMutation {
            owner: OwnerKey {
                producer: operation.producer,
                stable_id: StableOwnerId([7; 16]),
            },
            expected: None,
            next: Some(ObjectRef(Digest([8; 32]))),
        };
        let expected_mutation = mutation.clone();
        let peer = std::thread::spawn(move || -> Result<(), String> {
            let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).map_err(|error| error.to_string())?;
            if request.payload
                != (NativeIndexRequest::StageMutationPage {
                    ingress,
                    base,
                    mutations: vec![expected_mutation],
                })
            {
                return Err("borrowed mutation page changed the canonical request".into());
            }
            Ok(())
        });
        let (memory, active, _admissions) = publication_scope(false);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let client = NativeUdsClient::new(socket);
        let result = client.stage_mutation_page_borrowed_admitted(
            ingress,
            base,
            &[mutation],
            &budget,
            &memory,
        );
        match peer.join() {
            Ok(result) => result?,
            Err(payload) => std::panic::resume_unwind(payload),
        }
        if !matches!(result, Err(NativeUdsError::Unknown(key)) if key == operation)
            || active.load(AtomicOrdering::SeqCst) != 0
        {
            return Err("mutation ACK loss lost original operation or memory charge".into());
        }
        Ok(())
    }

    fn source_submit_request() -> UpdateRequest {
        let operation = key(33);
        UpdateRequest {
            operation,
            expected_view: ViewRef {
                workspace: operation.workspace,
                epoch: operation.epoch,
                sequence: 0,
                digest: Digest([7; 32]),
            },
            kind: UpdateKind::SourceAdvance(SourceTransition {
                expected_source: SourceCut {
                    owners: MapRoot(None),
                    paths: MapRoot(None),
                    frontiers: MapRoot(None),
                },
                owners: OwnerMutationSet {
                    root: MapRoot(None),
                    count: 0,
                    digest: Digest([8; 32]),
                },
                events: vec![SourceEvent {
                    producer: operation.producer,
                    stream: operation.stream,
                    stream_epoch: operation.stream_epoch,
                    predecessor: 0,
                    sequence: 1,
                    payload: Digest([9; 32]),
                }],
            }),
        }
    }

    #[test]
    fn borrowed_submit_matches_canonical_wire_and_original_ack_classification() {
        let request = source_submit_request();
        let canonical = NativeIndexRequestEnvelope {
            request_id: 1,
            payload: NativeIndexRequest::Submit(request.clone()),
        };
        let borrowed_wire = quanta_index_ipc::encode_request(&BorrowedSubmitEnvelope {
            request_id: 1,
            request: &request,
        })
        .expect("borrowed submit wire");
        let canonical_wire =
            quanta_index_ipc::encode_request(&canonical).expect("canonical submit wire");
        assert_eq!(borrowed_wire, canonical_wire);

        let operation = request.operation;
        let accepted = SubmitOutcome::Unknown(operation);
        let response = NativeIndexResponse::Submitted(OperationReply {
            operation,
            payload: accepted.clone(),
        });
        let actual = scripted_reply(response, |client, budget| {
            client.submit_borrowed(&request, budget)
        });
        assert!(matches!(actual, Ok(outcome) if outcome == accepted));

        let wrong_key = key(34);
        let wrong = NativeIndexResponse::Submitted(OperationReply {
            operation: wrong_key,
            payload: SubmitOutcome::Unknown(wrong_key),
        });
        let actual = scripted_reply(wrong, |client, budget| {
            client.submit_borrowed(&request, budget)
        });
        assert!(matches!(actual, Err(NativeUdsError::Unknown(key)) if key == operation));

        let actual = scripted_reply(
            NativeIndexResponse::Staged(ObjectRef(Digest([3; 32]))),
            |client, budget| client.submit_borrowed(&request, budget),
        );
        assert!(matches!(actual, Err(NativeUdsError::Unknown(key)) if key == operation));
    }

    #[test]
    fn borrowed_mutation_page_matches_canonical_wire() -> Result<(), Box<dyn std::error::Error>> {
        let ingress = IngressRef {
            operation: key(43),
            fence: 17,
        };
        let base = MapRoot(Some(ObjectRef(Digest([5; 32]))));
        let mutations = [OwnerMutation {
            owner: OwnerKey {
                producer: ingress.operation.producer,
                stable_id: StableOwnerId([3; 16]),
            },
            expected: Some(ObjectRef(Digest([6; 32]))),
            next: None,
        }];
        let borrowed = BorrowedMutationPageEnvelope {
            request_id: 99,
            ingress,
            base,
            mutations: &mutations,
        };
        let canonical = NativeIndexRequestEnvelope {
            request_id: 99,
            payload: NativeIndexRequest::StageMutationPage {
                ingress,
                base,
                mutations: mutations.to_vec(),
            },
        };
        if quanta_index_ipc::encode_request(&borrowed)?
            != quanta_index_ipc::encode_request(&canonical)?
        {
            return Err("borrowed mutation page changed canonical CBOR frame".into());
        }
        Ok(())
    }

    #[test]
    fn admitted_source_owner_lookup_binds_rows_and_retains_charge()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = ViewRef {
            workspace: key(1).workspace,
            epoch: 1,
            sequence: 1,
            digest: Digest([7; 32]),
        };
        let owners = NativeSourceOwnerKeys::new(
            (0..128)
                .map(|id| OwnerKey {
                    producer: ProducerId([1; 16]),
                    stable_id: StableOwnerId([id; 16]),
                })
                .collect(),
        )?;
        let source = SourceCut {
            owners: MapRoot(Some(ObjectRef(Digest([2; 32])))),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let rows = NativeSourceOwnerRows::new(
            owners
                .as_slice()
                .iter()
                .map(|owner| NativeSourceOwnerBinding {
                    owner: *owner,
                    source: Some(ObjectRef(Digest([5; 32]))),
                })
                .collect(),
        )?;
        let (memory, active, _) = publication_scope(false);
        let request = NativeIndexRequest::LookupSourceOwners {
            selection: ViewSelection::Exact(view),
            owners: owners.clone(),
        };
        let response = NativeIndexResponse::SourceOwners(NativeSourceOwnerLookup {
            view,
            source,
            rows: rows.clone(),
        });
        let reply = scripted_reply(response.clone(), |client, budget| {
            client.request_publication_admitted(request.clone(), budget, &memory)
        })?;
        if reply.response() != &response {
            return Err("admitted source-owner response changed".into());
        }
        let required = u64::try_from(
            128usize
                .checked_mul(std::mem::size_of::<NativeSourceOwnerBinding>())
                .ok_or("source-owner backing overflow")?,
        )?;
        if active.load(AtomicOrdering::SeqCst) < required {
            return Err("source-owner output did not retain backing charge".into());
        }
        drop(reply);
        if active.load(AtomicOrdering::SeqCst) != 0 {
            return Err("source-owner output charge was not refunded".into());
        }
        for foreign in [
            NativeSourceOwnerLookup {
                view: ViewRef {
                    sequence: 2,
                    ..view
                },
                source,
                rows: rows.clone(),
            },
            NativeSourceOwnerLookup {
                view,
                source,
                rows: NativeSourceOwnerRows::new(
                    rows.as_slice()
                        .get(..127)
                        .ok_or("source-owner short fixture range absent")?
                        .to_vec(),
                )?,
            },
            NativeSourceOwnerLookup {
                view,
                source,
                rows: NativeSourceOwnerRows::new(
                    owners
                        .as_slice()
                        .iter()
                        .map(|owner| NativeSourceOwnerBinding {
                            owner: OwnerKey {
                                producer: ProducerId([9; 16]),
                                ..*owner
                            },
                            source: None,
                        })
                        .collect(),
                )?,
            },
        ] {
            let result = scripted_reply(
                NativeIndexResponse::SourceOwners(foreign),
                |client, budget| {
                    client.request_publication_admitted(request.clone(), budget, &memory)
                },
            );
            if !matches!(result, Err(NativeUdsError::Protocol(_))) {
                return Err("source-owner response accepted a foreign binding".into());
            }
            if active.load(AtomicOrdering::SeqCst) != 0 {
                return Err("source-owner output charge was not refunded".into());
            }
        }
        Ok(())
    }

    #[test]
    fn admitted_source_owner_lookup_refusal_precedes_connect() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket = directory.path().join("native-owner-denied.sock");
        let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let (memory, active, admissions) = publication_scope(true);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let client = NativeUdsClient::new(socket);
        let result = client.request_publication_admitted(
            NativeIndexRequest::LookupSourceOwners {
                selection: ViewSelection::Exact(ViewRef {
                    workspace: key(1).workspace,
                    epoch: 1,
                    sequence: 1,
                    digest: Digest([7; 32]),
                }),
                owners: NativeSourceOwnerKeys::new(vec![OwnerKey {
                    producer: ProducerId([1; 16]),
                    stable_id: StableOwnerId([1; 16]),
                }])
                .map_err(str::to_owned)?,
            },
            &budget,
            &memory,
        );
        if !matches!(result, Err(NativeUdsError::Transport(_)))
            || admissions.load(AtomicOrdering::SeqCst) == 0
            || active.load(AtomicOrdering::SeqCst) != 0
            || !matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        {
            return Err("denied source-owner read contacted the peer or lost its charge".into());
        }
        Ok(())
    }

    #[test]
    fn admitted_projection_source_diff_refusal_precedes_connect() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let socket = directory.path().join("native-diff-denied.sock");
        let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let (memory, active, admissions) = publication_scope(true);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let client = NativeUdsClient::new(socket);
        let result = client.request_publication_admitted(
            NativeIndexRequest::DiffProjectionSourceOwners {
                selection: ViewSelection::Exact(ViewRef {
                    workspace: key(1).workspace,
                    epoch: 1,
                    sequence: 1,
                    digest: Digest([7; 32]),
                }),
                projection: ProjectionId([4; 16]),
                producer: ProducerId([1; 16]),
                after: None,
                limit: 1,
            },
            &budget,
            &memory,
        );
        if !matches!(result, Err(NativeUdsError::Transport(_)))
            || admissions.load(AtomicOrdering::SeqCst) == 0
            || active.load(AtomicOrdering::SeqCst) != 0
            || !matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        {
            return Err("denied source diff contacted the peer or lost its charge".into());
        }
        Ok(())
    }

    #[test]
    fn projection_source_diff_rejects_foreign_axes_and_moving_continuation()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_contract::native_protocol::{
            NativeProjectionSourceDiff, NativeSourceDiffRows,
        };
        let view = ViewRef {
            workspace: key(1).workspace,
            epoch: 1,
            sequence: 1,
            digest: Digest([7; 32]),
        };
        let projection = ProjectionId([4; 16]);
        let producer = ProducerId([1; 16]);
        let selected = ViewSelection::Exact(view);
        let page = NativeProjectionSourceDiff {
            view,
            projection,
            binding: None,
            source: quanta_index_contract::index::SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            producer,
            rows: NativeSourceDiffRows::new(vec![OwnerMutation {
                owner: OwnerKey {
                    producer,
                    stable_id: StableOwnerId([2; 16]),
                },
                expected: Some(ObjectRef(Digest([8; 32]))),
                next: None,
            }])?,
            has_more: false,
        };
        let request = NativeIndexRequest::DiffProjectionSourceOwners {
            selection: selected,
            projection,
            producer,
            after: None,
            limit: 1,
        };
        if !publication_response_matches_request(
            &request,
            &NativeIndexResponse::ProjectionSourceDiff(page.clone()),
        ) {
            return Err("canonical source diff refused".into());
        }
        let mut foreign_view = page.clone();
        foreign_view.view.sequence = 2;
        let mut foreign_projection = page.clone();
        foreign_projection.projection = ProjectionId([9; 16]);
        let mut foreign_producer = page.clone();
        foreign_producer.producer = ProducerId([9; 16]);
        for invalid in [foreign_view, foreign_projection, foreign_producer] {
            if publication_response_matches_request(
                &request,
                &NativeIndexResponse::ProjectionSourceDiff(invalid),
            ) {
                return Err("foreign source diff accepted".into());
            }
        }
        if projection_source_diff_matches_request(
            selected,
            projection,
            producer,
            Some(StableOwnerId([2; 16])),
            1,
            &page,
        ) || projection_source_diff_matches_request(
            ViewSelection::Latest {
                workspace: view.workspace,
                epoch: view.epoch,
            },
            projection,
            producer,
            Some(StableOwnerId([1; 16])),
            1,
            &page,
        ) || projection_source_diff_matches_request(
            selected, projection, producer, None, 0, &page,
        ) {
            return Err("invalid source diff cursor/limit accepted".into());
        }
        Ok(())
    }

    #[test]
    fn projection_owner_lookup_rejects_foreign_axes_and_unbound_contribution()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = ViewRef {
            workspace: key(1).workspace,
            epoch: 1,
            sequence: 1,
            digest: Digest([7; 32]),
        };
        let projection = ProjectionId([4; 16]);
        let owner = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([1; 16]),
        };
        let owners = NativeSourceOwnerKeys::new(vec![owner])?;
        let valid = NativeProjectionOwnerLookup {
            view,
            projection,
            binding: None,
            rows: quanta_index_contract::native_protocol::NativeProjectionOwnerRows::new(vec![
                quanta_index_contract::native_protocol::NativeProjectionOwnerBinding {
                    owner,
                    contribution: None,
                },
            ])?,
        };
        let selection = ViewSelection::Exact(view);
        assert!(projection_owner_lookup_matches_request(
            selection, projection, &owners, &valid
        ));
        let mut foreign = valid.clone();
        foreign.projection = ProjectionId([5; 16]);
        assert!(!projection_owner_lookup_matches_request(
            selection, projection, &owners, &foreign
        ));
        foreign = valid.clone();
        foreign.view.sequence += 1;
        assert!(!projection_owner_lookup_matches_request(
            selection, projection, &owners, &foreign
        ));
        foreign = valid.clone();
        foreign.rows =
            quanta_index_contract::native_protocol::NativeProjectionOwnerRows::new(vec![
                quanta_index_contract::native_protocol::NativeProjectionOwnerBinding {
                    owner,
                    contribution: Some(ObjectRef(Digest([9; 32]))),
                },
            ])?;
        assert!(!projection_owner_lookup_matches_request(
            selection, projection, &owners, &foreign
        ));
        Ok(())
    }

    #[test]
    fn source_owner_lookup_sdk_rejects_mismatched_view_and_rows()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = ViewRef {
            workspace: key(1).workspace,
            epoch: 1,
            sequence: 1,
            digest: Digest([7; 32]),
        };
        let owner_a = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([1; 16]),
        };
        let owner_b = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([2; 16]),
        };
        let owner_c = OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([3; 16]),
        };
        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let rows = |owners: &[OwnerKey]| -> Result<NativeSourceOwnerRows, &'static str> {
            NativeSourceOwnerRows::new(
                owners
                    .iter()
                    .map(|owner| NativeSourceOwnerBinding {
                        owner: *owner,
                        source: Some(ObjectRef(Digest([5; 32]))),
                    })
                    .collect(),
            )
        };
        let request_owners = vec![owner_a, owner_b];
        let correct = NativeIndexResponse::SourceOwners(NativeSourceOwnerLookup {
            view,
            source,
            rows: rows(&request_owners)?,
        });
        let returned = scripted_reply(correct, |client, budget| {
            client.lookup_source_owners(ViewSelection::Exact(view), request_owners.clone(), budget)
        })?;
        if returned.view != view || returned.rows.as_slice().len() != 2 {
            return Err("correct source-owner lookup rejected or truncated".into());
        }
        for reply in [
            NativeSourceOwnerLookup {
                view: ViewRef {
                    sequence: 2,
                    ..view
                },
                source,
                rows: rows(&request_owners)?,
            },
            NativeSourceOwnerLookup {
                view: ViewRef {
                    workspace: WorkspaceKey {
                        slot: SlotId([9; 16]),
                        ..view.workspace
                    },
                    ..view
                },
                source,
                rows: rows(&request_owners)?,
            },
            NativeSourceOwnerLookup {
                view,
                source,
                rows: rows(&[owner_a])?,
            },
            NativeSourceOwnerLookup {
                view,
                source,
                rows: rows(&[owner_a, owner_c])?,
            },
        ] {
            let outcome = scripted_reply(
                NativeIndexResponse::SourceOwners(reply),
                |client, budget| {
                    client.lookup_source_owners(
                        ViewSelection::Exact(view),
                        request_owners.clone(),
                        budget,
                    )
                },
            );
            if !matches!(outcome, Err(NativeUdsError::Protocol(_))) {
                return Err("mismatched source-owner lookup reply accepted".into());
            }
        }
        let latest = scripted_reply(
            NativeIndexResponse::SourceOwners(NativeSourceOwnerLookup {
                view: ViewRef { epoch: 2, ..view },
                source,
                rows: rows(&request_owners)?,
            }),
            |client, budget| {
                client.lookup_source_owners(
                    ViewSelection::Latest {
                        workspace: view.workspace,
                        epoch: view.epoch,
                    },
                    request_owners.clone(),
                    budget,
                )
            },
        );
        if !matches!(latest, Err(NativeUdsError::Protocol(_))) {
            return Err("Latest source-owner lookup accepted wrong epoch".into());
        }
        let absent = NativeUdsClient::new(PathBuf::from("/nonexistent/source-owners"));
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        for invalid in [
            Vec::new(),
            vec![owner_a, owner_a],
            vec![owner_b, owner_a],
            (0..=128)
                .map(|stable| OwnerKey {
                    producer: ProducerId([1; 16]),
                    stable_id: StableOwnerId([stable; 16]),
                })
                .collect(),
        ] {
            if !matches!(
                absent.lookup_source_owners(ViewSelection::Exact(view), invalid, &budget),
                Err(NativeUdsError::Protocol(_))
            ) {
                return Err("invalid source-owner keys reached transport".into());
            }
        }
        Ok(())
    }

    #[test]
    fn original_operation_binds_even_refused_and_unknown_replies() {
        assert!(matches!(
            validate_submit_reply(key(1), OperationReply {
                operation: key(2), payload: SubmitOutcome::Unknown(key(1)),
            }),
            Err(NativeUdsError::Unknown(operation)) if operation == key(1)
        ));
        assert!(matches!(
            validate_status_reply(
                key(1),
                OperationReply {
                    operation: key(2),
                    payload: OperationStatus::Unknown,
                }
            ),
            Err(NativeUdsError::Protocol(_))
        ));
    }

    #[test]
    fn cancelled_before_send_is_a_typed_pre_admission_refusal() {
        let client = NativeUdsClient::new(PathBuf::from("/nonexistent/native-index.sock"));
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(1));
        budget.cancel_handle().cancel();
        assert!(matches!(
            client.resume(key(1), &budget),
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            }))
        ));
    }

    #[test]
    fn stage_byte_preflight_has_boundary_and_interruption_precedence() {
        let active = RequestBudgetV1::for_duration(Duration::from_secs(10));
        assert!(check_stage_bytes_before_copy(8, 8, &active).is_ok());
        assert!(matches!(
            check_stage_bytes_before_copy(9, 8, &active),
            Err(NativeUdsError::Protocol(message))
                if message == "native stage raw bytes exceed IPC decoded-request cap"
        ));
        assert!(
            check_stage_bytes_before_copy(
                MAX_MULTIFRAME_REQUEST_BODY_BYTES,
                MAX_MULTIFRAME_REQUEST_BODY_BYTES,
                &active,
            )
            .is_ok()
        );
        assert!(matches!(
            check_stage_bytes_before_copy(
                MAX_MULTIFRAME_REQUEST_BODY_BYTES + 1,
                MAX_MULTIFRAME_REQUEST_BODY_BYTES,
                &active,
            ),
            Err(NativeUdsError::Protocol(_))
        ));

        let cancelled = RequestBudgetV1::for_duration(Duration::from_secs(10));
        cancelled.cancel_handle().cancel();
        assert!(matches!(
            check_stage_bytes_before_copy(9, 8, &cancelled),
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            }))
        ));

        let expired = RequestBudgetV1::until(Instant::now());
        assert!(matches!(
            check_stage_bytes_before_copy(9, 8, &expired),
            Err(NativeUdsError::Transport(message))
                if message == "native request deadline elapsed"
        ));
    }

    #[test]
    fn stage_byte_slices_refuse_cancel_and_expiry_before_connect() {
        let client = NativeUdsClient::new(PathBuf::from("/nonexistent/native-index.sock"));
        let ingress = IngressRef {
            operation: key(1),
            fence: 7,
        };
        let cancelled = RequestBudgetV1::for_duration(Duration::from_secs(10));
        cancelled.cancel_handle().cancel();
        assert!(matches!(
            client.stage_source(
                ingress,
                "sample.rs".into(),
                b"small",
                SourceTextAdmission::Excluded,
                &cancelled,
            ),
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            }))
        ));
        assert!(matches!(
            client.stage_projection_output(ingress, b"small", &cancelled),
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            }))
        ));

        let expired = RequestBudgetV1::until(Instant::now());
        // A socket attempt would be classified as Unknown for this durable
        // stage. Transport here is the pre-connect deadline refusal.
        assert!(matches!(
            client.stage_source(
                ingress,
                "sample.rs".into(),
                b"small",
                SourceTextAdmission::Excluded,
                &expired,
            ),
            Err(NativeUdsError::Transport(_))
        ));
        assert!(matches!(
            client.stage_projection_output(ingress, b"small", &expired),
            Err(NativeUdsError::Transport(_))
        ));
    }

    fn held_one_shot_cancel(
        expected_request: fn(&NativeIndexRequest) -> bool,
        call: impl FnOnce(NativeUdsClient, RequestBudgetV1) -> Result<(), NativeUdsError>
        + Send
        + 'static,
    ) -> (Result<(), NativeUdsError>, Duration) {
        let directory = tempfile::tempdir().expect("socket directory");
        let socket = directory.path().join("held-one-shot.sock");
        let listener = UnixListener::bind(&socket).expect("bind held native socket");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept one-shot client");
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("one-shot request");
            assert!(expected_request(&request.payload));
            stream
                .write_all(&[1])
                .expect("send partial response header");
            ready_tx.send(()).expect("signal held reply");
            let _released = release_rx.recv_timeout(Duration::from_secs(2));
        });
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let client_budget = budget.clone();
        let client = NativeUdsClient::new(socket);
        let (result_tx, result_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = call(client, client_budget);
            result_tx
                .send(result)
                .expect("return cancelled one-shot result");
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("peer must hold an incomplete response");
        let started = Instant::now();
        budget.cancel_handle().cancel();
        let observed = result_rx.recv_timeout(Duration::from_secs(1));
        let elapsed = started.elapsed();
        let _released = release_tx.send(());
        let worker_join = worker.join();
        let peer_join = peer.join();
        worker_join.expect("one-shot client thread");
        peer_join.expect("held peer thread");
        let result = observed.expect("original cancellation must interrupt one-shot I/O");
        (result, elapsed)
    }

    #[test]
    fn text_and_semantic_one_shot_cancel_before_original_deadline() {
        let text = text_result();
        let (text_result, text_elapsed) = held_one_shot_cancel(
            |request| matches!(request, NativeIndexRequest::SearchText { .. }),
            move |client, budget| {
                client
                    .search_text(
                        ViewSelection::Exact(text.view),
                        text.instance,
                        text_query(1),
                        &budget,
                    )
                    .map(|_result| ())
            },
        );
        assert!(matches!(
            text_result,
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            }))
        ));
        assert!(text_elapsed < Duration::from_secs(1));

        let semantic = semantic_result();
        let (semantic_result, semantic_elapsed) = held_one_shot_cancel(
            |request| matches!(request, NativeIndexRequest::SearchSemantic { .. }),
            move |client, budget| {
                client
                    .search_semantic(
                        ViewSelection::Exact(semantic.view),
                        semantic.instance,
                        semantic_query(1),
                        &budget,
                    )
                    .map(|_result| ())
            },
        );
        assert!(matches!(
            semantic_result,
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            }))
        ));
        assert!(semantic_elapsed < Duration::from_secs(1));
    }

    #[test]
    fn cancelled_in_flight_durable_one_shot_keeps_original_operation_unknown() {
        let original = key(111);
        let (resumed, elapsed) = held_one_shot_cancel(
            |request| matches!(request, NativeIndexRequest::Resume(actual) if *actual == key(111)),
            move |client, budget| client.resume(original, &budget).map(|_outcome| ()),
        );
        assert!(matches!(resumed, Err(NativeUdsError::Unknown(key)) if key == original));
        assert!(elapsed < Duration::from_secs(1));

        let (opened, elapsed) = held_one_shot_cancel(
            |request| matches!(request, NativeIndexRequest::BeginInput(actual) if *actual == key(111)),
            move |client, budget| client.begin_input(original, &budget).map(|_input| ()),
        );
        assert!(matches!(opened, Err(NativeUdsError::Unknown(key)) if key == original));
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn stage_replies_keep_original_operation_on_wrong_ack_or_response_kind() {
        let original = key(1);
        let ingress = IngressRef {
            operation: original,
            fence: 7,
        };
        let wrong_operation = IngressRef {
            operation: key(2),
            fence: 7,
        };
        assert!(matches!(
            scripted_reply(NativeIndexResponse::InputOpened(wrong_operation), |client, budget| client.begin_input(original, budget)),
            Err(NativeUdsError::Unknown(operation)) if operation == original
        ));
        assert!(matches!(
            scripted_reply(NativeIndexResponse::Staged(ObjectRef(Digest([1; 32]))), |client, budget| client.begin_input(original, budget)),
            Err(NativeUdsError::Unknown(operation)) if operation == original
        ));
        assert!(matches!(
            scripted_reply(NativeIndexResponse::InputDiscarded(IngressRef { fence: 8, ..ingress }), |client, budget| client.discard_input(ingress, budget)),
            Err(NativeUdsError::Unknown(operation)) if operation == original
        ));
    }

    #[test]
    fn batch_cardinality_and_seal_root_count_reconcile_original_operation() {
        let original = key(3);
        let ingress = IngressRef {
            operation: original,
            fence: 9,
        };
        assert!(matches!(
            scripted_reply(NativeIndexResponse::StagedSources(Vec::new()), |client, budget| client.stage_sources(
                ingress,
                vec![SourceInput { path: "a.rs".into(), bytes: b"source".to_vec(), text: SourceTextAdmission::Excluded }],
                budget,
            )),
            Err(NativeUdsError::Unknown(operation)) if operation == original
        ));
        for invalid in [
            OwnerMutationSet {
                root: MapRoot(Some(ObjectRef(Digest([2; 32])))),
                count: 0,
                digest: Digest([3; 32]),
            },
            OwnerMutationSet {
                root: MapRoot(None),
                count: 1,
                digest: Digest([3; 32]),
            },
        ] {
            assert!(matches!(
                scripted_reply(NativeIndexResponse::MutationsStaged(invalid), |client, budget| client.stage_mutations(ingress, std::iter::empty(), budget)),
                Err(NativeUdsError::Unknown(operation)) if operation == original
            ));
        }
    }

    #[test]
    fn borrowed_source_batch_matches_canonical_wire_and_scripted_uds_reply() {
        let ingress = IngressRef {
            operation: key(31),
            fence: 19,
        };
        let owned = vec![
            SourceInput {
                path: "src/a.rs".into(),
                bytes: vec![0, 23, 255],
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([7; 32]),
                },
            },
            SourceInput {
                path: "src/b.rs".into(),
                bytes: vec![1, 24, 128],
                text: SourceTextAdmission::Excluded,
            },
        ];
        let borrowed = owned
            .iter()
            .map(|input| BorrowedSourceInput {
                path: input.path.as_str(),
                bytes: input.bytes.as_slice(),
                text: input.text,
            })
            .collect::<Vec<_>>();
        let expected = NativeIndexRequestEnvelope {
            request_id: 1,
            payload: NativeIndexRequest::StageSources {
                ingress,
                inputs: owned.clone(),
            },
        };
        let borrowed_wire = quanta_index_ipc::encode_request(&StageSourcesEnvelope {
            request_id: 1,
            ingress,
            inputs: &borrowed,
        })
        .expect("borrowed source wire");
        let canonical_wire =
            quanta_index_ipc::encode_request(&expected).expect("canonical source wire");
        assert_eq!(borrowed_wire, canonical_wire);

        let directory = tempfile::tempdir().expect("socket directory");
        let socket = directory.path().join("native.sock");
        let listener = UnixListener::bind(&socket).expect("bind scripted native socket");
        let references = vec![ObjectRef(Digest([8; 32])), ObjectRef(Digest([9; 32]))];
        let reply_references = references.clone();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept borrowed source batch");
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("decode borrowed batch");
            assert_eq!(request, expected);
            let reply = NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: NativeIndexResponse::StagedSources(reply_references),
            };
            stream
                .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode reply"))
                .expect("write stage reply");
        });
        let client = NativeUdsClient::new(socket.as_path());
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        let result = client.stage_sources_borrowed(ingress, &borrowed, &budget);
        peer.join().expect("scripted native peer");
        assert_eq!(result.expect("borrowed stage reply"), references);
    }

    #[test]
    fn borrowed_source_batch_refuses_invalid_input_before_connect() {
        let client = NativeUdsClient::new(Path::new("/nonexistent/borrowed-source-batch"));
        let ingress = IngressRef {
            operation: key(32),
            fence: 20,
        };
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        assert!(matches!(
            client.stage_sources_borrowed(ingress, &[], &budget),
            Err(NativeUdsError::Protocol(_))
        ));
        let too_many = (0..=MAX_NATIVE_STAGE_SOURCES_INPUTS)
            .map(|_index| BorrowedSourceInput {
                path: "a.rs",
                bytes: b"x",
                text: SourceTextAdmission::Excluded,
            })
            .collect::<Vec<_>>();
        assert!(matches!(
            client.stage_sources_borrowed(ingress, &too_many, &budget),
            Err(NativeUdsError::Protocol(_))
        ));
        let oversized_path = "x".repeat(MAX_SOURCE_PATH_BYTES + 1);
        let invalid = [BorrowedSourceInput {
            path: oversized_path.as_str(),
            bytes: b"source",
            text: SourceTextAdmission::Excluded,
        }];
        assert!(matches!(
            client.stage_sources_borrowed(ingress, &invalid, &budget),
            Err(NativeUdsError::Protocol(_))
        ));
        let oversized_bytes = vec![0; MAX_NATIVE_STAGE_SOURCES_WEIGHTED_BYTES];
        let invalid_size = [BorrowedSourceInput {
            path: "a.rs",
            bytes: &oversized_bytes,
            text: SourceTextAdmission::Excluded,
        }];
        assert!(matches!(
            client.stage_sources_borrowed(ingress, &invalid_size, &budget),
            Err(NativeUdsError::Protocol(_))
        ));
        let cancelled = RequestBudgetV1::for_duration(Duration::from_secs(2));
        cancelled.cancel_handle().cancel();
        let valid = [BorrowedSourceInput {
            path: "a.rs",
            bytes: b"source",
            text: SourceTextAdmission::Excluded,
        }];
        assert!(matches!(
            client.stage_sources_borrowed(ingress, &valid, &cancelled),
            Err(NativeUdsError::Refused(NativeIndexWireError {
                code: NativeIndexErrorCode::Cancelled,
                ..
            }))
        ));
    }

    #[test]
    fn failed_mutation_iterator_never_seals_a_partial_root() {
        let ingress = IngressRef {
            operation: key(4),
            fence: 9,
        };
        let directory = tempfile::tempdir().expect("socket directory");
        let socket = directory.path().join("native.sock");
        let listener = UnixListener::bind(&socket).expect("bind scripted native socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking scripted listener");
        let (done_tx, done_rx) = mpsc::channel();
        let peer = std::thread::spawn(move || {
            let started = Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(accepted) => break accepted,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && started.elapsed() < Duration::from_secs(3) =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("mutation page was not sent: {error}"),
                }
            };
            stream
                .set_nonblocking(false)
                .expect("blocking scripted mutation stream");
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("bounded mutation request read");
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).expect("mutation page request");
            assert!(matches!(
                request.payload,
                NativeIndexRequest::StageMutationPage {
                    ingress: actual,
                    base: MapRoot(None),
                    ref mutations,
                } if actual == ingress
                    && mutations.len() == 128
                    && mutations
                        .windows(2)
                        .all(|pair| matches!(pair, [left, right] if left.owner < right.owner))
            ));
            let staged_root = MapRoot(Some(ObjectRef(Digest([4; 32]))));
            let reply = NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: NativeIndexResponse::MutationPageStaged(staged_root),
            };
            stream
                .write_all(&quanta_index_ipc::encode_response(&reply).expect("encode reply"))
                .expect("send mutation page reply");
            done_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("producer did not return after iterator error");
            assert!(matches!(
                listener.accept(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
            ));
        });
        let client = NativeUdsClient::new(socket);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
        let mutations = (0u8..128)
            .rev()
            .map(|id| {
                Ok(OwnerMutation {
                    owner: OwnerKey {
                        producer: ProducerId([2; 16]),
                        stable_id: StableOwnerId([id; 16]),
                    },
                    expected: None,
                    next: None,
                })
            })
            .chain(std::iter::once(Err(NativeUdsError::Protocol(
                "producer stage failed".into(),
            ))));
        let result = client.stage_mutations_fallible(ingress, mutations, &budget);
        done_tx.send(()).expect("notify scripted native peer");
        peer.join().expect("scripted native peer");
        assert!(matches!(
            result,
            Err(NativeUdsError::Protocol(message)) if message == "producer stage failed"
        ));
    }

    fn text_query(top_k: u32) -> TextQueryRequest {
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k,
            cursor: None,
        }
    }

    fn text_result() -> NativeTextResult {
        NativeTextResult {
            view: ViewRef {
                workspace: key(1).workspace,
                epoch: 1,
                sequence: 3,
                digest: Digest([8; 32]),
            },
            instance: IndexInstanceId([9; 16]),
            coverage: NativeTextCoverage::CoreRequired,
            artifact: ObjectRef(Digest([10; 32])),
            matched: 1,
            exhaustive: true,
            hits: vec![NativeTextHit {
                owner: OwnerKey {
                    producer: ProducerId([5; 16]),
                    stable_id: StableOwnerId([11; 16]),
                },
                source: ObjectRef(Digest([12; 32])),
                path: "src/lib.rs".into(),
                score: FiniteScore::new(1.25).expect("finite score"),
            }],
        }
    }

    #[test]
    fn text_query_accepts_selected_core_reply() {
        let result = text_result();
        for selection in [
            ViewSelection::Exact(result.view),
            ViewSelection::Latest {
                workspace: result.view.workspace,
                epoch: result.view.epoch,
            },
        ] {
            let observed = scripted_reply(
                NativeIndexResponse::TextRanked(result.clone()),
                |client, budget| {
                    client.search_text(selection, result.instance, text_query(1), budget)
                },
            )
            .expect("selected text reply");
            assert_eq!(observed, result);
        }
    }

    #[test]
    fn text_query_rejects_wrong_view_instance_cardinality_and_path() {
        let valid = text_result();
        let selected = ViewSelection::Exact(valid.view);
        let mut invalid = Vec::new();
        let mut wrong_view = valid.clone();
        wrong_view.view.sequence += 1;
        invalid.push(wrong_view);
        let mut wrong_instance = valid.clone();
        wrong_instance.instance = IndexInstanceId([13; 16]);
        invalid.push(wrong_instance);
        let mut over_limit = valid.clone();
        over_limit.hits.push(
            over_limit
                .hits
                .first()
                .expect("fixed fixture has one hit")
                .clone(),
        );
        invalid.push(over_limit);
        let mut impossible_count = valid.clone();
        impossible_count.matched = 0;
        invalid.push(impossible_count);
        let mut absolute_path = valid.clone();
        absolute_path
            .hits
            .first_mut()
            .expect("fixed fixture has one hit")
            .path = "/src/lib.rs".into();
        invalid.push(absolute_path);
        let mut oversize_path = valid.clone();
        oversize_path
            .hits
            .first_mut()
            .expect("fixed fixture has one hit")
            .path = "a".repeat(MAX_SOURCE_PATH_BYTES + 1);
        invalid.push(oversize_path);
        for reply in invalid {
            assert!(matches!(
                scripted_reply(NativeIndexResponse::TextRanked(reply), |client, budget| {
                    client.search_text(selected, valid.instance, text_query(1), budget)
                }),
                Err(NativeUdsError::Protocol(_))
            ));
        }
    }

    #[test]
    fn text_query_rejects_wrong_workspace_and_response_kind() {
        let valid = text_result();
        let selected = ViewSelection::Latest {
            workspace: valid.view.workspace,
            epoch: valid.view.epoch,
        };
        let mut wrong_workspace = valid.clone();
        wrong_workspace.view.workspace.collection = CollectionId([14; 16]);
        assert!(matches!(
            scripted_reply(
                NativeIndexResponse::TextRanked(wrong_workspace),
                |client, budget| {
                    client.search_text(selected, valid.instance, text_query(1), budget)
                }
            ),
            Err(NativeUdsError::Protocol(_))
        ));
        assert!(matches!(
            scripted_reply(
                NativeIndexResponse::PathPage(NativePathPage {
                    view: valid.view,
                    entries: Vec::new(),
                    has_more: false,
                }),
                |client, budget| {
                    client.search_text(selected, valid.instance, text_query(1), budget)
                }
            ),
            Err(NativeUdsError::Protocol(_))
        ));
    }

    #[test]
    fn text_query_rejects_duplicate_owners_and_rank_order_drift() {
        let valid = text_result();
        let selected = ViewSelection::Exact(valid.view);
        let mut duplicate = valid.clone();
        duplicate.matched = 2;
        duplicate.hits.push(
            duplicate
                .hits
                .first()
                .expect("fixed fixture has one hit")
                .clone(),
        );

        let mut score_inversion = valid.clone();
        score_inversion.matched = 2;
        let mut higher = score_inversion
            .hits
            .first()
            .expect("fixed fixture has one hit")
            .clone();
        higher.owner.stable_id = StableOwnerId([12; 16]);
        higher.score = FiniteScore::new(2.0).expect("finite score");
        score_inversion.hits.push(higher);

        let mut tie_inversion = valid.clone();
        tie_inversion.matched = 2;
        let mut lower_owner = tie_inversion
            .hits
            .first()
            .expect("fixed fixture has one hit")
            .clone();
        lower_owner.owner.stable_id = StableOwnerId([10; 16]);
        tie_inversion.hits.push(lower_owner);

        for reply in [duplicate, score_inversion, tie_inversion] {
            assert!(matches!(
                scripted_reply(NativeIndexResponse::TextRanked(reply), |client, budget| {
                    client.search_text(selected, valid.instance, text_query(2), budget)
                }),
                Err(NativeUdsError::Protocol(_))
            ));
        }
    }

    #[test]
    fn text_query_refuses_partial_scan_and_missing_ranked_hits() {
        let valid = text_result();
        let selected = ViewSelection::Exact(valid.view);
        let mut partial = valid.clone();
        partial.exhaustive = false;
        let mut omitted = valid.clone();
        omitted.matched = 2;
        for reply in [partial, omitted] {
            assert!(matches!(
                scripted_reply(NativeIndexResponse::TextRanked(reply), |client, budget| {
                    client.search_text(selected, valid.instance, text_query(2), budget)
                }),
                Err(NativeUdsError::Protocol(_))
            ));
        }
    }

    fn semantic_query(top_k: u32) -> SemanticQueryRequest {
        SemanticQueryRequest {
            query_text: "dense needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            lexical_scope: None,
            top_k,
        }
    }

    fn semantic_result() -> NativeSemanticResult {
        let instance = IndexInstanceId([9; 16]);
        let owner_root = MapRoot(Some(ObjectRef(Digest([16; 32]))));
        let desired = InputBinding {
            root: owner_root,
            schema: SchemaId([13; 16]),
            policy: Digest([14; 32]),
            context: Digest([15; 32]),
        };
        NativeSemanticResult {
            view: ViewRef {
                workspace: key(1).workspace,
                epoch: 1,
                sequence: 3,
                digest: Digest([8; 32]),
            },
            instance,
            coverage: CoverageBinding {
                instance,
                desired,
                covered: Some(desired),
                searchable: owner_root.0,
                status: CoverageStatus::Complete,
                owners: owner_root,
            },
            traversal:
                quanta_index_contract::native_protocol::NativeSemanticTraversal::ExactExhausted,
            exhaustive: true,
            scanned_owners: 2,
            skipped_stale: 0,
            hits: vec![NativeSemanticHit {
                owner: OwnerKey {
                    producer: ProducerId([5; 16]),
                    stable_id: StableOwnerId([11; 16]),
                },
                vector_id: b"vector-1".to_vec(),
                score: FiniteScore::new(0.75).expect("finite cosine"),
                chunk: None,
            }],
        }
    }

    #[test]
    fn overlay_semantic_traversal_rejects_forged_exhaustion() {
        use quanta_index_contract::native_protocol::NativeSemanticTraversal;
        let (query, _, valid, _) = overlay_effort_fixtures();
        let overlay = query.overlay.expect("overlay fixture context");
        for traversal in [
            NativeSemanticTraversal::ExactCapped,
            NativeSemanticTraversal::Approximate { capped: false },
            NativeSemanticTraversal::Approximate { capped: true },
        ] {
            let mut forged = valid.clone();
            forged.traversal = traversal;
            assert!(matches!(
                validate_overlay_semantic_result(
                    query.selection,
                    query.semantic_instance,
                    overlay.session,
                    overlay.expected_corpus,
                    1,
                    &forged,
                ),
                Err(NativeUdsError::Protocol(_))
            ));
            forged.exhaustive = false;
            validate_overlay_semantic_result(
                query.selection,
                query.semantic_instance,
                overlay.session,
                overlay.expected_corpus,
                1,
                &forged,
            )
            .expect("honest nonexhaustive traversal");
        }
    }

    #[test]
    fn semantic_query_accepts_complete_partial_pending_and_capped_scan_replies() {
        let complete = semantic_result();
        let selected = ViewSelection::Exact(complete.view);
        let mut capped = complete.clone();
        capped.exhaustive = false;
        capped.traversal =
            quanta_index_contract::native_protocol::NativeSemanticTraversal::ExactCapped;
        let mut partial = complete.clone();
        partial.coverage.status = CoverageStatus::Partial {
            missing: MapRoot(Some(ObjectRef(Digest([17; 32])))),
        };
        partial.coverage.covered = None;
        partial.exhaustive = false;
        partial.skipped_stale = 1;
        let mut pending = partial.clone();
        pending.coverage.status = CoverageStatus::Pending {
            previous_missing: MapRoot(None),
        };
        let mut approximate = complete.clone();
        approximate.exhaustive = false;
        approximate.traversal =
            quanta_index_contract::native_protocol::NativeSemanticTraversal::Approximate {
                capped: false,
            };
        let mut approximate_capped = approximate.clone();
        approximate_capped.traversal =
            quanta_index_contract::native_protocol::NativeSemanticTraversal::Approximate {
                capped: true,
            };
        for result in [
            complete,
            capped,
            partial,
            pending,
            approximate,
            approximate_capped,
        ] {
            for selection in [
                selected,
                ViewSelection::Latest {
                    workspace: result.view.workspace,
                    epoch: result.view.epoch,
                },
            ] {
                let observed = scripted_reply(
                    NativeIndexResponse::SemanticRanked(result.clone()),
                    |client, budget| {
                        client.search_semantic(
                            selection,
                            result.instance,
                            semantic_query(2),
                            budget,
                        )
                    },
                )
                .expect("selected semantic reply");
                assert_eq!(observed, result);
            }
        }
        let mut boundary = semantic_result();
        boundary.scanned_owners = 100_000;
        boundary
            .hits
            .first_mut()
            .expect("fixed fixture has one hit")
            .vector_id = vec![b'v'; 1024];
        assert_eq!(
            scripted_reply(
                NativeIndexResponse::SemanticRanked(boundary.clone()),
                |client, budget| client.search_semantic(
                    selected,
                    boundary.instance,
                    semantic_query(4096),
                    budget,
                )
            )
            .expect("inclusive native semantic top-k cap"),
            boundary
        );
        let mut multi_vector = semantic_result();
        multi_vector.scanned_owners = 1;
        let mut second = multi_vector
            .hits
            .first()
            .expect("fixed fixture has one hit")
            .clone();
        second.vector_id = b"vector-2".to_vec();
        second.score = FiniteScore::new(0.5).expect("finite cosine");
        multi_vector.hits.push(second);
        assert_eq!(
            scripted_reply(
                NativeIndexResponse::SemanticRanked(multi_vector.clone()),
                |client, budget| client.search_semantic(
                    selected,
                    multi_vector.instance,
                    semantic_query(2),
                    budget,
                )
            )
            .expect("multiple vectors may belong to one scanned owner"),
            multi_vector
        );
        let mut directory = semantic_result();
        directory.coverage.searchable = Some(ObjectRef(Digest([19; 32])));
        assert_eq!(
            scripted_reply(
                NativeIndexResponse::SemanticRanked(directory.clone()),
                |client, budget| client.search_semantic(
                    selected,
                    directory.instance,
                    semantic_query(2),
                    budget,
                )
            )
            .expect("semantic directory may differ from the owner map"),
            directory
        );
        let mut empty = semantic_result();
        empty.coverage.desired.root = MapRoot(None);
        empty.coverage.covered = Some(empty.coverage.desired);
        empty.coverage.searchable = None;
        empty.coverage.owners = MapRoot(None);
        empty.scanned_owners = 0;
        empty.hits.clear();
        assert_eq!(
            scripted_reply(
                NativeIndexResponse::SemanticRanked(empty.clone()),
                |client, budget| client.search_semantic(
                    selected,
                    empty.instance,
                    semantic_query(2),
                    budget,
                )
            )
            .expect("empty owner map has no scan or hits"),
            empty
        );
        let mut empty_directory = empty.clone();
        empty_directory.coverage.searchable = Some(ObjectRef(Digest([20; 32])));
        assert_eq!(
            scripted_reply(
                NativeIndexResponse::SemanticRanked(empty_directory.clone()),
                |client, budget| client.search_semantic(
                    selected,
                    empty_directory.instance,
                    semantic_query(2),
                    budget,
                )
            )
            .expect("empty owner map may retain a search directory"),
            empty_directory
        );
    }

    #[test]
    fn semantic_query_rejects_binding_coverage_scan_and_score_violations() {
        let valid = semantic_result();
        let selected = ViewSelection::Exact(valid.view);
        let mut invalid = Vec::new();
        for traversal in [
            quanta_index_contract::native_protocol::NativeSemanticTraversal::ExactCapped,
            quanta_index_contract::native_protocol::NativeSemanticTraversal::Approximate {
                capped: false,
            },
            quanta_index_contract::native_protocol::NativeSemanticTraversal::Approximate {
                capped: true,
            },
        ] {
            let mut forged = valid.clone();
            forged.traversal = traversal;
            invalid.push(forged);
        }
        let mut wrong_view = valid.clone();
        wrong_view.view.sequence += 1;
        invalid.push(wrong_view);
        let mut wrong_instance = valid.clone();
        wrong_instance.instance = IndexInstanceId([18; 16]);
        invalid.push(wrong_instance);
        let mut wrong_coverage_instance = valid.clone();
        wrong_coverage_instance.coverage.instance = IndexInstanceId([18; 16]);
        invalid.push(wrong_coverage_instance);
        let mut wrong_searchable = valid.clone();
        wrong_searchable.coverage.searchable = None;
        invalid.push(wrong_searchable);
        let mut incomplete_complete = valid.clone();
        incomplete_complete.coverage.covered = None;
        invalid.push(incomplete_complete);
        let mut exhaustive_partial = valid.clone();
        exhaustive_partial.coverage.status = CoverageStatus::Partial {
            missing: MapRoot(Some(ObjectRef(Digest([17; 32])))),
        };
        exhaustive_partial.coverage.covered = None;
        invalid.push(exhaustive_partial);
        let mut skipped_more_than_scanned = valid.clone();
        skipped_more_than_scanned.skipped_stale = 3;
        invalid.push(skipped_more_than_scanned);
        let mut skipped_but_exhaustive = valid.clone();
        skipped_but_exhaustive.skipped_stale = 1;
        invalid.push(skipped_but_exhaustive);
        let mut too_many_scanned = valid.clone();
        too_many_scanned.scanned_owners = 100_001;
        invalid.push(too_many_scanned);
        let mut empty_owners_with_hits = valid.clone();
        empty_owners_with_hits.coverage.searchable = None;
        empty_owners_with_hits.coverage.owners = MapRoot(None);
        invalid.push(empty_owners_with_hits);
        let mut empty_vector_id = valid.clone();
        empty_vector_id
            .hits
            .first_mut()
            .expect("fixed fixture has one hit")
            .vector_id
            .clear();
        invalid.push(empty_vector_id);
        let mut oversized_vector_id = valid.clone();
        oversized_vector_id
            .hits
            .first_mut()
            .expect("fixed fixture has one hit")
            .vector_id = vec![b'v'; 1025];
        invalid.push(oversized_vector_id);
        let mut high_cosine = valid.clone();
        high_cosine
            .hits
            .first_mut()
            .expect("fixed fixture has one hit")
            .score = FiniteScore::new(1.000_001).expect("finite score");
        invalid.push(high_cosine);
        let mut low_cosine = valid.clone();
        low_cosine
            .hits
            .first_mut()
            .expect("fixed fixture has one hit")
            .score = FiniteScore::new(-1.000_001).expect("finite score");
        invalid.push(low_cosine);
        for reply in invalid {
            assert!(matches!(
                scripted_reply(
                    NativeIndexResponse::SemanticRanked(reply),
                    |client, budget| {
                        client.search_semantic(selected, valid.instance, semantic_query(2), budget)
                    }
                ),
                Err(NativeUdsError::Protocol(_))
            ));
        }
        let latest = ViewSelection::Latest {
            workspace: valid.view.workspace,
            epoch: valid.view.epoch,
        };
        let mut wrong_workspace = valid.clone();
        wrong_workspace.view.workspace.collection = CollectionId([19; 16]);
        assert!(matches!(
            scripted_reply(
                NativeIndexResponse::SemanticRanked(wrong_workspace),
                |client, budget| client.search_semantic(
                    latest,
                    valid.instance,
                    semantic_query(2),
                    budget
                )
            ),
            Err(NativeUdsError::Protocol(_))
        ));
    }

    #[test]
    fn semantic_query_rejects_hits_without_enough_fresh_scanned_owners() {
        let valid = semantic_result();
        let selected = ViewSelection::Exact(valid.view);

        let mut hit_without_scan = valid.clone();
        hit_without_scan.scanned_owners = 0;

        let mut too_many_distinct_owners = valid.clone();
        too_many_distinct_owners.scanned_owners = 1;
        let mut other_owner = too_many_distinct_owners
            .hits
            .first()
            .expect("fixed fixture has one hit")
            .clone();
        other_owner.owner.stable_id = StableOwnerId([12; 16]);
        other_owner.vector_id = b"vector-2".to_vec();
        other_owner.score = FiniteScore::new(0.5).expect("finite cosine");
        too_many_distinct_owners.hits.push(other_owner);

        let mut hit_after_only_stale_scan = valid.clone();
        hit_after_only_stale_scan.coverage.status = CoverageStatus::Partial {
            missing: MapRoot(Some(ObjectRef(Digest([17; 32])))),
        };
        hit_after_only_stale_scan.coverage.covered = None;
        hit_after_only_stale_scan.exhaustive = false;
        hit_after_only_stale_scan.scanned_owners = 1;
        hit_after_only_stale_scan.skipped_stale = 1;

        for reply in [
            hit_without_scan,
            too_many_distinct_owners,
            hit_after_only_stale_scan,
        ] {
            assert!(matches!(
                scripted_reply(
                    NativeIndexResponse::SemanticRanked(reply),
                    |client, budget| client.search_semantic(
                        selected,
                        valid.instance,
                        semantic_query(2),
                        budget,
                    )
                ),
                Err(NativeUdsError::Protocol(_))
            ));
        }
    }

    #[test]
    fn semantic_query_rejects_duplicate_pair_rank_drift_and_response_kind() {
        let valid = semantic_result();
        let selected = ViewSelection::Exact(valid.view);
        let mut duplicate = valid.clone();
        duplicate.hits.push(
            duplicate
                .hits
                .first()
                .expect("fixed fixture has one hit")
                .clone(),
        );
        let mut score_inversion = valid.clone();
        let mut higher = score_inversion
            .hits
            .first()
            .expect("fixed fixture has one hit")
            .clone();
        higher.owner.stable_id = StableOwnerId([12; 16]);
        higher.score = FiniteScore::new(0.9).expect("finite cosine");
        score_inversion.hits.push(higher);
        let mut owner_tie_inversion = valid.clone();
        let mut lower_owner = owner_tie_inversion
            .hits
            .first()
            .expect("fixed fixture has one hit")
            .clone();
        lower_owner.owner.stable_id = StableOwnerId([10; 16]);
        owner_tie_inversion.hits.push(lower_owner);
        let mut vector_tie_inversion = valid.clone();
        let mut lower_vector = vector_tie_inversion
            .hits
            .first()
            .expect("fixed fixture has one hit")
            .clone();
        lower_vector.vector_id = b"vector-0".to_vec();
        vector_tie_inversion.hits.push(lower_vector);
        for reply in [
            duplicate,
            score_inversion,
            owner_tie_inversion,
            vector_tie_inversion,
        ] {
            assert!(matches!(
                scripted_reply(
                    NativeIndexResponse::SemanticRanked(reply),
                    |client, budget| {
                        client.search_semantic(selected, valid.instance, semantic_query(2), budget)
                    }
                ),
                Err(NativeUdsError::Protocol(_))
            ));
        }
        let mut over_top_k = valid.clone();
        over_top_k.hits.push(NativeSemanticHit {
            owner: OwnerKey {
                producer: ProducerId([5; 16]),
                stable_id: StableOwnerId([12; 16]),
            },
            vector_id: b"vector-2".to_vec(),
            score: FiniteScore::new(0.5).expect("finite cosine"),
            chunk: None,
        });
        assert!(matches!(
            scripted_reply(
                NativeIndexResponse::SemanticRanked(over_top_k),
                |client, budget| {
                    client.search_semantic(selected, valid.instance, semantic_query(1), budget)
                }
            ),
            Err(NativeUdsError::Protocol(_))
        ));
        assert!(matches!(
            scripted_reply(
                NativeIndexResponse::TextRanked(text_result()),
                |client, budget| client.search_semantic(
                    selected,
                    valid.instance,
                    semantic_query(2),
                    budget
                )
            ),
            Err(NativeUdsError::Protocol(_))
        ));
    }

    #[test]
    fn semantic_query_refuses_native_top_k_outside_1_to_4096_before_send() {
        let valid = semantic_result();
        let client = NativeUdsClient::new(PathBuf::from("/nonexistent/native-index.sock"));
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
        for top_k in [0, 4097] {
            assert!(matches!(
                client.search_semantic(
                    ViewSelection::Exact(valid.view),
                    valid.instance,
                    semantic_query(top_k),
                    &budget,
                ),
                Err(NativeUdsError::Protocol(_))
            ));
        }
        let mut oversized = semantic_query(1);
        oversized.query_text = "x".repeat(16 * 1024 + 1);
        assert!(matches!(
            client.search_semantic(
                ViewSelection::Exact(valid.view),
                valid.instance,
                oversized,
                &budget,
            ),
            Err(NativeUdsError::Protocol(_))
        ));
    }

    fn semantic_chunk_reply() -> NativeSemanticResult {
        let mut reply = semantic_result();
        let policy = reply.coverage.desired.policy;
        let hit = reply.hits.first_mut().expect("fixed semantic hit");
        hit.vector_id = vec![42; 32];
        hit.chunk = Some(
            quanta_index_contract::native_protocol::NativeSemanticChunk {
                id: Digest([42; 32]),
                payload: ObjectRef(Digest([31; 32])),
                input: quanta_index_contract::index::OwnerInputBinding {
                    source: Some(ObjectRef(Digest([32; 32]))),
                    projection: None,
                    producer_context: Digest([0; 32]),
                    derivation_policy: policy,
                },
                input_digest: Digest([33; 32]),
                span:
                    quanta_index_contract::native_protocol::NativeSemanticChunkSpan::SourcePayload {
                        start_byte: 0,
                        end_byte: 4,
                    },
            },
        );
        reply
    }

    fn semantic_chunk_mut(
        reply: &mut NativeSemanticResult,
    ) -> &mut quanta_index_contract::native_protocol::NativeSemanticChunk {
        reply
            .hits
            .first_mut()
            .and_then(|hit| hit.chunk.as_mut())
            .expect("fixed semantic chunk hit")
    }

    #[test]
    fn semantic_query_accepts_typed_source_and_projection_chunk_bindings() {
        let source = semantic_chunk_reply();
        let selection = ViewSelection::Exact(source.view);
        let observed = scripted_reply(
            NativeIndexResponse::SemanticRanked(source.clone()),
            |client, budget| {
                client.search_semantic(selection, source.instance, semantic_query(1), budget)
            },
        )
        .expect("selected source chunk binding");
        assert_eq!(observed, source);

        let mut projection = source;
        let chunk = semantic_chunk_mut(&mut projection);
        chunk.input.source = None;
        chunk.input.projection = Some(ObjectRef(Digest([34; 32])));
        chunk.input.producer_context = Digest([35; 32]);
        chunk.span =
            quanta_index_contract::native_protocol::NativeSemanticChunkSpan::ProjectionPayload {
                start_byte: 2,
                end_byte: 4,
            };
        let observed = scripted_reply(
            NativeIndexResponse::SemanticRanked(projection.clone()),
            |client, budget| {
                client.search_semantic(selection, projection.instance, semantic_query(1), budget)
            },
        )
        .expect("selected projection chunk binding");
        assert_eq!(observed, projection);

        semantic_chunk_mut(&mut projection).span =
            quanta_index_contract::native_protocol::NativeSemanticChunkSpan::RenderedMember {
                ordinal: 1,
            };
        let observed = scripted_reply(
            NativeIndexResponse::SemanticRanked(projection.clone()),
            |client, budget| {
                client.search_semantic(selection, projection.instance, semantic_query(1), budget)
            },
        )
        .expect("selected certified rendered member binding");
        assert_eq!(observed, projection);
    }

    #[test]
    fn semantic_query_rejects_unbound_chunk_identity_recipe_span_and_input_domain() {
        use quanta_index_contract::native_protocol::NativeSemanticChunkSpan;

        let valid = semantic_chunk_reply();
        let selection = ViewSelection::Exact(valid.view);
        let instance = valid.instance;
        let mut invalid = Vec::new();

        let mut id = valid.clone();
        semantic_chunk_mut(&mut id).id = Digest([43; 32]);
        invalid.push(id);

        let mut recipe = valid.clone();
        semantic_chunk_mut(&mut recipe).input.derivation_policy = Digest([44; 32]);
        invalid.push(recipe);

        let mut span = valid.clone();
        semantic_chunk_mut(&mut span).span = NativeSemanticChunkSpan::SourcePayload {
            start_byte: 5,
            end_byte: 4,
        };
        invalid.push(span);

        let mut mixed_source = valid.clone();
        semantic_chunk_mut(&mut mixed_source).input.projection = Some(ObjectRef(Digest([45; 32])));
        invalid.push(mixed_source);

        let mut mixed_projection = valid.clone();
        semantic_chunk_mut(&mut mixed_projection).span =
            NativeSemanticChunkSpan::ProjectionPayload {
                start_byte: 0,
                end_byte: 4,
            };
        invalid.push(mixed_projection);

        let mut rendered_source = valid.clone();
        semantic_chunk_mut(&mut rendered_source).span =
            NativeSemanticChunkSpan::RenderedMember { ordinal: 0 };
        invalid.push(rendered_source);

        let mut rendered_overflow = valid;
        let chunk = semantic_chunk_mut(&mut rendered_overflow);
        chunk.input.projection = Some(ObjectRef(Digest([45; 32])));
        chunk.span = NativeSemanticChunkSpan::RenderedMember { ordinal: 4096 };
        invalid.push(rendered_overflow);

        for reply in invalid {
            assert!(matches!(
                scripted_reply(
                    NativeIndexResponse::SemanticRanked(reply),
                    |client, budget| client.search_semantic(
                        selection,
                        instance,
                        semantic_query(1),
                        budget,
                    )
                ),
                Err(NativeUdsError::Protocol(message)) if message.contains("chunk binding")
            ));
        }
    }

    #[test]
    fn effort_stream_rejects_invalid_deep_chunk_before_callback() {
        let fast = text_result();
        let mut deep = semantic_chunk_reply();
        semantic_chunk_mut(&mut deep).input.derivation_policy = Digest([46; 32]);
        let (outcome, connected) = scripted_effort_stream(
            vec![
                NativeIndexResponse::TextRanked(fast),
                NativeIndexResponse::SemanticRanked(deep),
            ],
            None,
            |client, budget| {
                let mut fast_count = 0_u32;
                let mut deep_count = 0_u32;
                let result = client.query_effort_stream(effort_query(), budget, &mut |stage| {
                    match stage {
                        NativeEffortStreamStage::Fast(_) => {
                            fast_count = fast_count.saturating_add(1);
                        }
                        NativeEffortStreamStage::Deep(_) => {
                            deep_count = deep_count.saturating_add(1);
                        }
                        NativeEffortStreamStage::OverlayFast(_)
                        | NativeEffortStreamStage::OverlayDeep(_) => {
                            return Err(NativeUdsError::Protocol(
                                "durable effort received an overlay stage".into(),
                            ));
                        }
                    }
                    Ok(())
                });
                (result, fast_count, deep_count)
            },
        );
        assert!(connected);
        assert!(matches!(outcome.0, Err(NativeUdsError::Protocol(_))));
        assert_eq!(outcome.1, 1);
        assert_eq!(outcome.2, 0);
    }
}
