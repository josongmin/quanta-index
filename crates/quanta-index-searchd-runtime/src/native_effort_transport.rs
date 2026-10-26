//! The native UDS stream route over the retained in-process effort executor.

use std::sync::atomic::Ordering;

use quanta_index_contract::{
    index::{IndexInstanceId, ViewRef},
    native_protocol::{
        NativeEffortDone, NativeEffortPreference, NativeEffortReason, NativeIndexErrorCode,
        NativeIndexRequest, NativeIndexWireError, NativeOverlayCorpusStamp,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        IndexError, WorkBudget,
        effort::{DecisionReason, EffortPreference},
    },
};
use quanta_index_ipc::{DispatchContextV1, IpcError, IpcPlane, StreamDispatch};

use crate::{
    native::{NativeDispatcher, wire_error},
    native_effort::{
        NativeEffortObserver, NativeEffortOverlay, NativeEffortRequest, NativeEffortStage,
        NativeEffortSummary, check_original, query_effort_on_dispatcher,
    },
    native_reply::NativeReply,
    native_work_diagnostic::{self, QueryDelivery},
};

pub(crate) struct ScopedNativeEffortDone {
    result: NativeEffortDone,
    _memory: Option<quanta_index_core::index::MemoryLease>,
}
impl std::ops::Deref for ScopedNativeEffortDone {
    type Target = NativeEffortDone;
    fn deref(&self) -> &Self::Target {
        &self.result
    }
}
struct WireObserver<'a> {
    dispatcher: &'a NativeDispatcher,
    request: &'a RequestBudgetV1,
    corpus: Option<(
        quanta_index_contract::native_protocol::NativeOverlaySession,
        quanta_index_core::index::Digest,
    )>,
    emit: &'a mut dyn FnMut(NativeReply) -> Result<(), IpcError>,
    text_instance: IndexInstanceId,
    semantic_instance: IndexInstanceId,
    view: Option<ViewRef>,
    fast: bool,
    deep: bool,
    terminal_emitted: bool,
}

impl NativeEffortObserver for WireObserver<'_> {
    fn observe(&mut self, stage: NativeEffortStage<'_>) -> Result<(), IndexError> {
        match stage {
            NativeEffortStage::Fast(result) => {
                let selected_instance_changed = result.instance != self.text_instance;
                if self.fast || self.deep || selected_instance_changed {
                    return Err(IndexError::BackendViolation(
                        "native effort Fast stage order or instance changed".into(),
                    ));
                }
                (self.emit)(NativeReply::Text(result.clone()))
                    .map_err(|error| IndexError::Storage(error.to_string()))?;
                self.view = Some(result.view);
                self.fast = true;
            }
            NativeEffortStage::OverlayFast(result) => {
                let selected_instance_changed = result.instance != self.text_instance;
                if self.fast || self.deep || selected_instance_changed {
                    return Err(IndexError::BackendViolation(
                        "overlay Fast stage order or instance differs".into(),
                    ));
                }
                (self.emit)(NativeReply::OverlayText(result.clone()))
                    .map_err(|error| IndexError::Storage(error.to_string()))?;
                self.view = Some(result.view);
                self.corpus = Some((result.corpus.session, result.corpus.identity));
                self.fast = true;
            }
            NativeEffortStage::OverlayDeep(result) => {
                if !self.fast
                    || self.deep
                    || self.view != Some(result.view)
                    || result.instance != self.semantic_instance
                    || self.corpus != Some((result.corpus.session, result.corpus.identity))
                {
                    return Err(IndexError::BackendViolation(
                        "overlay Deep changed selected corpus or stage order".into(),
                    ));
                }
                (self.emit)(NativeReply::OverlaySemantic(result.clone()))
                    .map_err(|error| IndexError::Storage(error.to_string()))?;
                self.deep = true;
            }
            NativeEffortStage::Deep(result) => {
                if self.corpus.is_some()
                    || !self.fast
                    || self.deep
                    || self.view != Some(result.view)
                    || result.instance != self.semantic_instance
                {
                    return Err(IndexError::BackendViolation(
                        "native effort Deep stage order, view or instance changed".into(),
                    ));
                }
                (self.emit)(NativeReply::Semantic(result.clone()))
                    .map_err(|error| IndexError::Storage(error.to_string()))?;
                self.deep = true;
            }
        }
        Ok(())
    }

    fn finish(&mut self, summary: NativeEffortSummary<'_>) -> Result<(), IndexError> {
        let delivered_stages_changed = self.deep != summary.deep_delivered;
        if self.terminal_emitted
            || !self.fast
            || delivered_stages_changed
            || self.view != Some(summary.view)
            || self.corpus
                != summary
                    .corpus
                    .map(|corpus| (corpus.session, corpus.identity))
        {
            return Err(IndexError::BackendViolation(
                "native effort terminal differs from emitted stages".into(),
            ));
        }
        check_original(self.request)?;
        let bytes = summary
            .corpus
            .map_or(0, |corpus| corpus.revisions.len())
            .checked_mul(std::mem::size_of::<
                quanta_index_contract::native_protocol::NativeOverlayRevision,
            >())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            None
        } else {
            Some(self.dispatcher.resources.reserve_response_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            )?)
        };
        let corpus = if let Some(corpus) = summary.corpus {
            let mut revisions = Vec::new();
            revisions
                .try_reserve_exact(corpus.revisions.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            revisions.extend_from_slice(&corpus.revisions);
            Some(NativeOverlayCorpusStamp {
                session: corpus.session,
                identity: corpus.identity,
                revisions,
            })
        } else {
            None
        };
        let done = ScopedNativeEffortDone {
            result: NativeEffortDone {
                view: summary.view,
                reason: wire_reason(summary.reason),
                deep_delivered: summary.deep_delivered,
                corpus,
            },
            _memory: memory,
        };
        check_original(self.request)?;
        (self.emit)(NativeReply::EffortDone(done))
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        self.terminal_emitted = true;
        Ok(())
    }
}

