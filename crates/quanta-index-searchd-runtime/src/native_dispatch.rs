//! Native one-shot and progressive IPC routing over the same dispatcher.

use std::sync::Arc;

use quanta_index_contract::native_protocol::{
    NativeIndexErrorCode, NativeIndexRequest, NativeIndexWireError,
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{IndexError, WorkBudget, effort::ReservationGuard},
};
use quanta_index_ipc::{DispatchContextV1, IpcDispatcher, IpcError, IpcPlane, StreamDispatch};

use crate::native::{NativeDispatcher, wire_error};
use crate::native_reply::NativeReply;
use crate::native_work_diagnostic::{self, DiscardFailureStage};

impl NativeDispatcher {
    fn canonical_response(
        &self,
        produce: impl FnOnce() -> Result<
            quanta_index_contract::native_protocol::NativeIndexResponse,
            IndexError,
        >,
    ) -> Result<NativeReply, IndexError> {
        let slot = NativeReply::reserve_canonical(|bytes| {
            self.resources.reserve_response_memory(bytes).map(Some)
        })?;
        produce().map(|payload| slot.finish(payload))
    }
}

impl IpcDispatcher<NativeIndexRequest, NativeReply> for NativeDispatcher {
    fn reserve_response_memory(
        &self,
        budget: &RequestBudgetV1,
        bytes: u64,
    ) -> Result<Option<quanta_index_core::index::MemoryLease>, IpcError> {
        budget
            .checkpoint("native_response_admission")
            .map_err(|error| {
                IpcError::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, error))
            })?;
        let memory = self
            .resources
            .reserve_response_memory(bytes)
            .map_err(|error| {
                if error == IndexError::ResourceLimited {
                    IpcError::FrameMemoryLimited { bytes }
                } else {
                    IpcError::Io(std::io::Error::other(error))
                }
            })?;
        budget
            .checkpoint("native_response_admission")
            .map_err(|error| {
                IpcError::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, error))
            })?;
        Ok(Some(memory))
    }

    fn dispatch(
        &self,
        context: &DispatchContextV1,
        request: NativeIndexRequest,
        budget: &RequestBudgetV1,
    ) -> NativeReply {
        // Payload fields never grant operator authority. The credential came
        // from the kernel at accept, and in-process calls have no principal.
        if !matches!(context.principal, Some(peer) if peer.uid == context.owner_uid || peer.uid == 0)
            || context.plane != IpcPlane::Control
        {
            return NativeReply::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::Denied,
                message: "native index control requires a kernel-authenticated operator".into(),
            });
        }
        let result = match request {
            NativeIndexRequest::ReadSession(request) => {
                crate::native_read_session_transport::dispatch(self, context, request, budget)
            }
            interaction @ (NativeIndexRequest::InteractionBegin { .. }
            | NativeIndexRequest::InteractionActivity { .. }
            | NativeIndexRequest::InteractionRenew { .. }
            | NativeIndexRequest::InteractionEnd { .. }) => self.canonical_response(|| {
                crate::native_interaction_transport::dispatch(self, context, &interaction, budget)
            }),
            NativeIndexRequest::Overlay(request) => {
                crate::native_overlay_transport::dispatch(self, context, request, budget)
                    .map(NativeReply::Overlay)
            }
            NativeIndexRequest::Stub(request) => {
                crate::native_stub_transport::dispatch(self, context, request, budget)
                    .map(NativeReply::Stub)
            }
            NativeIndexRequest::BindOverlaySave(intent) => {
                crate::native_overlay_transport::bind_save(self, context, &intent, budget)
                    .map(NativeReply::Overlay)
            }
            NativeIndexRequest::CompleteOverlaySave(receipt) => {
                crate::native_overlay_transport::complete_save(self, context, &receipt, budget)
                    .map(NativeReply::Overlay)
            }
            NativeIndexRequest::QueryGraph {
                selection,
                instance,
                query,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                crate::native_graph_transport::query(self, selection, instance, query, &mut work)
                    .map(NativeReply::Graph)
            }
            NativeIndexRequest::QueryRepoMap {
                selection,
                instance,
                query,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                crate::native_repomap_transport::query(self, selection, instance, query, &mut work)
                    .map(NativeReply::RepoMapRanked)
            }
            NativeIndexRequest::Submit(request) => {
                let key = request.operation;
                let work = WorkBudget::with_request(self.limits, budget.clone());
                self.canonical_response(|| {
                    self.submitted_or_reconcile(
                        key,
                        self.worker.submit(Arc::clone(&self.kernel), request, work),
                    )
                })
            }
            NativeIndexRequest::Resume(key) => {
                let work = WorkBudget::with_request(self.limits, budget.clone());
                self.canonical_response(|| {
                    self.submitted_or_reconcile(
                        key,
                        self.worker.resume(Arc::clone(&self.kernel), key, work),
                    )
                })
            }
            NativeIndexRequest::LookupSourceOwners { selection, owners } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                work.checkpoint().and_then(|()| {
                    self.lookup_source_owners(selection, &owners, &mut work)
                        .map(NativeReply::SourceOwners)
                })
            }
            NativeIndexRequest::LookupProjectionOwners {
                selection,
                projection,
                owners,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                work.checkpoint().and_then(|()| {
                    self.lookup_projection_owners(selection, projection, &owners, &mut work)
                        .map(NativeReply::ProjectionOwners)
                })
            }
            NativeIndexRequest::DiffProjectionSourceOwners {
                selection,
                projection,
                producer,
                after,
                limit,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                work.checkpoint().and_then(|()| {
                    self.diff_projection_source_owners(
                        selection, projection, producer, after, limit, &mut work,
                    )
                    .map(NativeReply::ProjectionSourceDiff)
                })
            }
            NativeIndexRequest::ListProducerSourceOwners {
                selection,
                producer,
                after,
                limit,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                work.checkpoint().and_then(|()| {
                    self.list_producer_source_owners(selection, producer, after, limit, &mut work)
                        .map(NativeReply::ProducerSourceOwners)
                })
            }
            NativeIndexRequest::SearchLiteral {
                selection,
                instance,
                literal,
                after,
                limit,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                work.checkpoint().and_then(|()| {
                    self.search_literal(selection, instance, literal, after, limit, &mut work)
                        .map(NativeReply::Literal)
                })
            }
            NativeIndexRequest::QueryPaths {
                selection,
                instance,
                prefix,
                after,
                limit,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                work.checkpoint().and_then(|()| {
                    self.query_paths(selection, instance, prefix, after, limit, &mut work)
                        .map(NativeReply::Path)
                })
            }
            NativeIndexRequest::SearchOverlayText(request) => {
                crate::native_overlay_query::query(self, context, &request, budget)
                    .map(NativeReply::OverlayText)
            }
            NativeIndexRequest::SearchText {
                selection,
                instance,
                query,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                work.checkpoint().and_then(|()| {
                    self.search_text(selection, instance, &query, &mut work)
                        .map(NativeReply::Text)
                })
            }
            NativeIndexRequest::SearchSemantic {
                selection,
                instance,
                query,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                work.checkpoint().and_then(|()| {
                    self.search_semantic(selection, instance, query, &mut work)
                        .map(NativeReply::Semantic)
                })
            }
            NativeIndexRequest::QueryEffortStream { .. } => Err(IndexError::InvalidInput(
                "progressive effort requires the stream route".into(),
            )),
            other @ (NativeIndexRequest::CreateWorkspace { .. }
            | NativeIndexRequest::BeginInput(_)
            | NativeIndexRequest::StageSource { .. }
            | NativeIndexRequest::StageSources { .. }
            | NativeIndexRequest::StageProfile { .. }
            | NativeIndexRequest::StageProjectionOutput { .. }
            | NativeIndexRequest::StageProjectionOwner { .. }
            | NativeIndexRequest::StageProjectionCertificate { .. }
            | NativeIndexRequest::StageMutationPage { .. }
            | NativeIndexRequest::SealMutations { .. }
            | NativeIndexRequest::DiscardInput(_)
            | NativeIndexRequest::ReadView(_)
            | NativeIndexRequest::ReadProjection { .. }
            | NativeIndexRequest::Status(_)
            | NativeIndexRequest::ReadyWork { .. }
            | NativeIndexRequest::ClaimWork { .. }
            | NativeIndexRequest::RenewWork { .. }
            | NativeIndexRequest::WorkStatus(_)) => {
                let is_discard = matches!(&other, NativeIndexRequest::DiscardInput(_));
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                (|| {
                    work.checkpoint().inspect_err(|error| {
                        if is_discard {
                            native_work_diagnostic::record_discard_if_enabled(
                                DiscardFailureStage::RequestCheckpoint,
                                &work,
                                error,
                            );
                        }
                    })?;
                    let control = matches!(
                        &other,
                        NativeIndexRequest::Status(_)
                            | NativeIndexRequest::WorkStatus(_)
                            | NativeIndexRequest::RenewWork { .. }
                    );
                    let (scheduler, need) = if control {
                        (&self.control_scheduler, self.control_need)
                    } else {
                        (&self.interactive_scheduler, self.interactive_need)
                    };
                    let Some(reservation) =
                        ReservationGuard::try_acquire(Arc::clone(scheduler), need)
                    else {
                        let error = scheduler.denial();
                        if is_discard {
                            native_work_diagnostic::record_discard_if_enabled(
                                DiscardFailureStage::SchedulerAdmission,
                                &work,
                                &error,
                            );
                        }
                        return Err(error);
                    };
                    work.bind_memory_scope(reservation.memory_scope());
                    self.execute(other, &mut work)
                })()
            }
        };
        match result {
            Ok(response) => response,
            Err(error) => NativeReply::Error(wire_error(&error)),
        }
    }

    fn dispatch_stream(
        &self,
        context: &DispatchContextV1,
        request: NativeIndexRequest,
        budget: &RequestBudgetV1,
        emit: &mut dyn FnMut(NativeReply) -> Result<(), IpcError>,
    ) -> StreamDispatch<NativeReply> {
        crate::native_effort_transport::dispatch_stream(self, context, request, budget, emit)
    }
}
