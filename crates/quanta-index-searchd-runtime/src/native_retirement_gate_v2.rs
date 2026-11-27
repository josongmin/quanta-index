//! Request admission under the signed, BOOT-verified retirement capability.

use super::{NativeDispatcher, NativeRetirementAuthorityV2};
use quanta_index_contract::{
    index::{
        IndexInstanceId, IndexProfile, InputDomain, ObjectRef, OperationKey, OperationStatus,
        ProjectionId, SourceOperationMode, SubmitOutcome, UpdateKind, UpdateRequest, ViewSelection,
        WorkIntent, WorkScope,
    },
    native_protocol::{NativeIndexRequest, NativeIndexResponse},
};
use quanta_index_core::index::{
    IndexError, OWNED_WORK_PRODUCER, WorkBudget,
    ports::{ImmutableObjectPort, OperationInspectAccess, OperationJournalPort},
};

fn retired() -> IndexError {
    IndexError::InvalidInput("retired projection or instance identity".into())
}

fn checked_resume_status_v2(
    observed: OperationStatus,
    budget: &mut WorkBudget,
    check_pending: impl FnOnce(&mut WorkBudget) -> Result<(), IndexError>,
    status_after_error: impl FnOnce(&mut WorkBudget) -> Result<OperationStatus, IndexError>,
) -> Result<Option<OperationStatus>, IndexError> {
    if !matches!(observed, OperationStatus::Pending(_)) {
        return Ok(Some(observed));
    }
    match check_pending(budget) {
        Ok(()) => Ok(None),
        Err(error) => {
            let latest = status_after_error(budget)?;
            if matches!(
                latest,
                OperationStatus::Published(_) | OperationStatus::Refused(_)
            ) {
                Ok(Some(latest))
            } else {
                Err(error)
            }
        }
    }
}

impl NativeRetirementAuthorityV2 {
    fn reject_projection(&self, projection: ProjectionId) -> Result<(), IndexError> {
        match self
            .retired_projections
            .binary_search_by_key(&projection.0, |value| value.0)
        {
            Ok(_position) => Err(retired()),
            Err(_insertion_point) => Ok(()),
        }
    }

    fn reject_instance(&self, instance: IndexInstanceId) -> Result<(), IndexError> {
        match self
            .retired_instances
            .binary_search_by_key(&instance.0, |value| value.0)
        {
            Ok(_position) => Err(retired()),
            Err(_insertion_point) => Ok(()),
        }
    }

    fn check_profile(
        &self,
        profile: &IndexProfile,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        for spec in &profile.instances {
            budget.checkpoint()?;
            self.reject_instance(spec.instance)?;
            if let InputDomain::Projection { projection, .. } = spec.input_domain {
                self.reject_projection(projection)?;
            }
        }
        Ok(())
    }

    pub(super) fn check_work_intent(&self, intent: &WorkIntent) -> Result<(), IndexError> {
        self.reject_instance(intent.instance)?;
        if let WorkScope::ProjectionDomain { projection, .. } = intent.scope {
            self.reject_projection(projection)?;
        }
        Ok(())
    }
}

impl NativeDispatcher {
    fn check_retired_resume_body_v2(
        &self,
        retirement: &NativeRetirementAuthorityV2,
        operation: OperationKey,
        request: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let object = self.objects.get(request, budget)?;
        if object.registration.reference != request {
            return Err(IndexError::DigestConflict);
        }
        quanta_index_kernel::with_decoded_update_request_v1(
            &object.payload,
            budget,
            |update, budget| {
                if update.operation != operation {
                    return Err(IndexError::DigestConflict);
                }
                self.check_update_v2(retirement, update, budget)
            },
        )??;
        Ok(())
    }

    fn check_retired_terminal_view_v2(
        &self,
        retirement: &NativeRetirementAuthorityV2,
        operation: quanta_index_contract::index::OperationKey,
        view: quanta_index_contract::index::ViewRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let published = self.kernel.published_view_metadata(operation, budget)?;
        if published.identity != view {
            return Err(IndexError::DigestConflict);
        }
        self.with_read_view(ViewSelection::Exact(view), budget, |pinned, budget| {
            if pinned != &published {
                return Err(IndexError::DigestConflict);
            }
            quanta_index_kernel::with_loaded_profile_v1(
                pinned.profile,
                self.objects.as_ref(),
                budget,
                |profile, budget| retirement.check_profile(profile, budget),
            )??;
            for projection in &retirement.retired_projections {
                budget.checkpoint()?;
                let (observed, binding) = self.kernel.read_projection_binding(
                    ViewSelection::Exact(view),
                    *projection,
                    budget,
                )?;
                if observed != view {
                    return Err(IndexError::DigestConflict);
                }
                if binding.is_some() {
                    return Err(retired());
                }
            }
            Ok(())
        })
    }

