//! In-process Fast/Deep execution over one pinned native view and request budget.

use std::{sync::Arc, time::Instant};

use crate::native_overlay::{NativeOverlay, OverlayCorpusSnapshot, OverlaySessionKey};
pub use crate::native_overlay_query::ScopedOverlayTextResult;
pub use crate::native_semantic_result::ScopedOverlaySemanticResult;
use quanta_index_contract::{
    SemanticQueryRequest, TextQueryRequest,
    index::{IndexInstanceId, ViewRef, ViewSelection},
    native_protocol::{NativeOverlayCorpusStamp, NativeOverlayEffortContext},
};
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticLaneV1, DiagnosticSpanGuardV1, DiagnosticStageV1,
    DiagnosticTerminalV1, RequestBudgetV1,
    index::{
        IndexError, WorkBudget,
        effort::{
            CancellationStatus, ContextStatus, CoverageExactness, DecisionFrame, DecisionReason,
            EffortAuthority, EffortError, EffortGovernor, EffortPreference, ExecutionPhase,
            PolicyProposal, PolicyState, ProviderPermission, RequiredProofStatus, ResourceNeed,
            StageObligation, StageOffer,
        },
    },
};
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_semantic::native::{SemanticOverlayDocument, SemanticReadRequest};

use crate::native::{
    NativeDispatcher, NativeIndexService, ScopedNativeSemanticResult, ScopedNativeTextResult,
    native_semantic_result, native_text_result, plan_native_text,
};

const FAST_STAGE: u64 = 1;
const DEEP_STAGE: u64 = 2;

fn begin_query_diagnostic(
    request: &RequestBudgetV1,
    stage: DiagnosticStageV1,
) -> (Option<DiagnosticSpanGuardV1>, Option<DiagnosticContextV1>) {
    let Some(parent) = request.diagnostic_context() else {
        return (None, None);
    };
    let span = crate::runtime_diagnostic_span(parent, parent.enter(stage, DiagnosticLaneV1(0)));
    let child = span.as_ref().map(|span| parent.child_context(span));
    (span, child)
}

fn finish_query_diagnostic<T>(span: Option<DiagnosticSpanGuardV1>, result: &Result<T, IndexError>) {
    if let Some(span) = span {
        let terminal = match result {
            Ok(_) => DiagnosticTerminalV1::Ok,
            Err(IndexError::Cancelled) => DiagnosticTerminalV1::Cancelled { code: 1 },
            Err(_) => DiagnosticTerminalV1::Error { code: 1 },
        };
        let _finished = span.finish(terminal);
    }
}

/// Existing typed requests, plus only the orchestration choice and member IDs.
pub struct NativeEffortRequest {
    pub selection: ViewSelection,
    pub text_instance: IndexInstanceId,
    pub semantic_instance: IndexInstanceId,
    pub text_query: TextQueryRequest,
    pub semantic_query: SemanticQueryRequest,
    pub preference: EffortPreference,
    pub overlay: Option<NativeEffortOverlay>,
}

pub use crate::native_effort_ports::{
    NativeEffortClockPort, NativeEffortConfiguration, NativeEffortControl,
    NativeEffortDocumentContext, NativeEffortObservationHistory, NativeEffortSignalPort,
};

