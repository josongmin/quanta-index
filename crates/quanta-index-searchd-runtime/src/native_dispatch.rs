//! Native one-shot and progressive IPC routing over the same dispatcher.

use std::sync::Arc;

use quanta_index_contract::{
    index::{OperationStatus, SubmitOutcome},
    native_protocol::{
        NATIVE_DSL_PRODUCER_V1, NativeDslReplayBasisV1, NativeDslReplayV1, NativeIndexErrorCode,
        NativeIndexRequest, NativeIndexResponse, NativeIndexWireError, native_dsl_context_v1,
        native_dsl_sequence_v1,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{IndexError, WorkBudget, effort::ReservationGuard, ports::OperationJournalPort},
};
use quanta_index_ipc::{DispatchContextV1, IpcDispatcher, IpcError, StreamDispatch};

use crate::native::{NativeDispatcher, wire_error};
use crate::native_authorization::request_authorized;
use crate::native_reply::NativeReply;
use crate::native_work_diagnostic::{
    self, DiscardFailureStage, ReadFailureRoute, ReadFailureStage,
};

impl NativeDispatcher {
    fn replay_dsl(
        &self,
        input: NativeDslReplayV1,
        mut work: WorkBudget,
    ) -> Result<NativeIndexResponse, IndexError> {
        if input.operation.producer != NATIVE_DSL_PRODUCER_V1 {
            return Err(IndexError::InvalidInput(
                "Native DSL replay producer differs".into(),
            ));
        }
        let (status, stamp) = {
            let reservation = ReservationGuard::try_acquire(
                Arc::clone(&self.control_scheduler),
                self.control_need,
            )
            .ok_or_else(|| self.control_scheduler.denial())?;
            work.bind_memory_scope(reservation.memory_scope());
            let observed = self.catalog.projection_replay(input.operation, &mut work)?;
            work.bind_memory_scope(None);
            observed
        };
        if status == OperationStatus::Unknown {
            return self.submitted_or_reconcile(
                input.operation,
                Ok(SubmitOutcome::Unknown(input.operation)),
                None,
            );
        }
        let stamp = stamp.ok_or(IndexError::DigestConflict)?;
        let sequence = native_dsl_sequence_v1(stamp.context);
        let same_base = match input.basis {
            NativeDslReplayBasisV1::Exact {
                view,
                event_predecessor,
            } => stamp.expected_view == view && event_predecessor.checked_add(1) == Some(sequence),
            NativeDslReplayBasisV1::Source(source) => source == stamp.source,
            NativeDslReplayBasisV1::SourceWithServingSequence {
                source,
                serving_event_sequence,
            } => source == stamp.source && sequence <= serving_event_sequence,
        };
        if !same_base
            || stamp.projection != input.projection
            || sequence == 0
            || stamp.context != native_dsl_context_v1(sequence, input.authority)
        {
            return Err(IndexError::DigestConflict);
        }
        // Reuse the admitted row if recovery loses its acknowledgement.
        let (result, observed_admission) = match status {
            pending @ OperationStatus::Pending(_) => (
                self.worker
                    .resume(Arc::clone(&self.kernel), input.operation, work),
                Some(pending),
            ),
            terminal => (Ok(SubmitOutcome::Terminal(terminal)), None),
        };
        self.submitted_or_reconcile(input.operation, result, observed_admission)
    }

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
        if let NativeIndexRequest::BeginInput(operation) = &request
            && let Some(admission) = &self.process_memory_admission
            && let Err(error) = admission.admit_publication_start()
        {
            match self.catalog.reopen_ingress_if_present(*operation) {
                Ok(Some(ingress)) => {
                    return match self.canonical_response(|| {
                        Ok(
                            quanta_index_contract::native_protocol::NativeIndexResponse::InputOpened(
                                ingress,
                            ),
                        )
                    }) {
                        Ok(response) => response,
                        Err(response_error) => NativeReply::Error(wire_error(&response_error)),
                    };
                }
                Ok(None) => {
                    if error.code == NativeIndexErrorCode::ProcessRssCeilingExceeded {
                        admission.record_refusal();
                    }
                    return NativeReply::Error(error);
                }
                Err(resume_error) => return NativeReply::Error(wire_error(&resume_error)),
            }
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
            NativeIndexRequest::ReadRepoMapPriorFileV1 { view, instance, owner } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                crate::native_repomap_prior_file_transport::read(
                    self, view, instance, owner, &mut work,
                )
                .map(NativeReply::RepoMapPriorFile)
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
                        None,
                    )
                })
            }
            NativeIndexRequest::Resume(key) => {
                let work = WorkBudget::with_request(self.limits, budget.clone());
                self.canonical_response(|| {
                    self.submitted_or_reconcile(
                        key,
                        self.worker.resume(Arc::clone(&self.kernel), key, work),
                        None,
                    )
                })
            }
            NativeIndexRequest::ReplayDslV1(input) => {
                let work = WorkBudget::with_request(self.limits, budget.clone());
                self.canonical_response(|| self.replay_dsl(input, work))
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
            NativeIndexRequest::ListProducerProjectionOwnersV1 {
                view, projection, producer, after, limit,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                self.list_producer_projection_owners_v1(view, projection, producer, after, limit, &mut work)
                    .map(NativeReply::ProducerProjectionOwnersV1)
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
                    self.query_repository_paths(
                        selection, instance, prefix, after, limit, &mut work,
                    )
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
                    self.search_text(selection, instance, &query, false, &mut work, &mut stage)
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
            NativeIndexRequest::SearchTextPreview {
                selection,
                instance,
                query,
            } => {
                let mut work = WorkBudget::with_request(self.limits, budget.clone());
                let mut stage = ReadFailureStage::Checkpoint;
                let result = work.checkpoint().and_then(|()| {
                    self.search_text(selection, instance, &query, true, &mut work, &mut stage)
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
            | NativeIndexRequest::ReopenInputWithRootForSourceStageV1 { .. }
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
            | NativeIndexRequest::SealMutationsForReplayV1 { .. }
            | NativeIndexRequest::ReadSealedMutationsForReplayV1 { .. }
            | NativeIndexRequest::DiscardInput(_)
            | NativeIndexRequest::ReadView(_)
            | NativeIndexRequest::ObserveRootView(_)
            | NativeIndexRequest::ObserveCandidateInstallationV1
            | NativeIndexRequest::ObserveRepoMapGraphInstallationV1(_)
            | NativeIndexRequest::ObserveRepoMapEventFrontierV1 { .. }
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
                        | NativeIndexRequest::ObserveCandidateInstallationV1
                        | NativeIndexRequest::ObserveRepoMapGraphInstallationV1(_)
                        | NativeIndexRequest::ObserveRepoMapEventFrontierV1 { .. }
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
                            | NativeIndexRequest::ReadSealedMutationsForReplayV1 { .. }
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
    use quanta_index_ipc::{IpcPlane, PeerCredentials};

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
        let NativeIndexRequest::PublishedViewMetadata(operation) = published_metadata_request()
        else {
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
        let observer = Some(PeerCredentials {
            uid: 1001,
            gid: 100,
            pid: None,
        });
        assert!(request_authorized(
            IpcPlane::Query,
            observer,
            1000,
            &request
        ));
        assert!(request_authorized(
            IpcPlane::Control,
            observer,
            1000,
            &request
        ));
        assert!(!request_authorized(
            IpcPlane::Ingest,
            observer,
            1000,
            &request
        ));
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
        let operator = Some(PeerCredentials {
            uid: 1000,
            gid: 100,
            pid: None,
        });
        for request in [
            NativeIndexRequest::InteractionBegin { window_millis: 1 },
            NativeIndexRequest::ObserveCandidateInstallationV1,
        ] {
            assert!(!request_authorized(IpcPlane::Query, peer, 1000, &request));
            assert!(!request_authorized(IpcPlane::Control, peer, 1000, &request));
            assert!(!request_authorized(
                IpcPlane::Query,
                operator,
                1000,
                &request
            ));
            assert!(request_authorized(
                IpcPlane::Control,
                operator,
                1000,
                &request
            ));
        }
    }
}
