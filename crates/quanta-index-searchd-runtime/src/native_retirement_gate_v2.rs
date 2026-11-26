//! Request admission under the signed, BOOT-verified retirement capability.

use super::{NativeDispatcher, NativeRetirementAuthorityV2};
use quanta_index_contract::{
    index::{
        IndexInstanceId, IndexProfile, InputDomain, ObjectRef, OperationStatus, ProjectionId,
        SubmitOutcome, UpdateKind, UpdateRequest, WorkIntent, WorkScope,
    },
    native_protocol::{NativeIndexRequest, NativeIndexResponse},
};
use quanta_index_core::index::{
    Digest, IndexError, OWNED_WORK_PRODUCER, RetirementCheckV2, WorkBudget, WorkUsage,
    ports::{ImmutableObjectPort, OperationInspectAccess, OperationJournalPort},
};
use sha2::{Digest as _, Sha256};

struct RequestHasherV2 {
    hash: Sha256,
    bytes: u64,
}

impl std::io::Write for RequestHasherV2 {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.checked_add(bytes.len() as u64)
            .ok_or_else(|| std::io::Error::other("retirement request length overflow"))?;
        self.hash.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn request_sha256_v2(request: &UpdateRequest, budget: &mut WorkBudget) -> Result<Digest, IndexError> {
    let mut writer = RequestHasherV2 { hash: Sha256::new(), bytes: 0 };
    serde_json::to_writer(&mut writer, request)
        .map_err(|error| IndexError::InvalidInput(format!("retirement request encoding: {error}")))?;
    budget.charge(WorkUsage { bytes: writer.bytes, ..WorkUsage::default() })?;
    Ok(Digest(writer.hash.finalize().into()))
}

fn retired() -> IndexError {
    IndexError::InvalidInput("retired projection or instance identity".into())
}

fn target_profile_ref(kind: &UpdateKind) -> Option<ObjectRef> {
    match kind {
        UpdateKind::Reconfigure { profile } => Some(*profile),
        UpdateKind::HistoryAdvance(transition) => Some(transition.profile),
        UpdateKind::SourceHistoryAdvance(transition)
        | UpdateKind::SourceHistoryResync { transition, .. } => Some(transition.history.profile),
        UpdateKind::SourceAdvance(_)
        | UpdateKind::SourceResync { .. }
        | UpdateKind::ProjectionAdvance(_)
        | UpdateKind::HistoryAppend(_)
        | UpdateKind::CompactSourceLocal { .. }
        | UpdateKind::Materialize { .. } => None,
    }
}

impl NativeRetirementAuthorityV2 {
    fn reject_projection(&self, projection: ProjectionId) -> Result<(), IndexError> {
        if self
            .retired_projections
            .binary_search_by_key(&projection.0, |value| value.0)
            .is_ok()
        {
            Err(retired())
        } else {
            Ok(())
        }
    }

    fn reject_instance(&self, instance: IndexInstanceId) -> Result<(), IndexError> {
        if self
            .retired_instances
            .binary_search_by_key(&instance.0, |value| value.0)
            .is_ok()
        {
            Err(retired())
        } else {
            Ok(())
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
    fn check_update_v2(
        &self,
        retirement: &NativeRetirementAuthorityV2,
        request: &UpdateRequest,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if let Some(profile) = target_profile_ref(&request.kind) {
            quanta_index_kernel::with_loaded_profile_v1(
                profile,
                self.objects.as_ref(),
                budget,
                |loaded, budget| retirement.check_profile(loaded, budget),
            )??;
        }
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
            NativeIndexRequest::Submit(update) => {
                self.check_update_v2(retirement, update, budget)?;
                let request_sha256 = request_sha256_v2(update, budget)?;
                budget.set_retirement_check_v2(RetirementCheckV2 {
                    operation: update.operation,
                    request_sha256,
                    policy_sha256: retirement.policy_ref.sha256,
                })?;
            }
            NativeIndexRequest::Resume(operation) => {
                if operation.producer == OWNED_WORK_PRODUCER {
                    return Err(IndexError::InvalidInput("reserved owned work producer".into()));
                }
                let status = self.kernel
                    .status(*operation, OperationInspectAccess::Request(budget))?;
                if status == OperationStatus::Unknown {
                    // Settle the observation here. Dispatching again could pick up
                    // a concurrent admission that this check never inspected.
                    return self.submitted_or_reconcile(
                        *operation,
                        Ok(SubmitOutcome::Unknown(*operation)),
                        None,
                    ).map(Some);
                }
                let saved = self.catalog.operation_request(*operation, budget)?;
                // The policy marker is not an independently verified
                // classification of the original request. It cannot replace
                // the retained request body after physical custody release.
                let object = self.objects.get(saved.request, budget)?;
                if object.registration.reference != saved.request {
                    return Err(IndexError::DigestConflict);
                }
                quanta_index_kernel::with_decoded_update_request_v1(
                    &object.payload,
                    budget,
                    |update, budget| {
                        if update.operation != *operation {
                            return Err(IndexError::DigestConflict);
                        }
                        self.check_update_v2(retirement, update, budget)
                    },
                )??;
            }
            NativeIndexRequest::ReplayDslV1(input) => {
                retirement.reject_projection(input.projection)?;
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
        AuxiliaryGenerationKeyV1, HistoryAppendV1, HistoryGenerationAdvanceV1,
        HistorySourceCutV1, HistoryTransition, MapRoot, OwnerMutationSet, SourceCut,
        SourceHistoryTransitionV1, SourceTransition,
    };
    use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }

    #[test]
    fn retirement_checks_every_update_profile_reference() -> TestResult {
        let profile = reference(1);
        let cut = HistorySourceCutV1 {
            generation: AuxiliaryGenerationKeyV1 {
                repo_id: RepoId::new("history-repo")?,
                revision_id: RevisionId::new("history-revision")?,
                generation: ManifestGeneration::new(2),
            },
            durable_epoch: AuxEpochV1::new(3),
            snapshot_digest: Digest([2; 32]),
            replay_start: Digest([3; 32]),
        };
        let source = SourceTransition {
            expected_source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            owners: OwnerMutationSet {
                root: MapRoot(None),
                count: 0,
                digest: Digest([4; 32]),
            },
            events: Vec::new(),
        };
        let history = HistoryTransition {
            instance: IndexInstanceId([5; 16]),
            expected: None,
            next: cut.clone(),
            input: reference(6),
            profile,
        };
        let append = HistoryAppendV1 {
            instance: history.instance,
            expected: cut.clone(),
            input: reference(7),
            profile,
        };
        let source_history = SourceHistoryTransitionV1 {
            source: source.clone(),
            history: HistoryGenerationAdvanceV1 {
                prior_instance: IndexInstanceId([8; 16]),
                expected: cut,
                instance: history.instance,
                input: reference(9),
                profile,
            },
        };
        for kind in [
            UpdateKind::Reconfigure { profile },
            UpdateKind::HistoryAdvance(history),
            UpdateKind::SourceHistoryAdvance(Box::new(source_history.clone())),
            UpdateKind::SourceHistoryResync {
                prior_epoch: 1,
                transition: Box::new(source_history),
            },
        ] {
            assert_eq!(target_profile_ref(&kind), Some(profile));
        }
        assert_eq!(target_profile_ref(&UpdateKind::HistoryAppend(append)), None);
        assert_eq!(target_profile_ref(&UpdateKind::SourceAdvance(source)), None);
        Ok(())
    }
}