/// Publisher authentication is created by the transport, never payload fields.
pub struct NativeEffortOverlay {
    key: OverlaySessionKey,
    expected: Option<quanta_index_core::index::Digest>,
}
impl NativeEffortOverlay {
    pub(super) fn from_dispatch(
        input: NativeOverlayEffortContext,
        context: &quanta_index_ipc::DispatchContextV1,
    ) -> Result<Self, IndexError> {
        Ok(Self {
            key: OverlaySessionKey::from_dispatch(input.session, context)?,
            expected: input.expected_corpus,
        })
    }
}
pub enum NativeEffortFastResult {
    Durable(ScopedNativeTextResult),
    Overlay(ScopedOverlayTextResult),
}
impl NativeEffortFastResult {
    pub fn durable(&self) -> Result<&ScopedNativeTextResult, IndexError> {
        match self {
            Self::Durable(value) => Ok(value),
            Self::Overlay(_) => Err(IndexError::InvalidInput(
                "expected durable Fast result".into(),
            )),
        }
    }
    pub fn into_durable(self) -> Result<ScopedNativeTextResult, IndexError> {
        match self {
            Self::Durable(value) => Ok(value),
            Self::Overlay(_) => Err(IndexError::InvalidInput(
                "expected durable Fast result".into(),
            )),
        }
    }
    #[must_use]
    pub fn overlay(&self) -> Option<&ScopedOverlayTextResult> {
        match self {
            Self::Durable(_) => None,
            Self::Overlay(value) => Some(value),
        }
    }
    fn view(&self) -> ViewRef {
        match self {
            Self::Durable(value) => value.view,
            Self::Overlay(value) => value.view,
        }
    }
    fn corpus(&self) -> Option<&NativeOverlayCorpusStamp> {
        match self {
            Self::Durable(_) => None,
            Self::Overlay(value) => Some(&value.corpus),
        }
    }
}
pub enum NativeEffortDeepResult {
    Durable(ScopedNativeSemanticResult),
    Overlay(ScopedOverlaySemanticResult),
}
impl NativeEffortDeepResult {
    pub fn durable(&self) -> Result<&ScopedNativeSemanticResult, IndexError> {
        match self {
            Self::Durable(value) => Ok(value),
            Self::Overlay(_) => Err(IndexError::InvalidInput(
                "expected durable Deep result".into(),
            )),
        }
    }
    pub fn into_durable(self) -> Result<ScopedNativeSemanticResult, IndexError> {
        match self {
            Self::Durable(value) => Ok(value),
            Self::Overlay(_) => Err(IndexError::InvalidInput(
                "expected durable Deep result".into(),
            )),
        }
    }
    #[must_use]
    pub fn overlay(&self) -> Option<&ScopedOverlaySemanticResult> {
        match self {
            Self::Durable(_) => None,
            Self::Overlay(value) => Some(value),
        }
    }
    fn view(&self) -> ViewRef {
        match self {
            Self::Durable(value) => value.view,
            Self::Overlay(value) => value.view,
        }
    }
}
pub struct NativeEffortResult {
    pub view: ViewRef,
    pub fast: NativeEffortFastResult,
    pub deep: Option<NativeEffortDeepResult>,
    pub reason: DecisionReason,
}
pub enum NativeEffortStage<'a> {
    Fast(&'a ScopedNativeTextResult),
    Deep(&'a ScopedNativeSemanticResult),
    OverlayFast(&'a ScopedOverlayTextResult),
    OverlayDeep(&'a ScopedOverlaySemanticResult),
}
#[derive(Clone, Copy)]
pub struct NativeEffortSummary<'a> {
    pub view: ViewRef,
    pub reason: DecisionReason,
    pub deep_delivered: bool,
    pub corpus: Option<&'a NativeOverlayCorpusStamp>,
}
struct OverlayExecution<'a> {
    store: &'a NativeOverlay,
    snapshot: &'a OverlayCorpusSnapshot,
    stamp: &'a NativeOverlayCorpusStamp,
    request: &'a RequestBudgetV1,
}
impl OverlayExecution<'_> {
    fn validate(&self) -> Result<(), IndexError> {
        self.store
            .validate_corpus_request(self.snapshot, self.request)
    }
    fn context(&self) -> Result<NativeEffortDocumentContext<'_>, IndexError> {
        Ok(NativeEffortDocumentContext {
            session: self.stamp.session,
            identity: self.stamp.identity,
            revisions: &self.stamp.revisions,
            unchanged_for: self
                .store
                .corpus_unchanged_for_request(self.snapshot, self.request)?,
        })
    }
}
fn validate_overlay(context: Option<&OverlayExecution<'_>>) -> Result<(), IndexError> {
    if let Some(context) = context {
        context.validate()?;
    }
    Ok(())
}

/// Receives stages while the selected view and original request remain live.
///
/// Returning an error aborts subsequent stages and releases the session pin.
/// Cancellation and deadline are checked before and after each call; the
/// observer must return cooperatively because an in-flight call is not preempted.
pub trait NativeEffortObserver {
    fn observe(&mut self, stage: NativeEffortStage<'_>) -> Result<(), IndexError>;

    /// Called once for a successful query while the selected ticket is still
    /// pinned. A transport observer writes its terminal frame here.
    fn finish(&mut self, _summary: NativeEffortSummary<'_>) -> Result<(), IndexError> {
        Ok(())
    }
}

struct NoopNativeEffortObserver;

impl NativeEffortObserver for NoopNativeEffortObserver {
    fn observe(&mut self, _stage: NativeEffortStage<'_>) -> Result<(), IndexError> {
        Ok(())
    }
}

/// Retains the same state-root lease as the listener; no reopen or rebinding.
pub struct NativeIndexEffortHandle {
    dispatcher: Arc<NativeDispatcher>,
    _lease: Arc<StateRootLease>,
}

impl NativeIndexService {
    /// Retain the service root for an in-process, pinned Fast/Deep query.
    #[must_use]
    pub fn effort_handle(&self) -> NativeIndexEffortHandle {
        NativeIndexEffortHandle::new(Arc::clone(&self.dispatcher), Arc::clone(&self.lease))
    }
}

impl NativeIndexEffortHandle {
    pub(crate) fn new(dispatcher: Arc<NativeDispatcher>, lease: Arc<StateRootLease>) -> Self {
        Self {
            dispatcher,
            _lease: lease,
        }
    }

