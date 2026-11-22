//! Assemble funded native request results before their IPC write.

use std::time::Duration;

use quanta_index_contract::{
    TextQueryRequest,
    index::{Digest, IndexInstanceId, ViewSelection},
    native_protocol::{
        NativeCandidateInstallationV1, NativeIndexRequest, NativeIndexResponse,
        NativeProjectionRead, NativeRepoMapEventFrontierV1, NativeRepoMapGraphInstallationV1,
        NativeRootScopedViewV1,
    },
};
use quanta_index_core::index::{
    IndexError, WorkBudget, owner_map_key,
    ports::{OperationInspectAccess, PersistentMapPort},
};

use super::{NativeDispatcher, native_text_result, plan_native_projected_text};
use crate::native_reply::NativeReply;
use crate::native_work_diagnostic::{self, DiscardFailureStage, ReadFailureStage};

const MAX_STAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_STAGE_PATH_BYTES: usize = quanta_index_core::index::MAX_SOURCE_PATH_BYTES;

impl NativeDispatcher {
    fn observe_held_root_identity(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<(quanta_index_core::publication_capacity::InodeKey, Digest), IndexError> {
        // This request crosses the Native UDS boundary. Preserve the failure
        // class without returning host paths or lock metadata to the peer.
        let observe_inode = |budget: &mut WorkBudget| {
            self.current_root_identity_for_view(budget)
                .map_err(|error| match error {
                    IndexError::Storage(_) => {
                        IndexError::Storage("root-scoped view identity unavailable".into())
                    }
                    other => other,
                })
        };
        let before = observe_inode(budget)?;
        let nonce = self.catalog.observe_native_root_nonce_v1(budget)?;
        let after = observe_inode(budget)?;
        if before != after || before.inode == 0 || nonce == [0; 32] {
            return Err(IndexError::DigestConflict);
        }
        budget.checkpoint()?;
        Ok((before, Digest(nonce)))
    }

    fn loaded_config_sha256(&self) -> Result<Digest, IndexError> {
        self.installation_config_sha256
            .get()
            .copied()
            .ok_or(IndexError::SnapshotUnavailable)
    }

    fn observe_root_scoped_view(
        &self,
        selection: ViewSelection,
        budget: &mut WorkBudget,
    ) -> Result<NativeRootScopedViewV1, IndexError> {
        self.with_read_view(selection, budget, |view, budget| {
            let (identity, root_nonce) = self.observe_held_root_identity(budget)?;
            Ok(NativeRootScopedViewV1 {
                view: view.clone(),
                root_nonce,
                root_device: identity.device,
                root_inode: identity.inode,
            })
        })
    }

    fn observe_candidate_installation(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<NativeCandidateInstallationV1, IndexError> {
        let config_sha256 = self.loaded_config_sha256()?;
        let (identity, root_nonce) = self.observe_held_root_identity(budget)?;
        Ok(NativeCandidateInstallationV1 {
            config_sha256,
            root_nonce,
            root_device: identity.device,
            root_inode: identity.inode,
        })
    }

    pub(crate) fn search_text(
        &self,
        selection: ViewSelection,
        instance: IndexInstanceId,
        query: &TextQueryRequest,
        with_preview: bool,
        budget: &mut WorkBudget,
        stage: &mut ReadFailureStage,
    ) -> Result<NativeReply, IndexError> {
        let (prepared, result_limit, path_projection) = {
            *stage = ReadFailureStage::ControlAdmission;
            let control = self
                .control_scheduler
                .acquire_control_for_request(self.control_need, budget)?;
            budget.bind_memory_scope(control.memory_scope());
            *stage = ReadFailureStage::QueryPreparation;
            let mut planned = plan_native_projected_text(query, |bytes, lifetime| {
                budget.reserve_memory(bytes, lifetime)
            })?;
            if with_preview {
                planned = planned.with_preview();
            }
            let result_limit = planned.result_limit()?;
            let path_projection =
                planned.plan.filters.surface == quanta_index_lexical::symbol::ResultSurface::Path;
            let prepared = self.kernel.prepare_query(
                &self.text_queries,
                selection,
                instance,
                planned,
                budget,
            )?;
            (prepared, result_limit, path_projection)
        };
        let need = prepared.required_resources();
        *stage = ReadFailureStage::InteractiveAdmission;
        let reservation = match self
            .interactive_scheduler
            .acquire_interactive_for_request(need, prepared.admission_budget())
        {
            Ok(reservation) => reservation,
            Err(error) => {
                prepared.abort()?;
                return Err(error);
            }
        };
        *stage = ReadFailureStage::QueryExecution;
        let outcome = prepared.execute(&reservation)?;
        *stage = ReadFailureStage::ResultConversion;
        let reserve = |bytes, lifetime| budget.reserve_memory(bytes, lifetime);
        let result = match (path_projection, with_preview) {
            (true, _) => crate::native_text_result::convert_paths(
                outcome,
                instance,
                result_limit,
                with_preview,
                reserve,
            )
            .map(NativeReply::TextPaths)?,
            (false, true) => {
                crate::native_text_result::convert_preview(outcome, instance, result_limit, reserve)
                    .map(NativeReply::TextPreview)?
            }
            (false, false) => native_text_result(outcome, instance, result_limit, reserve)
                .map(NativeReply::Text)?,
        };
        *stage = ReadFailureStage::Checkpoint;
        budget.checkpoint()?;
        Ok(result)
    }

    pub(crate) fn execute(
        &self,
        request: NativeIndexRequest,
        budget: &mut WorkBudget,
    ) -> Result<NativeReply, IndexError> {
        let is_discard = matches!(&request, NativeIndexRequest::DiscardInput(_));
        budget.checkpoint().inspect_err(|error| {
            if is_discard {
                native_work_diagnostic::record_discard_if_enabled(
                    DiscardFailureStage::ExecuteCheckpoint,
                    budget,
                    error,
                );
            }
        })?;
        // Staged references already own their retained backing grant.
        let request = match request {
            NativeIndexRequest::StageSources { ingress, inputs } => {
                return self
                    .kernel
                    .stage_sources(ingress, inputs, budget)
                    .map(NativeReply::StagedSources);
            }
            NativeIndexRequest::StageRepositorySources {
                ingress,
                repository,
                inputs,
            } => {
                return self
                    .kernel
                    .stage_repository_sources(ingress, &repository, inputs, budget)
                    .map(NativeReply::StagedSources);
            }
            request => request,
        };
        let reply_slot = NativeReply::reserve_canonical(|bytes| {
            budget.reserve_memory(bytes, quanta_index_core::index::MemoryLifetime::Retained)
        })
        .inspect_err(|error| {
            if is_discard {
                native_work_diagnostic::record_discard_if_enabled(
                    DiscardFailureStage::ReplyReservation,
                    budget,
                    error,
                );
            }
        })?;
        let result = match request {
            NativeIndexRequest::SealCandidateWithSourceRoutes {
                primary,
                ingress,
                routes,
            } => self.seal_source_routes_v1(primary, ingress, routes, budget),
            NativeIndexRequest::ResolveProtectedSourceHead {
                repo_id,
                revision_id,
            } => self.resolve_protected_source_head_v1(&repo_id, &revision_id, budget),
            NativeIndexRequest::ResolveProtectedRepoMapHead {
                repo_id,
                revision_id,
            } => self.resolve_protected_repomap_head_v1(&repo_id, &revision_id, budget),
            NativeIndexRequest::ReadImportedRepoMapMember { view, instance } => {
                self.read_imported_repomap_member_v1(view, instance, budget)
            }
            NativeIndexRequest::CreateWorkspace { workspace, epoch } => self
                .kernel
                .create_workspace(workspace, epoch, budget)
                .map(NativeIndexResponse::Created),
            NativeIndexRequest::BeginInput(key) => self
                .kernel
                .begin_input(key)
                .map(NativeIndexResponse::InputOpened),
            NativeIndexRequest::ReopenInputWithRootForSourceStageV1 { operation, root } => self
                .catalog
                .reopen_source_stage_ingress_v1(operation, root, budget)
                .map(NativeIndexResponse::InputOpened),
            NativeIndexRequest::StageSource {
                ingress,
                path,
                bytes,
                text,
            } => {
                if path.is_empty()
                    || path.len() > MAX_STAGE_PATH_BYTES
                    || bytes.len() > MAX_STAGE_BYTES
                    || path.starts_with('/')
                    || path.contains('\0')
                    || path
                        .split('/')
                        .any(|part| part.is_empty() || part == "." || part == "..")
                {
                    return Err(IndexError::InvalidInput(
                        "source stage exceeds native bounds".into(),
                    ));
                }
                let reference = self
                    .kernel
                    .stage_source_text(ingress, &path, &bytes, text, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProfile { ingress, profile } => {
                let reference = self.kernel.stage_profile(ingress, &profile, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProjectionOutput {
                ingress,
                bytes,
                children,
            } => {
                if bytes.len() > MAX_STAGE_BYTES {
                    return Err(IndexError::ResourceLimited);
                }
                let reference = self.kernel.stage_projection_output_with_children(
                    ingress,
                    &bytes,
                    children.as_slice(),
                    budget,
                )?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageImmutableObject {
                ingress,
                bytes,
                children,
            } => {
                let wire_bytes = u64::try_from(bytes.len())
                    .ok()
                    .and_then(|payload| payload.checked_add(16))
                    .and_then(|payload| {
                        payload.checked_add(
                            u64::try_from(children.as_slice().len())
                                .ok()?
                                .checked_mul(32)?,
                        )
                    })
                    .ok_or(IndexError::ResourceLimited)?;
                if wire_bytes > quanta_index_contract::native_protocol::MAX_NATIVE_IMMUTABLE_OBJECT_WIRE_BYTES_V1 {
                    return Err(IndexError::ResourceLimited);
                }
                let reference = self.kernel.stage_immutable_object_v1(
                    ingress,
                    &bytes,
                    children.as_slice(),
                    budget,
                )?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProjectionOwner {
                ingress,
                contribution,
            } => {
                let ticket = self.kernel.pin_view(
                    ViewSelection::Latest {
                        workspace: ingress.operation.workspace,
                        epoch: ingress.operation.epoch,
                    },
                    budget,
                )?;
                ticket.validate()?;
                if contribution.owner.producer != contribution.producer {
                    return Err(IndexError::InvalidInput(
                        "projection owner must belong to its producer".into(),
                    ));
                }
                match (contribution.source_owner, contribution.source) {
                    (Some(source_owner), Some(source)) => {
                        let expected = self.objects.lookup_object(
                            ticket.view().source.owners,
                            &owner_map_key(source_owner),
                            budget,
                        )?;
                        if expected != Some(source) {
                            return Err(IndexError::Conflict);
                        }
                    }
                    (None, None) => {}
                    _ => {
                        return Err(IndexError::InvalidInput(
                            "projection source owner and record must be paired".into(),
                        ));
                    }
                }
                let reference =
                    self.kernel
                        .stage_projection_owner(ingress, contribution, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageProjectionCertificate {
                ingress,
                certificate,
            } => {
                let ticket = self.kernel.pin_view(
                    ViewSelection::Latest {
                        workspace: ingress.operation.workspace,
                        epoch: ingress.operation.epoch,
                    },
                    budget,
                )?;
                ticket.validate()?;
                if certificate.target_source != ticket.view().source {
                    return Err(IndexError::Conflict);
                }
                let reference =
                    self.kernel
                        .stage_projection_certificate(ingress, &certificate, budget)?;
                Ok(NativeIndexResponse::Staged(reference))
            }
            NativeIndexRequest::StageMutationPage {
                ingress,
                base,
                mutations,
            } => {
                if mutations.is_empty() || mutations.len() > 128 {
                    return Err(IndexError::ResourceLimited);
                }
                self.kernel
                    .stage_mutation_page(ingress, base, &mutations, budget)
                    .map(NativeIndexResponse::MutationPageStaged)
            }
            NativeIndexRequest::SealMutations {
                ingress,
                root,
                count,
            } => self
                .kernel
                .seal_mutations(ingress, root, count, budget)
                .map(NativeIndexResponse::MutationsStaged),
            NativeIndexRequest::SealMutationsForReplayV1 {
                ingress,
                root,
                count,
                context,
            } => self
                .kernel
                .seal_mutations_for_replay_v1(ingress, root, count, context, budget)
                .map(NativeIndexResponse::MutationsSealedForReplayV1),
            NativeIndexRequest::ReadSealedMutationsForReplayV1 {
                operation,
                correlation,
            } => self
                .kernel
                .read_sealed_mutations_for_replay_v1(operation, correlation, budget)
                .map(NativeIndexResponse::SealedMutationsForReplayV1),
            NativeIndexRequest::DiscardInput(ingress) => {
                let _physical = self.prepare_discard(budget)?;
                self.kernel
                    .discard_input(ingress, budget)
                    .inspect_err(|error| {
                        native_work_diagnostic::record_discard_if_enabled(
                            DiscardFailureStage::CatalogDiscard,
                            budget,
                            error,
                        );
                        native_work_diagnostic::record_discard_physical_if_enabled(
                            self.catalog.as_ref(),
                            self.physical.as_ref(),
                        );
                    })?;
                Ok(NativeIndexResponse::InputDiscarded(ingress))
            }
            NativeIndexRequest::StageSources { .. }
            | NativeIndexRequest::StageRepositorySources { .. }
            | NativeIndexRequest::Overlay(_)
            | NativeIndexRequest::Stub(_)
            | NativeIndexRequest::BindOverlaySave(_)
            | NativeIndexRequest::CompleteOverlaySave(_)
            | NativeIndexRequest::QueryGraph { .. }
            | NativeIndexRequest::ReadRepoMapPriorFileV1 { .. }
            | NativeIndexRequest::QueryImportedRepoMap { .. }
            | NativeIndexRequest::QueryRepoMap { .. }
            | NativeIndexRequest::LookupSourceOwners { .. }
            | NativeIndexRequest::LookupProjectionOwners { .. }
            | NativeIndexRequest::ListProducerProjectionOwnersV1 { .. }
            | NativeIndexRequest::ListProducerSourceOwners { .. }
            | NativeIndexRequest::DiffProjectionSourceOwners { .. }
            | NativeIndexRequest::SearchLiteral { .. }
            | NativeIndexRequest::QueryPaths { .. }
            | NativeIndexRequest::QueryRepositoryPaths { .. }
            | NativeIndexRequest::SearchOverlayText(_)
            | NativeIndexRequest::ReadSession(_)
            | NativeIndexRequest::SearchText { .. }
            | NativeIndexRequest::SearchTextPreview { .. }
            | NativeIndexRequest::SearchSemantic { .. }
            | NativeIndexRequest::QueryEffortStream { .. } => Err(IndexError::BackendViolation(
                "native query bypassed selected reader admission".into(),
            )),
            NativeIndexRequest::Submit(_)
            | NativeIndexRequest::Resume(_)
            | NativeIndexRequest::ReplayDslV1(_) => Err(IndexError::BackendViolation(
                "async native operation routed to sync path".into(),
            )),
            NativeIndexRequest::PublishedViewMetadata(key) => self
                .kernel
                .published_view_metadata(key, budget)
                .map(|payload| {
                    NativeIndexResponse::PublishedViewMetadata(
                        quanta_index_core::index::OperationReply {
                            operation: key,
                            payload,
                        },
                    )
                }),
            NativeIndexRequest::Status(key) => self
                .kernel
                .status(key, OperationInspectAccess::Request(budget))
                .map(|payload| {
                    NativeIndexResponse::Status(quanta_index_core::index::OperationReply {
                        operation: key,
                        payload,
                    })
                }),
            NativeIndexRequest::ReadProjection {
                selection,
                projection,
            } => {
                let (view, binding) = self
                    .kernel
                    .read_projection_binding(selection, projection, budget)?;
                Ok(NativeIndexResponse::Projection(NativeProjectionRead {
                    view,
                    binding,
                }))
            }
            NativeIndexRequest::ReadView(selection) => self
                .with_read_view(selection, budget, |view, _budget| Ok(view.clone()))
                .map(NativeIndexResponse::View),
            NativeIndexRequest::ObserveRootView(selection) => self
                .observe_root_scoped_view(selection, budget)
                .map(NativeIndexResponse::RootScopedView),
            NativeIndexRequest::ObserveCandidateInstallationV1 => self
                .observe_candidate_installation(budget)
                .map(NativeIndexResponse::CandidateInstallationV1),
            NativeIndexRequest::ObserveRepoMapGraphInstallationV1(selection) => {
                let &descriptor = self
                    .repomap_graph_installation
                    .get()
                    .ok_or(IndexError::SnapshotUnavailable)?;
                let config_sha256 = self.loaded_config_sha256()?;
                let root = self.observe_root_scoped_view(selection, budget)?;
                Ok(NativeIndexResponse::RepoMapGraphInstallationV1(
                    NativeRepoMapGraphInstallationV1 {
                        root,
                        config_sha256,
                        descriptor,
                    },
                ))
            }
            NativeIndexRequest::ObserveRepoMapEventFrontierV1 {
                view,
                producer,
                stream,
            } => {
                if self.repomap_graph_installation.get().is_none() {
                    return Err(IndexError::SnapshotUnavailable);
                }
                let root = self.observe_root_scoped_view(ViewSelection::Exact(view), budget)?;
                let cursor = self
                    .catalog
                    .observe_native_event_frontier_v1(view, producer, stream, budget)?;
                Ok(NativeIndexResponse::RepoMapEventFrontierV1(
                    NativeRepoMapEventFrontierV1 {
                        view: root.view.identity,
                        root_nonce: root.root_nonce,
                        producer,
                        stream,
                        cursor,
                    },
                ))
            }
            NativeIndexRequest::ReadyWork {
                workspace,
                epoch,
                limit,
            } => {
                if limit == 0 || limit > 1024 {
                    return Err(IndexError::ResourceLimited);
                }
                self.kernel
                    .ready_work(workspace, epoch, limit, budget)
                    .map(NativeIndexResponse::ReadyWork)
            }
            NativeIndexRequest::ClaimWork {
                intent,
                lease_millis,
            } => {
                if lease_millis == 0 || lease_millis > 600_000 {
                    return Err(IndexError::InvalidInput(
                        "work lease outside 1..=600000 ms".into(),
                    ));
                }
                self.kernel
                    .claim_work(&intent, Duration::from_millis(lease_millis), budget)
                    .map(NativeIndexResponse::Claimed)
            }
            NativeIndexRequest::RenewWork {
                claim,
                lease_millis,
            } => {
                if lease_millis == 0 || lease_millis > 600_000 {
                    return Err(IndexError::InvalidInput(
                        "work lease outside 1..=600000 ms".into(),
                    ));
                }
                self.kernel
                    .renew_work(&claim, Duration::from_millis(lease_millis), budget)
                    .map(NativeIndexResponse::Renewed)
            }
            NativeIndexRequest::WorkStatus(intent) => self
                .kernel
                .work_status(&intent, budget)
                .map(NativeIndexResponse::WorkStatus),
            NativeIndexRequest::InteractionBegin { .. }
            | NativeIndexRequest::InteractionActivity { .. }
            | NativeIndexRequest::InteractionRenew { .. }
            | NativeIndexRequest::InteractionEnd { .. } => Err(IndexError::InvalidInput(
                "interaction publication requires a kernel-authenticated IPC principal".into(),
            )),
        };
        result.map(|payload| reply_slot.finish(payload))
    }
}
