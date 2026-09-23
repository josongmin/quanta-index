//! Process-owned readiness, separate from a repository's active generation.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use quanta_index_contract::{
    ProcessComponentsHealthV1, ProcessProviderClaimV1, ProcessProviderReadinessV1,
    ProcessReadinessPhaseV1, ProcessReadinessReasonV1, ProcessReadinessV1,
};
use quanta_index_core::CoreError;
use quanta_index_search_plane::readiness::ActivationCatalog;
use quanta_index_search_plane::{
    ActivationPromotionParts, ProcessReadinessPort, SearchCorpusGenerationV1,
    SearchCorpusLifecycleOwner,
};

use crate::app::integrity_scrub::ScrubTalliesV1;
use crate::app::maintenance::MaintenanceTallies;
use crate::app::supervisor::{SupervisorPhase, SupervisorStatus};

pub(crate) struct RuntimeReadiness {
    pub status: Arc<SupervisorStatus>,
    pub maintenance: Arc<MaintenanceTallies>,
    pub maintenance_cadence: Duration,
    pub activation_catalog: Arc<ActivationCatalog>,
    pub lifecycle: Arc<SearchCorpusLifecycleOwner>,
    pub promotion: ActivationPromotionParts,
    pub provider_claim: ProcessProviderClaimV1,
    pub provider_child_required: bool,
    pub scrub: Arc<ScrubTalliesV1>,
    /// One physical proof per active identity and scrub invalidation epoch.
    pub proven_active: Mutex<Option<ProvenActive>>,
}

pub(crate) struct ProvenActive {
    identity: Vec<SearchCorpusGenerationV1>,
    scrub_epoch: (u64, u64),
}

impl ProvenActive {
    pub(crate) fn new(identity: Vec<SearchCorpusGenerationV1>, scrub_epoch: (u64, u64)) -> Self {
        Self {
            identity,
            scrub_epoch,
        }
    }
}

impl ProcessReadinessPort for RuntimeReadiness {
    fn readiness(&self) -> Result<ProcessReadinessV1, CoreError> {
        let before = self.activation_catalog.active_inventory_v1()?;
        let scrub_epoch = self.scrub.proof_invalidation_epoch();
        // The first observation of an active identity uses the same physical
        // proof as boot. Subsequent health polls reuse only that exact
        // identity until an activation or scrub corruption invalidates it.
        let active_integrity = if before.0.is_empty() {
            None
        } else {
            let mut cached_proof = self.proven_active.lock().map_err(|error| {
                CoreError::Storage(format!("process readiness proof cache poisoned: {error}"))
            })?;
            if cached_proof
                .as_ref()
                .is_some_and(|proof| proof.identity == before.0 && proof.scrub_epoch == scrub_epoch)
                && self.scrub.proof_invalidation_epoch() == scrub_epoch
            {
                Some(true)
            } else {
                let proof = self
                    .lifecycle
                    .validate_rehydrated_active_generations_v1(&self.promotion);
                let after = self.activation_catalog.active_inventory_v1()?;
                let valid = matches!(proof, Ok(count) if count == before.0.len() && after == before)
                    && self.scrub.proof_invalidation_epoch() == scrub_epoch;
                *cached_proof = valid.then(|| ProvenActive::new(before.0.clone(), scrub_epoch));
                drop(cached_proof);
                Some(valid)
            }
        };
        let (query_plane, control_plane, ingest_plane) = self.status.required_planes();
        let phase = match self.status.phase() {
            SupervisorPhase::Starting => ProcessReadinessPhaseV1::Starting,
            SupervisorPhase::Ready => ProcessReadinessPhaseV1::Ready,
            SupervisorPhase::Draining => ProcessReadinessPhaseV1::Draining,
            SupervisorPhase::Stopped => ProcessReadinessPhaseV1::Stopped,
            SupervisorPhase::Failed => ProcessReadinessPhaseV1::Failed,
        };
        let components = ProcessComponentsHealthV1 {
            query_plane,
            control_plane,
            ingest_plane,
            maintenance_heartbeat: self.maintenance.heartbeat_fresh(self.maintenance_cadence)?,
            required_backend: true, // Successful runtime assembly opened and proved required adapters.
            provider: ProcessProviderReadinessV1 {
                claim: self.provider_claim,
                healthy: self.provider_claim == ProcessProviderClaimV1::Required
                    && (!self.provider_child_required || self.status.provider_running()),
            },
        };
        // A cache hit and an empty catalog need the same final race check as a
        // fresh probe. Never publish a ready report for a changed active head
        // or a scrub finding that arrived during this observation.
        if self.activation_catalog.active_inventory_v1()? != before
            || self.scrub.proof_invalidation_epoch() != scrub_epoch
        {
            return Err(CoreError::Storage(
                "process readiness inputs changed during observation".to_owned(),
            ));
        }
        Ok(synthesize(phase, components, active_integrity, before.1))
    }
}

