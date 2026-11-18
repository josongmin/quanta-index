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
use crate::native_work_diagnostic::{
    self, DiscardFailureStage, ReadFailureRoute, ReadFailureStage,
};

pub(crate) fn request_authorized(
    plane: IpcPlane,
    principal: Option<quanta_index_ipc::PeerCredentials>,
    owner_uid: u32,
    request: &NativeIndexRequest,
) -> bool {
    let Some(peer) = principal else {
        return false;
    };
    match request {
        NativeIndexRequest::ReadSession(_)
        | NativeIndexRequest::ReadView(_)
        | NativeIndexRequest::ObserveRootView(_)
        | NativeIndexRequest::ResolveProtectedSourceHead { .. }
            | NativeIndexRequest::ResolveProtectedRepoMapHead { .. }
            | NativeIndexRequest::ReadImportedRepoMapMember { .. }
        | NativeIndexRequest::PublishedViewMetadata(_)
        | NativeIndexRequest::ReadProjection { .. }
        | NativeIndexRequest::LookupSourceOwners { .. }
        | NativeIndexRequest::ListProducerSourceOwners { .. }
        | NativeIndexRequest::LookupProjectionOwners { .. }
        | NativeIndexRequest::DiffProjectionSourceOwners { .. }
        | NativeIndexRequest::SearchLiteral { .. }
        | NativeIndexRequest::QueryPaths { .. }
        | NativeIndexRequest::QueryRepositoryPaths { .. }
        | NativeIndexRequest::SearchText { .. }
        | NativeIndexRequest::SearchSemantic { .. }
        | NativeIndexRequest::QueryGraph { .. }
        | NativeIndexRequest::QueryImportedRepoMap { .. }
        | NativeIndexRequest::QueryRepoMap { .. }
        | NativeIndexRequest::QueryEffortStream { .. } => {
            matches!(plane, IpcPlane::Query | IpcPlane::Control)
        }
        _ => plane == IpcPlane::Control && (peer.uid == owner_uid || peer.uid == 0),
    }
}

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
        if !request_authorized(
            context.plane,
            context.principal,
            context.owner_uid,
            &request,
        ) {
            return NativeReply::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::Denied,
                message:
                    "native operation is not authorized for this authenticated principal and plane"
                        .into(),
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
            NativeIndexRequest::QueryImportedRepoMap {
                selection,
                instance,
                query,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                crate::native_imported_repomap_transport::query(
                    self, selection, instance, query, &mut work,
                )
                .map(NativeReply::ImportedRepoMap)
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
            NativeIndexRequest::QueryRepositoryPaths {
                selection,
                instance,
                prefix,
                after,
                limit,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                work.checkpoint().and_then(|()| {
                    self.query_repository_paths(selection, instance, prefix, after, limit, &mut work)
                        .map(NativeReply::RepositoryPath)
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
                let mut stage = ReadFailureStage::Checkpoint;
                let result = work.checkpoint().and_then(|()| {
                    self.search_text(selection, instance, &query, &mut work, &mut stage)
                        .map(NativeReply::Text)
                });
                if let Err(error) = &result {
                    native_work_diagnostic::record_read_error_if_enabled(
                        context,
                        ReadFailureRoute::OrdinarySearch,
                        stage,
                        &work,
                        error,
                    );
                }
                result
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
            | NativeIndexRequest::SealCandidateWithSourceRoutes { .. }
            | NativeIndexRequest::BeginInput(_)
            | NativeIndexRequest::StageSource { .. }
            | NativeIndexRequest::StageSources { .. }
            | NativeIndexRequest::StageRepositorySources { .. }
            | NativeIndexRequest::StageProfile { .. }
            | NativeIndexRequest::StageProjectionOutput { .. }
            | NativeIndexRequest::StageImmutableObject { .. }
            | NativeIndexRequest::StageProjectionOwner { .. }
            | NativeIndexRequest::StageProjectionCertificate { .. }
            | NativeIndexRequest::StageMutationPage { .. }
            | NativeIndexRequest::SealMutations { .. }
            | NativeIndexRequest::DiscardInput(_)
            | NativeIndexRequest::ReadView(_)
            | NativeIndexRequest::ObserveRootView(_)
            | NativeIndexRequest::ResolveProtectedSourceHead { .. }
            | NativeIndexRequest::ResolveProtectedRepoMapHead { .. }
            | NativeIndexRequest::ReadImportedRepoMapMember { .. }
            | NativeIndexRequest::PublishedViewMetadata(_)
            | NativeIndexRequest::ReadProjection { .. }
            | NativeIndexRequest::Status(_)
            | NativeIndexRequest::ReadyWork { .. }
            | NativeIndexRequest::ClaimWork { .. }
            | NativeIndexRequest::RenewWork { .. }
            | NativeIndexRequest::WorkStatus(_)) => {
                let is_discard = matches!(&other, NativeIndexRequest::DiscardInput(_));
                let is_read_view = matches!(
                    &other,
                    NativeIndexRequest::ReadView(_)
                        | NativeIndexRequest::ObserveRootView(_)
                        | NativeIndexRequest::ResolveProtectedSourceHead { .. }
            | NativeIndexRequest::ResolveProtectedRepoMapHead { .. }
            | NativeIndexRequest::ReadImportedRepoMapMember { .. }
                );
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                let mut read_view_stage = ReadFailureStage::Checkpoint;
                let result = (|| {
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
                    read_view_stage = ReadFailureStage::InteractiveAdmission;
                    let admission = if control {
                        ReservationGuard::try_acquire(
                            Arc::clone(&self.control_scheduler),
                            self.control_need,
                        )
                        .ok_or_else(|| self.control_scheduler.denial())
                    } else {
                        self.interactive_scheduler
                            .acquire_interactive_for_request(self.interactive_need, &work)
                    };
                    let reservation = admission.inspect_err(|error| {
                        if is_discard {
                            native_work_diagnostic::record_discard_if_enabled(
                                DiscardFailureStage::SchedulerAdmission,
                                &work,
                                error,
                            );
                        }
                    })?;
                    work.bind_memory_scope(reservation.memory_scope());
                    read_view_stage = ReadFailureStage::ViewCustody;
                    self.execute(other, &mut work)
                })();
                if is_read_view && let Err(error) = &result {
                    native_work_diagnostic::record_read_error_if_enabled(
                        context,
                        ReadFailureRoute::ReadView,
                        read_view_stage,
                        &work,
                        error,
                    );
                }
                result
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

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::native_protocol::{NativeReadSessionId, NativeReadSessionRequest};
    use quanta_index_ipc::PeerCredentials;

    fn published_metadata_request() -> NativeIndexRequest {
        use quanta_index_contract::index::{
            CollectionId, OperationKey, ProducerId, SlotId, StreamId, TenantId, WorkspaceKey,
            WorktreeId,
        };
        NativeIndexRequest::PublishedViewMetadata(OperationKey {
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
            sequence: 7,
        })
    }

    #[test]
    fn repository_path_request_uses_authenticated_read_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        let NativeIndexRequest::PublishedViewMetadata(operation) = published_metadata_request() else {
            return Err("invalid operation fixture".into());
        };
        let request = NativeIndexRequest::QueryRepositoryPaths {
            selection: quanta_index_contract::index::ViewSelection::Latest {
                workspace: operation.workspace,
                epoch: operation.epoch,
            },
            instance: quanta_index_contract::index::IndexInstanceId([7; 16]),
            prefix: String::new(),
            after: None,
            limit: 1,
        };
        let observer = Some(PeerCredentials { uid: 1001, gid: 100, pid: None });
        assert!(request_authorized(IpcPlane::Query, observer, 1000, &request));
        assert!(request_authorized(IpcPlane::Control, observer, 1000, &request));
        assert!(!request_authorized(IpcPlane::Ingest, observer, 1000, &request));
        assert!(!request_authorized(IpcPlane::Query, None, 1000, &request));
        Ok(())
    }

    #[test]
    fn admitted_observer_can_read_session_on_the_same_control_socket() {
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::Status(
            NativeReadSessionId([7; 32]),
        ));
        let peer = Some(PeerCredentials {
            uid: 1001,
            gid: 100,
            pid: None,
        });
        for request in [request, published_metadata_request()] {
            assert!(request_authorized(IpcPlane::Control, peer, 1000, &request));
            assert!(!request_authorized(IpcPlane::Ingest, peer, 1000, &request));
            assert!(!request_authorized(IpcPlane::Query, None, 1000, &request));
        }
    }

    #[test]
    fn admitted_observer_can_query_without_receiving_operator_authority() {
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::Status(
            NativeReadSessionId([7; 32]),
        ));
        let peer = Some(PeerCredentials {
            uid: 1001,
            gid: 100,
            pid: None,
        });
        for request in [request, published_metadata_request()] {
            assert!(request_authorized(IpcPlane::Query, peer, 1000, &request));
        }
        let write = NativeIndexRequest::InteractionBegin { window_millis: 1 };
        assert!(!request_authorized(IpcPlane::Query, peer, 1000, &write));
        assert!(!request_authorized(IpcPlane::Control, peer, 1000, &write));
        let operator = Some(PeerCredentials {
            uid: 1000,
            gid: 100,
            pid: None,
        });
        assert!(!request_authorized(IpcPlane::Query, operator, 1000, &write));
        assert!(request_authorized(
            IpcPlane::Control,
            operator,
            1000,
            &write
        ));
    }
}