    pub fn query_effort(
        &self,
        request: NativeEffortRequest,
        control: NativeEffortControl<'_>,
        budget: &mut WorkBudget,
    ) -> Result<NativeEffortResult, IndexError> {
        self.query_effort_with_observer(request, control, budget, &mut NoopNativeEffortObserver)
    }

    /// Execute typed Fast and optional Deep stages under one pinned view and
    /// original work budget, delivering each validated result in stage order.
    pub fn query_effort_with_observer(
        &self,
        request: NativeEffortRequest,
        control: NativeEffortControl<'_>,
        budget: &mut WorkBudget,
        observer: &mut dyn NativeEffortObserver,
    ) -> Result<NativeEffortResult, IndexError> {
        query_effort_on_dispatcher(&self.dispatcher, request, control, budget, observer)
    }
}

pub(super) fn query_effort_on_dispatcher(
    dispatcher: &NativeDispatcher,
    request: NativeEffortRequest,
    control: NativeEffortControl<'_>,
    budget: &mut WorkBudget,
    observer: &mut dyn NativeEffortObserver,
) -> Result<NativeEffortResult, IndexError> {
    let NativeEffortRequest {
        selection,
        text_instance,
        semantic_instance,
        text_query,
        semantic_query,
        preference,
        overlay,
    } = request;
    let semantic_top_k = semantic_query.top_k;
    budget.checkpoint()?;
    let original_request = budget.request().clone();
    let clock = Instant::now();
    let start_ms = control.clock.map_or(0, NativeEffortClockPort::now_ms);
    if control.history.is_some() && control.clock.is_some() {
        return Err(IndexError::InvalidInput(
            "shared effort history cannot use a query-local clock".into(),
        ));
    }
    check_original(&original_request)?;

    let (snapshot, stamp, fast_request) = {
        let capture = quanta_index_core::index::effort::ReservationGuard::try_acquire(
            Arc::clone(&dispatcher.control_scheduler),
            dispatcher.control_need,
        )
        .ok_or_else(|| dispatcher.control_scheduler.denial())?;
        budget.bind_memory_scope(capture.memory_scope());
        let snapshot = if let Some(input) = &overlay {
            let store = dispatcher
                .overlay
                .get()
                .ok_or(IndexError::SnapshotUnavailable)?;
            if store.text_instance != text_instance {
                return Err(IndexError::InvalidInput(
                    "effort overlay text instance differs".into(),
                ));
            }
            let (span, child) =
                begin_query_diagnostic(&original_request, DiagnosticStageV1::QueryOverlaySnapshot);
            let captured = (|| {
                let mut scoped = budget.scoped_diagnostic_context(child);
                let captured = store.snapshot_corpus(input.key, &mut scoped)?;
                if input
                    .expected
                    .is_some_and(|expected| expected != captured.identity)
                {
                    return Err(IndexError::Conflict);
                }
                Ok(captured)
            })();
            finish_query_diagnostic(span, &captured);
            Some(captured?)
        } else {
            None
        };
        let fast_request = if let Some(snapshot) = &snapshot {
            crate::native_overlay_query::plan(snapshot, &text_query, budget)?
        } else {
            plan_native_text(&text_query, |bytes, lifetime| {
                budget.reserve_memory(bytes, lifetime)
            })?
        };
        let stamp = if let Some(snapshot) = &snapshot {
            Some(crate::native_overlay_query::stamp(
                snapshot,
                &|bytes, lifetime| budget.reserve_memory(bytes, lifetime),
            )?)
        } else {
            None
        };
        (snapshot, stamp, fast_request)
    };
    let mut session = {
        let control = quanta_index_core::index::effort::ReservationGuard::try_acquire(
            Arc::clone(&dispatcher.control_scheduler),
            dispatcher.control_need,
        )
        .ok_or_else(|| dispatcher.control_scheduler.denial())?;
        budget.bind_memory_scope(control.memory_scope());
        dispatcher.kernel.begin_query_session(selection, budget)?
    };
    let overlay_execution = if let (Some(snapshot), Some((stamp, _memory))) = (&snapshot, &stamp) {
        Some(OverlayExecution {
            store: dispatcher
                .overlay
                .get()
                .ok_or(IndexError::SnapshotUnavailable)?,
            snapshot,
            stamp,
            request: &original_request,
        })
    } else {
        None
    };
    if overlay_execution
        .as_ref()
        .is_some_and(|context| context.stamp.session.workspace != session.view().workspace)
    {
        return Err(IndexError::Conflict);
    }
    validate_overlay(overlay_execution.as_ref())?;
    let fast_result_limit = fast_request.result_limit()?;
    let view = session.view();
    let fast_stage = {
        let _control = quanta_index_core::index::effort::ReservationGuard::try_acquire(
            Arc::clone(&dispatcher.control_scheduler),
            dispatcher.control_need,
        )
        .ok_or_else(|| dispatcher.control_scheduler.denial())?;
        session.prepare_stage(&dispatcher.text_queries, text_instance, fast_request)?
    };
    let fast_offer = StageOffer {
        id: FAST_STAGE,
        phase: ExecutionPhase::Fast,
        obligation: StageObligation::Required,
        ready: true,
        requires_exact_coverage: false,
        requires_provider: false,
        estimated_latency_ms: None,
        reservation: fast_stage.required_resources(),
    };
    let state = PolicyState {
        phase: ExecutionPhase::Fast,
        entered_at_ms: start_ms,
        clear_since_ms: None,
        clear_valid_until_ms: None,
        automatic_transitions: 0,
    };
    let (frame, state, proposal) = observe_policy(
        control,
        clock,
        preference,
        overlay_execution.as_ref(),
        authority(
            &original_request,
            RequiredProofStatus::Pending,
            ProviderPermission::Denied,
            dispatcher.interactive_need,
        )?,
        std::slice::from_ref(&fast_offer),
        state,
    )?;
    check_original(&original_request)?;
    let admitted = EffortGovernor::new(Arc::clone(&dispatcher.interactive_scheduler))
        .admit(&frame, state, proposal)
        .map_err(effort_error)?;
    if admitted.runnable_stage != Some(FAST_STAGE) {
        fast_stage.abort();
        return Err(dispatcher.interactive_scheduler.denial());
    }
    let permit = admitted
        .permit
        .ok_or_else(|| dispatcher.interactive_scheduler.denial())?;
    let (fast_span, fast_child) =
        begin_query_diagnostic(&original_request, DiagnosticStageV1::QueryFast);
    let fast_outcome = fast_stage
        .with_diagnostic_context(fast_child)
        .execute(&permit);
    let fast_result = fast_outcome.and_then(|fast_outcome| {
        validate_overlay(overlay_execution.as_ref())?;
        if let Some(snapshot) = &snapshot {
            Ok(NativeEffortFastResult::Overlay(
                crate::native_overlay_query::convert(
                    fast_outcome,
                    text_instance,
                    fast_result_limit,
                    snapshot,
                    |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                )?,
            ))
        } else {
            Ok(NativeEffortFastResult::Durable(native_text_result(
                fast_outcome,
                text_instance,
                fast_result_limit,
                |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
            )?))
        }
    });
    finish_query_diagnostic(fast_span, &fast_result);
    let fast = fast_result?;
    drop(permit);
    if fast.view() != view {
        return Err(IndexError::BackendViolation(
            "Fast result changed selected view".into(),
        ));
    }
    check_original(&original_request)?;
    validate_overlay(overlay_execution.as_ref())?;
    observer.observe(match &fast {
        NativeEffortFastResult::Durable(value) => NativeEffortStage::Fast(value),
        NativeEffortFastResult::Overlay(value) => NativeEffortStage::OverlayFast(value),
    })?;
    validate_overlay(overlay_execution.as_ref())?;
    check_original(&original_request)?;

    // Fast is the only executed stage. Retain the observed pressure dwell
    // window, but never commit the proposal's unexecuted Deep transition.
    let state = PolicyState {
        clear_since_ms: admitted.next_state.clear_since_ms,
        clear_valid_until_ms: admitted.next_state.clear_valid_until_ms,
        ..state
    };

    check_original(&original_request)?;
    let empty: [StageOffer; 0] = [];
    // The initial Deep proposal could only execute the required Fast offer.
    // It must not commit a Deep state before that stage runs.
    let (_, _, intention) = observe_policy(
        control,
        clock,
        preference,
        overlay_execution.as_ref(),
        authority(
            &original_request,
            RequiredProofStatus::Satisfied,
            ProviderPermission::Denied,
            dispatcher.enrichment_need,
        )?,
        &empty,
        state,
    )?;
    check_original(&original_request)?;
    if intention.target_phase != ExecutionPhase::Deep {
        let result = NativeEffortResult {
            view,
            fast,
            deep: None,
            reason: intention.reason,
        };
        check_original(&original_request)?;
        session.renew()?;
        check_original(&original_request)?;
        validate_overlay(overlay_execution.as_ref())?;
        observer.finish(NativeEffortSummary {
            corpus: result.fast.corpus(),
            view,
            reason: result.reason,
            deep_delivered: false,
        })?;
        session.close()?;
        check_original(&original_request)?;
        return Ok(result);
    }
    if semantic_query
        .lexical_scope
        .as_ref()
        .is_some_and(|scope| scope.constraints != semantic_query.constraints)
    {
        return Err(IndexError::InvalidInput(
            "semantic and lexical scope constraints differ".into(),
        ));
    }

    // Deep's optional policy decision is made before funding its lexical
    // scope. Equal Fast/Deep scope reuses the already emitted Fast universe;
    // a distinct scope executes as another bounded Text stage under this
    // exact ticket, captured corpus, and original request budget.
    let lexical_eligibility = if let Some(scope) = semantic_query.lexical_scope.as_ref() {
        let attempted = (|| {
            let preparation = quanta_index_core::index::effort::ReservationGuard::try_acquire(
                Arc::clone(&dispatcher.control_scheduler),
                dispatcher.control_need,
            )
            .ok_or_else(|| dispatcher.control_scheduler.denial())?;
            let memory = preparation.memory_scope().ok_or_else(|| {
                IndexError::BackendViolation("Deep scope preparation has no memory scope".into())
            })?;
            session.bind_query_memory_scope(memory)?;
            validate_overlay(overlay_execution.as_ref())?;
            let eligible = if scope == &text_query {
                let eligible = match (&fast, &snapshot) {
                    (NativeEffortFastResult::Overlay(result), Some(snapshot)) => {
                        crate::native_lexical_scope::lexical_eligibility_from_overlay_fast(
                            result,
                            view,
                            snapshot,
                            |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                        )?
                    }
                    (NativeEffortFastResult::Durable(result), None) => {
                        crate::native_lexical_scope::lexical_eligibility_from_durable_fast(
                            result,
                            view,
                            |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                        )?
                    }
                    _ => {
                        return Err(IndexError::BackendViolation(
                            "Fast lexical origin differs from captured corpus".into(),
                        ));
                    }
                };
                drop(preparation);
                eligible
            } else {
                let planned = if let Some(snapshot) = &snapshot {
                    crate::native_overlay_query::plan_with_reservation(
                        snapshot,
                        scope,
                        |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                    )?
                } else {
                    plan_native_text(scope, |bytes, lifetime| {
                        session.reserve_query_memory(bytes, lifetime)
                    })?
                };
                let limit = planned.result_limit()?;
                let scope_stage =
                    session.prepare_stage(&dispatcher.text_queries, text_instance, planned)?;
                let scope_need = scope_stage.required_resources();
                drop(preparation);
                let Some(permit) = quanta_index_core::index::effort::ReservationGuard::try_acquire(
                    Arc::clone(&dispatcher.interactive_scheduler),
                    scope_need,
                ) else {
                    scope_stage.abort();
                    return Err(dispatcher.interactive_scheduler.denial());
                };
                check_original(&original_request)?;
                validate_overlay(overlay_execution.as_ref())?;
                let outcome = scope_stage.execute(&permit)?;
                validate_overlay(overlay_execution.as_ref())?;
                let eligible = crate::native_lexical_scope::lexical_eligibility_from_raw(
                    &outcome,
                    view,
                    limit,
                    snapshot.as_ref(),
                    |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                )?;
                drop(outcome);
                drop(permit);
                eligible
            };
            check_original(&original_request)?;
            validate_overlay(overlay_execution.as_ref())?;
            Ok(eligible)
        })();
        match attempted {
            Ok(eligible) => Some(eligible),
            Err(IndexError::ResourceLimited) => {
                // Capacity is optional after Fast has been delivered. A real
                // cancellation or expired original request remains terminal.
                check_original(&original_request)?;
                let result = NativeEffortResult {
                    view,
                    fast,
                    deep: None,
                    reason: DecisionReason::ResourceDenied,
                };
                session.renew()?;
                check_original(&original_request)?;
                validate_overlay(overlay_execution.as_ref())?;
                observer.finish(NativeEffortSummary {
                    corpus: result.fast.corpus(),
                    view,
                    reason: result.reason,
                    deep_delivered: false,
                })?;
                session.close()?;
                check_original(&original_request)?;
                return Ok(result);
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };

    let deep_stage = {
        let Some(preparation) = quanta_index_core::index::effort::ReservationGuard::try_acquire(
            Arc::clone(&dispatcher.control_scheduler),
            dispatcher.control_need,
        ) else {
            check_original(&original_request)?;
            optional_capacity_denial(dispatcher.control_scheduler.denial())?;
            let result = NativeEffortResult {
                view,
                fast,
                deep: None,
                reason: DecisionReason::ResourceDenied,
            };
            check_original(&original_request)?;
            session.renew()?;
            check_original(&original_request)?;
            validate_overlay(overlay_execution.as_ref())?;
            observer.finish(NativeEffortSummary {
                corpus: result.fast.corpus(),
                view,
                reason: result.reason,
                deep_delivered: false,
            })?;
            session.close()?;
            check_original(&original_request)?;
            return Ok(result);
        };
        session.renew()?;
        validate_overlay(overlay_execution.as_ref())?;
        let mut semantic_request = SemanticReadRequest::from(semantic_query);
        semantic_request.lexical_eligibility = lexical_eligibility;
        if let Some(snapshot) = &snapshot {
            let count = snapshot.documents.iter().filter(|row| !row.retired).count();
            if count > quanta_index_semantic::native::MAX_SEMANTIC_OVERLAY_DOCUMENTS {
                return Err(IndexError::ResourceLimited);
            }
            if count != 0 {
                let bytes = quanta_index_semantic::native::MAX_SEMANTIC_OVERLAY_DOCUMENTS
                    .checked_mul(std::mem::size_of::<SemanticOverlayDocument>())
                    .ok_or(IndexError::ResourceLimited)?;
                let memory = preparation.memory_scope().ok_or_else(|| {
                    IndexError::BackendViolation("Deep preparation has no memory scope".into())
                })?;
                check_original(&original_request)?;
                semantic_request.overlay_memory = Some(memory.reserve(
                    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                    quanta_index_core::index::MemoryLifetime::Retained,
                )?);
                check_original(&original_request)?;
                semantic_request
                    .overlay
                    .try_reserve_exact(count)
                    .map_err(|_error| IndexError::ResourceLimited)?;
                if semantic_request.overlay.capacity()
                    > quanta_index_semantic::native::MAX_SEMANTIC_OVERLAY_DOCUMENTS
                {
                    return Err(IndexError::ResourceLimited);
                }
                for row in snapshot.documents.iter().filter(|row| !row.retired) {
                    check_original(&original_request)?;
                    semantic_request.overlay.push(SemanticOverlayDocument {
                        path: row.path.clone(),
                        owner: row.owner,
                        revision: row.revision,
                        text: row.text.clone(),
                    });
                }
            }
        }
        session.prepare_optional_stage(
            &dispatcher.semantic_queries,
            semantic_instance,
            semantic_request,
        )?
    };
    let Some(deep_stage) = deep_stage else {
        let result = NativeEffortResult {
            view,
            fast,
            deep: None,
            reason: DecisionReason::ProviderUnavailable,
        };
        check_original(&original_request)?;
        session.renew()?;
        check_original(&original_request)?;
        validate_overlay(overlay_execution.as_ref())?;
        observer.finish(NativeEffortSummary {
            corpus: result.fast.corpus(),
            view,
            reason: result.reason,
            deep_delivered: false,
        })?;
        session.close()?;
        check_original(&original_request)?;
        return Ok(result);
    };
    let deep_offer = StageOffer {
        id: DEEP_STAGE,
        phase: ExecutionPhase::Deep,
        obligation: StageObligation::Optional,
        ready: true,
        requires_exact_coverage: false,
        requires_provider: true,
        estimated_latency_ms: None,
        reservation: deep_stage.required_resources(),
    };
    // Optional preparation may take longer than signal TTL or outlive an
    // interaction publisher. Reobserve after it and admit from this exact
    // frame/state/proposal, not from the pre-prepare intention.
    let deep_state_before = PolicyState {
        clear_since_ms: intention.next_state.clear_since_ms,
        clear_valid_until_ms: intention.next_state.clear_valid_until_ms,
        ..state
    };
    let (deep_frame, policy_state, deep_proposal) = observe_policy(
        control,
        clock,
        preference,
        overlay_execution.as_ref(),
        authority(
            &original_request,
            RequiredProofStatus::Satisfied,
            ProviderPermission::Allowed,
            dispatcher.enrichment_need,
        )?,
        std::slice::from_ref(&deep_offer),
        deep_state_before,
    )?;
    check_original(&original_request)?;
    let admitted = EffortGovernor::new(Arc::clone(&dispatcher.enrichment_scheduler))
        .admit(&deep_frame, policy_state, deep_proposal)
        .map_err(effort_error)?;
    if admitted.runnable_stage != Some(DEEP_STAGE) {
        if admitted.reason == DecisionReason::ResourceDenied {
            optional_capacity_denial(dispatcher.enrichment_scheduler.denial())?;
        }
        deep_stage.abort();
        let result = NativeEffortResult {
            view,
            fast,
            deep: None,
            reason: admitted.reason,
        };
        check_original(&original_request)?;
        session.renew()?;
        check_original(&original_request)?;
        validate_overlay(overlay_execution.as_ref())?;
        observer.finish(NativeEffortSummary {
            corpus: result.fast.corpus(),
            view,
            reason: result.reason,
            deep_delivered: false,
        })?;
        session.close()?;
        check_original(&original_request)?;
        return Ok(result);
    }
    let permit = admitted
        .permit
        .ok_or_else(|| dispatcher.enrichment_scheduler.denial())?;
    let (deep_span, deep_child) =
        begin_query_diagnostic(&original_request, DiagnosticStageV1::QueryDeep);
    let deep_result = if permit.need().provider_units > 0 {
        match crate::native_provider_custody::NativeProviderExecutionCustody::new_with_owner(
            permit,
            dispatcher.root_lease.clone(),
        ) {
            Ok(custody) => {
                let outcome = deep_stage
                    .with_diagnostic_context(deep_child)
                    .execute_with_provider_custody(custody.reservation(), &custody.port());
                outcome.and_then(|outcome| {
                    if let Some(snapshot) = &snapshot {
                        Ok(NativeEffortDeepResult::Overlay(
                            crate::native_semantic_result::convert_overlay(
                                outcome,
                                semantic_instance,
                                semantic_top_k,
                                snapshot,
                                |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                            )?,
                        ))
                    } else {
                        Ok(NativeEffortDeepResult::Durable(native_semantic_result(
                            outcome,
                            semantic_instance,
                            semantic_top_k,
                            |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                        )?))
                    }
                })
            }
            Err(error) => {
                deep_stage.abort();
                Err(error)
            }
        }
    } else {
        let outcome = deep_stage
            .with_diagnostic_context(deep_child)
            .execute(&permit);
        let result = outcome.and_then(|outcome| {
            if let Some(snapshot) = &snapshot {
                Ok(NativeEffortDeepResult::Overlay(
                    crate::native_semantic_result::convert_overlay(
                        outcome,
                        semantic_instance,
                        semantic_top_k,
                        snapshot,
                        |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                    )?,
                ))
            } else {
                Ok(NativeEffortDeepResult::Durable(native_semantic_result(
                    outcome,
                    semantic_instance,
                    semantic_top_k,
                    |bytes, lifetime| session.reserve_query_memory(bytes, lifetime),
                )?))
            }
        });
        drop(permit);
        result
    };
    finish_query_diagnostic(deep_span, &deep_result);
    let deep = deep_result?;
    if deep.view() != view {
        return Err(IndexError::BackendViolation(
            "Deep result changed selected view".into(),
        ));
    }
    check_original(&original_request)?;
    validate_overlay(overlay_execution.as_ref())?;
    observer.observe(match &deep {
        NativeEffortDeepResult::Durable(value) => NativeEffortStage::Deep(value),
        NativeEffortDeepResult::Overlay(value) => NativeEffortStage::OverlayDeep(value),
    })?;
    validate_overlay(overlay_execution.as_ref())?;
    check_original(&original_request)?;
    let result = NativeEffortResult {
        view,
        fast,
        deep: Some(deep),
        reason: admitted.reason,
    };
    check_original(&original_request)?;
    session.renew()?;
    check_original(&original_request)?;
    validate_overlay(overlay_execution.as_ref())?;
    observer.finish(NativeEffortSummary {
        corpus: result.fast.corpus(),
        view,
        reason: result.reason,
        deep_delivered: true,
    })?;
    session.close()?;
    check_original(&original_request)?;
    if preference == EffortPreference::Auto
        && let Some(history) = control.history
    {
        history.record_auto_deep_delivery();
    }
    Ok(result)
}

fn observe_policy<'a>(
    control: NativeEffortControl<'_>,
    query_clock: Instant,
    preference: EffortPreference,
    document: Option<&OverlayExecution<'_>>,
    authority: EffortAuthority,
    offers: &'a [StageOffer],
    legacy_state: PolicyState,
) -> Result<(DecisionFrame<'a>, PolicyState, PolicyProposal), IndexError> {
    validate_overlay(document)?;
    let document_context = document.map(OverlayExecution::context).transpose()?;
    let propose = |now_ms, signals, state| {
        let frame = DecisionFrame {
            now_ms,
            preference,
            signals,
            authority,
            useful_refinement: true,
            offers,
        };
        let proposal = control
            .policy
            .propose(&frame, state, control.estimator)
            .map_err(effort_error)?;
        Ok(((frame, state, proposal), proposal.next_state))
    };
    if let Some(history) = control.history {
        history.observe_for_document(control.signals, document_context.as_ref(), propose)
    } else {
        let sample_started_ms = match control.clock {
            Some(port) => port.now_ms(),
            None => elapsed_ms(query_clock)?,
        };
        let signals = document_context.as_ref().map_or_else(
            || control.signals.sample(sample_started_ms),
            |context| {
                control
                    .signals
                    .sample_for_document(sample_started_ms, context)
            },
        );
        let now_ms = match control.clock {
            Some(port) => port.now_ms(),
            None => elapsed_ms(query_clock)?,
        };
        if now_ms < sample_started_ms || now_ms == u64::MAX {
            return Err(IndexError::BackendViolation(
                "effort observation clock regressed or overflowed".into(),
            ));
        }
        let (result, _) = propose(now_ms, signals, legacy_state)?;
        Ok(result)
    }
}

fn optional_capacity_denial(denial: IndexError) -> Result<(), IndexError> {
    if denial == IndexError::ResourceLimited {
        Ok(())
    } else {
        Err(denial)
    }
}

fn elapsed_ms(clock: Instant) -> Result<u64, IndexError> {
    u64::try_from(clock.elapsed().as_millis())
        .map_err(|_conversion_error| IndexError::ResourceLimited)
}

pub(super) fn check_original(request: &RequestBudgetV1) -> Result<(), IndexError> {
    if request.is_cancelled() {
        Err(IndexError::Cancelled)
    } else if request.remaining().is_zero() {
        Err(IndexError::ResourceLimited)
    } else {
        Ok(())
    }
}

fn authority(
    request: &RequestBudgetV1,
    required_proof: RequiredProofStatus,
    provider: ProviderPermission,
    remaining: ResourceNeed,
) -> Result<EffortAuthority, IndexError> {
    let time_left = request.remaining();
    let remaining_time_ms = if time_left.is_zero() {
        0
    } else {
        u64::try_from(time_left.as_millis())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?
            .max(1)
    };
    Ok(EffortAuthority {
        context: ContextStatus::Current,
        cancellation: if request.is_cancelled() {
            CancellationStatus::Cancelled
        } else {
            CancellationStatus::Active
        },
        coverage: CoverageExactness::Inexact,
        provider,
        required_proof,
        remaining_time_ms,
        remaining,
        max_automatic_transitions: 1,
    })
}

fn effort_error(error: EffortError) -> IndexError {
    match error {
        EffortError::Cancelled => IndexError::Cancelled,
        EffortError::Deadline => IndexError::ResourceLimited,
        EffortError::StaleContext => IndexError::SnapshotUnavailable,
        EffortError::InvalidConfiguration
        | EffortError::InvalidFrame
        | EffortError::InvalidProposal => IndexError::BackendViolation(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::optional_capacity_denial;
    use crate::native_resources::{NativeResourceClass, NativeResourceLedger, NativeResourcePlan};
    use quanta_index_core::index::{
        IndexError,
        effort::{ResourceNeed, ResourceScheduler},
    };

    fn require(condition: bool, message: &str) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(message.into()))
        }
    }

    #[test]
    fn optional_capacity_denial_never_hides_invalid_ledger() -> Result<(), IndexError> {
        let one = ResourceNeed {
            cpu_units: 1,
            io_bytes: 1,
            memory_bytes: 1,
            provider_units: 1,
        };
        let plan = NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 4,
                memory_bytes: 4,
                provider_units: 4,
            },
            control: one,
            interactive: one,
            core: one,
            enrichment: one,
        };
        let ledger = NativeResourceLedger::new(plan)?;
        let control = ledger.scheduler(NativeResourceClass::Control);
        let enrichment = ledger.scheduler(NativeResourceClass::Enrichment);
        optional_capacity_denial(control.denial())?;
        optional_capacity_denial(enrichment.denial())?;

        let token = control
            .try_reserve(plan.control)
            .ok_or(IndexError::ResourceLimited)?;
        // A mismatched release is a real ledger invariant failure, not an
        // ordinary capacity refusal. Both optional paths must preserve it.
        enrichment.release(token);
        require(
            control.try_reserve(plan.control).is_none(),
            "invalid control ledger admitted work",
        )?;
        require(
            enrichment.try_reserve(plan.enrichment).is_none(),
            "invalid enrichment ledger admitted work",
        )?;
        require(
            matches!(
                optional_capacity_denial(control.denial()),
                Err(IndexError::BackendViolation(_))
            ),
            "control ledger violation was treated as optional capacity denial",
        )?;
        require(
            matches!(
                optional_capacity_denial(enrichment.denial()),
                Err(IndexError::BackendViolation(_))
            ),
            "enrichment ledger violation was treated as optional capacity denial",
        )?;
        Ok(())
    }
}