fn synthesize(
    phase: ProcessReadinessPhaseV1,
    components: ProcessComponentsHealthV1,
    active_candidate_integrity: Option<bool>,
    active_repositories: u64,
) -> ProcessReadinessV1 {
    use ProcessReadinessReasonV1 as Reason;
    let mut not_ready_reasons = Vec::new();
    if phase != ProcessReadinessPhaseV1::Ready {
        not_ready_reasons.push(Reason::SupervisorNotReady);
    }
    if !components.query_plane {
        not_ready_reasons.push(Reason::QueryPlaneUnhealthy);
    }
    if !components.control_plane {
        not_ready_reasons.push(Reason::ControlPlaneUnhealthy);
    }
    if !components.ingest_plane {
        not_ready_reasons.push(Reason::IngestPlaneUnhealthy);
    }
    if !components.maintenance_heartbeat {
        not_ready_reasons.push(Reason::MaintenanceHeartbeatStale);
    }
    if !components.required_backend {
        not_ready_reasons.push(Reason::RequiredBackendOpenUnproven);
    }
    if components.provider.claim == ProcessProviderClaimV1::Required && !components.provider.healthy
    {
        not_ready_reasons.push(Reason::ProviderRequiredUnhealthy);
    }
    if active_candidate_integrity == Some(false) {
        not_ready_reasons.push(Reason::ActiveCandidateIntegrityFailed);
    }
    ProcessReadinessV1 {
        ready: not_ready_reasons.is_empty(),
        supervisor_phase: phase,
        components,
        active_candidate_integrity,
        active_repositories,
        not_ready_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> ProcessComponentsHealthV1 {
        ProcessComponentsHealthV1 {
            query_plane: true,
            control_plane: true,
            ingest_plane: true,
            maintenance_heartbeat: true,
            required_backend: true,
            provider: ProcessProviderReadinessV1 {
                claim: ProcessProviderClaimV1::Disabled,
                healthy: false,
            },
        }
    }

    #[test]
    fn zero_active_repositories_can_be_process_ready() {
        let report = synthesize(ProcessReadinessPhaseV1::Ready, healthy(), None, 0);
        assert!(report.ready);
        assert!(report.not_ready_reasons.is_empty());
    }

    #[test]
    fn declared_degraded_provider_does_not_claim_required_health() {
        let mut components = healthy();
        components.provider = ProcessProviderReadinessV1 {
            claim: ProcessProviderClaimV1::Degraded,
            healthy: false,
        };
        let report = synthesize(ProcessReadinessPhaseV1::Ready, components, None, 0);
        assert!(report.ready);
        assert!(!report.components.provider.healthy);
    }

    #[test]
    fn every_required_component_failure_is_reported() {
        let mut components = healthy();
        components.query_plane = false;
        components.ingest_plane = false;
        components.maintenance_heartbeat = false;
        components.provider.claim = ProcessProviderClaimV1::Required;
        components.provider.healthy = false;
        let report = synthesize(ProcessReadinessPhaseV1::Failed, components, Some(false), 2);
        assert!(!report.ready);
        assert_eq!(report.active_repositories, 2);
        assert_eq!(report.not_ready_reasons.len(), 6);
        assert!(
            report
                .not_ready_reasons
                .contains(&ProcessReadinessReasonV1::QueryPlaneUnhealthy)
        );
        assert!(
            report
                .not_ready_reasons
                .contains(&ProcessReadinessReasonV1::ActiveCandidateIntegrityFailed)
        );
    }
}