pub(super) fn dispatch_stream(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    request: NativeIndexRequest,
    budget: &RequestBudgetV1,
    emit: &mut dyn FnMut(NativeReply) -> Result<(), IpcError>,
) -> StreamDispatch<NativeReply> {
    let NativeIndexRequest::QueryEffortStream {
        selection,
        text_instance,
        semantic_instance,
        text_query,
        semantic_query,
        preference,
        overlay,
    } = request
    else {
        return StreamDispatch::Unsupported;
    };
    if !matches!(context.principal, Some(peer) if peer.uid == context.owner_uid || peer.uid == 0)
        || context.plane != IpcPlane::Control
    {
        return StreamDispatch::Terminal(NativeReply::Error(NativeIndexWireError {
            code: NativeIndexErrorCode::Denied,
            message: "native effort stream requires a kernel-authenticated operator".into(),
        }));
    }
    let overlay = match overlay
        .map(|input| NativeEffortOverlay::from_dispatch(input, context))
        .transpose()
    {
        Ok(value) => value,
        Err(error) => {
            return StreamDispatch::Terminal(NativeReply::Error(wire_error(&error)));
        }
    };
    let preference = match preference {
        NativeEffortPreference::Fast => EffortPreference::Fast,
        NativeEffortPreference::Deep => EffortPreference::Deep,
        NativeEffortPreference::Auto => EffortPreference::Auto,
    };
    let Some(configuration) = dispatcher.effort.get() else {
        return unavailable("native effort policy and signal source are not configured");
    };
    let mut work = WorkBudget::with_request(dispatcher.limits, budget.clone());
    let mut observer = WireObserver {
        dispatcher,
        request: budget,
        corpus: None,
        emit,
        text_instance,
        semantic_instance,
        view: None,
        fast: false,
        deep: false,
        terminal_emitted: false,
    };
    let result = query_effort_on_dispatcher(
        dispatcher,
        NativeEffortRequest {
            selection,
            text_instance,
            semantic_instance,
            text_query,
            semantic_query,
            preference,
            overlay,
        },
        configuration.control(),
        &mut work,
        &mut observer,
    );
    if let Err(error) = &result {
        native_work_diagnostic::record_query_error_if_enabled(
            context.request_id.get(),
            context.connection_id,
            QueryDelivery {
                fast: observer.fast,
                deep: observer.deep,
                terminal: observer.terminal_emitted,
            },
            &work,
            error,
        );
    }
    if observer.terminal_emitted {
        // The terminal was sent while the selected ticket was pinned. A
        // subsequent release error cannot be turned into a second frame.
        if result.is_err() {
            context
                .diagnostic_backend_error
                .store(true, Ordering::Release);
        }
        return StreamDispatch::EmittedTerminal;
    }
    let terminal = match result {
        Ok(_) => wire_error(&IndexError::BackendViolation(
            "native effort completed without a retained terminal".into(),
        )),
        Err(error) => wire_error(&error),
    };
    StreamDispatch::Terminal(NativeReply::Error(terminal))
}

fn unavailable(message: &str) -> StreamDispatch<NativeReply> {
    StreamDispatch::Terminal(NativeReply::Error(NativeIndexWireError {
        code: NativeIndexErrorCode::CapabilityUnavailable,
        message: message.into(),
    }))
}

fn wire_reason(reason: DecisionReason) -> NativeEffortReason {
    match reason {
        DecisionReason::ExplicitFast => NativeEffortReason::ExplicitFast,
        DecisionReason::ExplicitDeep => NativeEffortReason::ExplicitDeep,
        DecisionReason::RequiredProgress => NativeEffortReason::RequiredProgress,
        DecisionReason::StableRefinement => NativeEffortReason::StableRefinement,
        DecisionReason::Pressure => NativeEffortReason::Pressure,
        DecisionReason::UnknownTelemetry => NativeEffortReason::UnknownTelemetry,
        DecisionReason::Dwell => NativeEffortReason::Dwell,
        DecisionReason::NoOpportunity => NativeEffortReason::NoOpportunity,
        DecisionReason::ResourceDenied => NativeEffortReason::ResourceDenied,
        DecisionReason::TransitionCapped => NativeEffortReason::TransitionCapped,
        DecisionReason::ProviderUnavailable => NativeEffortReason::ProviderUnavailable,
        DecisionReason::AwaitRequired => NativeEffortReason::AwaitRequired,
        DecisionReason::AwaitOptional => NativeEffortReason::AwaitOptional,
    }
}