    fn check_update_v2(
        &self,
        retirement: &NativeRetirementAuthorityV2,
        request: &UpdateRequest,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        match &request.kind {
            UpdateKind::ProjectionAdvance(transition) => {
                retirement.reject_projection(transition.projection)?;
                if let Some(previous) = transition.expected {
                    retirement.reject_projection(previous.projection)?;
                }
            }
            UpdateKind::CompactSourceLocal { instance, .. } => {
                retirement.reject_instance(*instance)?;
            }
            UpdateKind::Materialize { claim } => {
                retirement.check_work_intent(&claim.intent)?;
            }
            UpdateKind::Reconfigure { profile } => {
                quanta_index_kernel::with_loaded_profile_v1(
                    *profile,
                    self.objects.as_ref(),
                    budget,
                    |loaded, budget| retirement.check_profile(loaded, budget),
                )??;
            }
            UpdateKind::HistoryAdvance(transition) => {
                retirement.reject_instance(transition.instance)?;
                quanta_index_kernel::with_loaded_profile_v1(
                    transition.profile,
                    self.objects.as_ref(),
                    budget,
                    |loaded, budget| retirement.check_profile(loaded, budget),
                )??;
            }
            UpdateKind::HistoryAppend(append) => {
                retirement.reject_instance(append.instance)?;
                quanta_index_kernel::with_loaded_profile_v1(
                    append.profile,
                    self.objects.as_ref(),
                    budget,
                    |loaded, budget| retirement.check_profile(loaded, budget),
                )??;
            }
            UpdateKind::SourceHistoryAdvance(transition)
            | UpdateKind::SourceHistoryResync { transition, .. } => {
                retirement.reject_instance(transition.history.prior_instance)?;
                retirement.reject_instance(transition.history.instance)?;
                quanta_index_kernel::with_loaded_profile_v1(
                    transition.history.profile,
                    self.objects.as_ref(),
                    budget,
                    |loaded, budget| retirement.check_profile(loaded, budget),
                )??;
            }
            UpdateKind::SourceHistoryBootstrap(transition) => {
                retirement.reject_instance(transition.history.instance)?;
                quanta_index_kernel::with_loaded_profile_v1(
                    transition.history.profile,
                    self.objects.as_ref(),
                    budget,
                    |loaded, budget| retirement.check_profile(loaded, budget),
                )??;
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn check_retired_request_v2(
        &self,
        retirement: &NativeRetirementAuthorityV2,
        request: &NativeIndexRequest,
        budget: &mut WorkBudget,
    ) -> Result<Option<NativeIndexResponse>, IndexError> {
        // The unused-looking capability retains the signed installation's
        // rotation lock throughout this listener's lifetime.
        let _signed = &retirement.approval;
        let _policy_ref = retirement.policy_ref;
        budget.checkpoint()?;
        match request {
            NativeIndexRequest::QueryGraph { instance, .. }
            | NativeIndexRequest::QueryPaths { instance, .. }
            | NativeIndexRequest::QueryRepositoryPaths { instance, .. } => {
                retirement.reject_instance(*instance)?;
            }
            NativeIndexRequest::ReadProjection { projection, .. }
            | NativeIndexRequest::LookupProjectionOwners { projection, .. }
            | NativeIndexRequest::ListProducerProjectionOwnersV1 { projection, .. }
            | NativeIndexRequest::DiffProjectionSourceOwners { projection, .. } => {
                retirement.reject_projection(*projection)?;
            }
            NativeIndexRequest::StageProfile { profile, .. } => {
                retirement.check_profile(profile, budget)?
            }
            NativeIndexRequest::StageProjectionOwner { contribution, .. } => {
                retirement.reject_projection(contribution.projection)?;
            }
            NativeIndexRequest::StageProjectionCertificate { certificate, .. } => {
                retirement.reject_projection(certificate.projection)?;
                if let Some(previous) = certificate.prior {
                    retirement.reject_projection(previous.projection)?;
                }
            }
            NativeIndexRequest::Submit(update)
            | NativeIndexRequest::SubmitStagedSourceV1 {
                request: update, ..
            }
            | NativeIndexRequest::SealStagedDslRequestV1 {
                request: update, ..
            } => {
                self.check_update_v2(retirement, update, budget)?;
            }
            NativeIndexRequest::Resume(operation) => {
                if operation.producer == OWNED_WORK_PRODUCER {
                    return Err(IndexError::InvalidInput(
                        "reserved owned work producer".into(),
                    ));
                }
                let observed = self
                    .kernel
                    .status(*operation, OperationInspectAccess::Request(budget))?;
                let status = checked_resume_status_v2(
                    observed,
                    budget,
                    |budget| {
                        let saved = self.catalog.operation_request(*operation, budget)?;
                        if saved.key != *operation {
                            return Err(IndexError::DigestConflict);
                        }
                        self.check_retired_resume_body_v2(
                            retirement,
                            *operation,
                            saved.request,
                            budget,
                        )
                    },
                    |budget| {
                        self.kernel
                            .status(*operation, OperationInspectAccess::Request(budget))
                    },
                )?;
                let Some(status) = status else {
                    // Pending custody was checked; let the ordinary Resume
                    // path settle it without another request-body read.
                    budget.checkpoint()?;
                    return Ok(None);
                };
                if status == OperationStatus::Unknown {
                    // Settle the observation here. Dispatching again could pick up
                    // a concurrent admission that this check never inspected.
                    return self
                        .submitted_or_reconcile(
                            *operation,
                            Ok(SubmitOutcome::Unknown(*operation)),
                            None,
                        )
                        .map(Some);
                }
                let saved = self.catalog.operation_request(*operation, budget)?;
                if saved.key != *operation {
                    return Err(IndexError::DigestConflict);
                }
                // Advance and Resync are durable request classes issued only
                // by Source and SourceHistory operations. A terminal result
                // cannot execute again, and its request bytes may have been
                // reclaimed. Inspect the exact published view before replying.
                let retained_history_instance = if saved.source_mode
                    == SourceOperationMode::Unchanged
                {
                    self.catalog
                        .authenticated_terminal_history_request_v1(*operation, &status, budget)?
                } else {
                    None
                };
                if let Some(instance) = retained_history_instance {
                    retirement.reject_instance(instance)?;
                }
                if matches!(
                    saved.source_mode,
                    SourceOperationMode::Advance | SourceOperationMode::Resync { .. }
                ) || retained_history_instance.is_some()
                {
                    if let OperationStatus::Published(view) = &status {
                        self.check_retired_terminal_view_v2(retirement, *operation, *view, budget)?;
                    }
                    return self
                        .submitted_or_reconcile(
                            *operation,
                            Ok(SubmitOutcome::Terminal(status)),
                            None,
                        )
                        .map(Some);
                }
                // A former catalog marker cannot establish this request's
                // classification when terminal metadata carries no Source or
                // authenticated History witness.
                self.check_retired_resume_body_v2(retirement, *operation, saved.request, budget)?;
            }
            NativeIndexRequest::ReplayDslV1(input) | NativeIndexRequest::ReadSealedDslV1(input) => {
                retirement.reject_projection(input.projection)?;
            }
            NativeIndexRequest::SubmitSealedDslV1(input) => {
                retirement.reject_projection(input.replay.projection)?;
            }
            NativeIndexRequest::ClaimWork { intent, .. }
            | NativeIndexRequest::WorkStatus(intent) => retirement.check_work_intent(intent)?,
            NativeIndexRequest::RenewWork { claim, .. } => {
                retirement.check_work_intent(&claim.intent)?;
            }
            _ => {}
        }
        budget.checkpoint()?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::{
        AttemptId, AttemptRef, CollectionId, Digest, ProducerId, Refusal, SlotId, StreamId,
        TenantId, ViewRef, WorkspaceKey, WorktreeId,
    };
    use quanta_index_core::index::WorkLimits;

    fn operation() -> OperationKey {
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
            sequence: 1,
        }
    }

    fn budget() -> WorkBudget {
        WorkBudget::with_request(
            WorkLimits {
                reads: 1,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            quanta_index_core::RequestBudgetV1::unbounded(),
        )
    }

    #[test]
    fn pending_resume_rechecks_terminal_after_custody_error() {
        let key = operation();
        let pending = OperationStatus::Pending(AttemptRef {
            operation: key,
            attempt: AttemptId([8; 16]),
            fence: 1,
        });
        let published = OperationStatus::Published(ViewRef {
            workspace: key.workspace,
            epoch: key.epoch,
            sequence: 2,
            digest: Digest([7; 32]),
        });
        let checked = std::cell::Cell::new(0);
        let rechecked = std::cell::Cell::new(0);
        let settled = checked_resume_status_v2(
            pending.clone(),
            &mut budget(),
            |_budget| {
                checked.set(checked.get() + 1);
                Err(IndexError::CorruptObject("request body reclaimed".into()))
            },
            |_budget| {
                rechecked.set(rechecked.get() + 1);
                Ok(published.clone())
            },
        );
        assert!(matches!(settled, Ok(Some(OperationStatus::Published(_)))));
        assert_eq!((checked.get(), rechecked.get()), (1, 1));

        let still_pending = checked_resume_status_v2(
            pending.clone(),
            &mut budget(),
            |_budget| Err(IndexError::InvalidInput("retired identity".into())),
            |_budget| Ok(pending.clone()),
        );
        assert!(matches!(still_pending, Err(IndexError::InvalidInput(_))));

        let dispatch = checked_resume_status_v2(
            pending,
            &mut budget(),
            |_budget| Ok(()),
            |_budget| panic!("checked pending resume rechecked status"),
        );
        assert!(matches!(dispatch, Ok(None)));

        let terminal = checked_resume_status_v2(
            OperationStatus::Refused(Refusal::InvalidInput),
            &mut budget(),
            |_budget| panic!("terminal resume read pending request body"),
            |_budget| panic!("terminal resume rechecked status"),
        );
        assert!(matches!(terminal, Ok(Some(OperationStatus::Refused(_)))));
    }
}
