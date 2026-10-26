//! Shared real-service pump fixture; no publication authority is introduced.

#[path = "native_physical_profile.rs"]
mod native_physical_profile;

use super::native_runtime::native::{
    FixedPumpPolicy, NativePumpConfig, NativePumpScope, NativeResourcePlan, NativeServiceResources,
};
use quanta_index_contract::{
    index::{
        CollectionId, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile,
        InputDomain, InstanceSpec, OperationKey, OperationStatus, OwnerInputBinding, OwnerKey,
        OwnerMemberBinding, OwnerMutation, ProducerId, PublicationGroupId, SchemaId, SlotId,
        SourceEvent, SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId,
        StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewSelection, WorkIntent,
        WorkScope, WorkspaceKey, WorktreeId,
    },
    native_protocol::{NativeIndexRequest, NativeIndexResponse},
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        AttemptContext, IndexError, OwnedWorkPolicy, WorkLimits,
        effort::ResourceNeed,
        ports::{IndexFuture, IndexOwnerBuildPort},
    },
};
use quanta_index_ipc::ShutdownHandle;
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry};
use quanta_index_sdk::native_uds::NativeUdsClient;
use sha2::{Digest as _, Sha256};
use std::{
    error::Error,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn Error>>;

pub(super) struct ServiceThreadGuard {
    pub(super) shutdown: ShutdownHandle,
    pub(super) runner: Option<thread::JoinHandle<()>>,
}

impl ServiceThreadGuard {
    pub(super) fn stop(&mut self) -> TestResult {
        self.shutdown.trigger();
        if let Some(runner) = self.runner.take() {
            runner.join().map_err(|_panic| "native service panicked")?;
        }
        Ok(())
    }
}

impl Drop for ServiceThreadGuard {
    fn drop(&mut self) {
        drop(self.stop());
    }
}

pub(super) fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

pub(super) fn limits() -> WorkLimits {
    WorkLimits {
        reads: 100_000,
        writes: 100_000,
        bytes: 64 * 1024 * 1024,
        mutations: 100_000,
    }
}

pub(super) fn resources() -> NativeServiceResources {
    let unit = ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024,
        // Fixtures use sub-8 KiB objects: reserve wire + decoded data together.
        memory_bytes: 64 * 1024,
        provider_units: 0,
    };
    NativeServiceResources {
        // Map spill currently reserves a fixed 4 GiB envelope even for a
        // small delta. This fixture covers pump behavior with that configured
        // capacity; it does not qualify a smaller scratch envelope.
        physical: native_physical_profile::profile(),
        scratch_reserved_bytes: 16 * 1024 * 1024 * 1024,
        read_ticket_lease: Duration::from_secs(30),
        execution: NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 5,
                io_bytes: 5120,
                // Five execution floors and one bounded escaping-data allowance.
                memory_bytes: (5 + 1) * 64 * 1024,
                ..unit
            },
            control: unit,
            interactive: unit,
            core: unit,
            enrichment: unit,
        },
    }
}

pub(super) fn config() -> NativePumpConfig {
    NativePumpConfig {
        scopes: vec![NativePumpScope {
            workspace: workspace(),
            epoch: 1,
        }],
        batch_limit: 16,
        max_in_flight: 1,
        poll_interval: Duration::from_millis(20),
        new_assignment: OwnedWorkPolicy {
            dispatch_limits: limits(),
            max_dispatches: 2,
            total_timeout_ms: 10_000,
            lease_ms: 2_000,
            retry_backoff_ms: 100,
        },
        cadence: Arc::new(FixedPumpPolicy),
    }
}

pub(super) fn private_root() -> TestResult<tempfile::TempDir> {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(root)
}

pub(super) fn budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(5))
}

pub(super) fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

pub(super) fn operation_in(workspace: WorkspaceKey, sequence: u64) -> OperationKey {
    OperationKey {
        workspace,
        ..operation(sequence)
    }
}

pub(super) fn owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: operation(1).producer,
        stable_id: StableOwnerId([id; 16]),
    }
}

pub(super) fn spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([31; 16]),
        kind: IndexKind([32; 16]),
        group: PublicationGroupId([33; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([34; 16]),
        policy: Digest([35; 32]),
        format: StorageFormatId([36; 16]),
    }
}

pub(super) struct ProviderGate {
    pub(super) entered: mpsc::SyncSender<WorkIntent>,
    pub(super) cancellation_notice: Option<mpsc::SyncSender<()>>,
    pub(super) released: Mutex<bool>,
    pub(super) wake: Condvar,
    pub(super) calls: AtomicUsize,
}

impl ProviderGate {
    pub(super) fn release(&self) {
        if let Ok(mut released) = self.released.lock() {
            *released = true;
            self.wake.notify_all();
        }
    }
}

pub(super) struct ReleaseProvider(pub(super) Arc<ProviderGate>);

impl Drop for ReleaseProvider {
    fn drop(&mut self) {
        self.0.release();
    }
}

pub(super) struct OwnerAdapter {
    gate: Arc<ProviderGate>,
    provider_units: u64,
}

impl IndexOwnerBuildPort for OwnerAdapter {
    type Candidate = OwnerMemberBinding;
    fn resource_demand(&self) -> ResourceNeed {
        ResourceNeed {
            provider_units: self.provider_units,
            ..ResourceNeed::default()
        }
    }
    fn validate_instance(
        &self,
        _instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        Ok(())
    }

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        _prior: Option<OwnerMemberBinding>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            context.budget.checkpoint()?;
            let _cancellation_notice = self.gate.cancellation_notice.as_ref().map(|sender| {
                let sender = sender.clone();
                context.budget.request().cancel_waiter(Arc::new(move || {
                    let _sent = sender.try_send(());
                }))
            });
            let _prior = self.gate.calls.fetch_add(1, Ordering::SeqCst);
            self.gate
                .entered
                .send(WorkIntent {
                    workspace: workspace(),
                    epoch: 1,
                    instance: instance.instance,
                    scope: WorkScope::Owner {
                        owner,
                        expected: input,
                    },
                })
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            let mut released = self
                .gate
                .released
                .lock()
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            while !*released {
                released = self
                    .gate
                    .wake
                    .wait(released)
                    .map_err(|error| IndexError::Storage(error.to_string()))?;
            }
            drop(released);
            context.budget.checkpoint()?;
            let payload = serde_json::to_vec(&(owner, input))
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
            let artifact = context.put(&payload, &input.source.into_iter().collect::<Vec<_>>())?;
            Ok(OwnerMemberBinding {
                instance: instance.instance,
                owner,
                input,
                format: instance.format,
                artifact,
                logical_commitment: Digest(Sha256::digest(payload).into()),
            })
        })
    }
    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            let stored = context.objects.get(candidate.artifact, context.budget)?;
            if stored.registration.reference != candidate.artifact
                || stored.registration.children
                    != candidate.input.source.into_iter().collect::<Vec<_>>()
                || Digest(Sha256::digest(&stored.payload).into()) != candidate.logical_commitment
            {
                return Err(IndexError::CorruptObject(
                    "owner artifact gold mismatch".into(),
                ));
            }
            Ok(candidate)
        })
    }
}

pub(super) fn registry(gate: &Arc<ProviderGate>) -> TestResult<AdapterRegistry> {
    registry_with_provider(gate, 0)
}

pub(super) fn registry_with_provider(
    gate: &Arc<ProviderGate>,
    provider_units: u64,
) -> TestResult<AdapterRegistry> {
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        OwnerAdapter {
            gate: Arc::clone(gate),
            provider_units,
        },
    )?;
    Ok(registry)
}

pub(super) fn published(outcome: SubmitOutcome) -> TestResult<quanta_index_core::index::ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => Err(format!("expected publication, got {other:?}").into()),
    }
}

pub(super) fn advance(
    client: &NativeUdsClient,
    view: quanta_index_core::index::ViewRef,
    sequence: u64,
    owner_id: u8,
) -> TestResult<quanta_index_core::index::ViewRef> {
    advance_in(client, workspace(), view, sequence, owner_id)
}

pub(super) fn advance_in(
    client: &NativeUdsClient,
    workspace: WorkspaceKey,
    view: quanta_index_core::index::ViewRef,
    sequence: u64,
    owner_id: u8,
) -> TestResult<quanta_index_core::index::ViewRef> {
    let key = operation_in(workspace, sequence);
    let ingress = client.begin_input(key, &budget())?;
    let source = client.stage_source(
        ingress,
        format!("owner-{owner_id}.rs"),
        b"source",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: owner(owner_id),
            expected: None,
            next: Some(source),
        }],
        &budget(),
    )?;
    let prior = client.read_view(ViewSelection::Exact(view), &budget())?;
    let event_sequence = sequence.checked_sub(1).ok_or("event sequence underflow")?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("predecessor underflow")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    published(client.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)
}

pub(super) fn configure(client: &NativeUdsClient) -> TestResult<quanta_index_core::index::ViewRef> {
    configure_in(client, workspace())
}

pub(super) fn configure_in(
    client: &NativeUdsClient,
    workspace: WorkspaceKey,
) -> TestResult<quanta_index_core::index::ViewRef> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace,
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("expected genesis".into());
    };
    let ingress = client.begin_input(operation_in(workspace, 1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![spec()],
        },
        &budget(),
    )?;
    let configured = published(client.submit(
        UpdateRequest {
            operation: operation_in(workspace, 1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    Ok(configured)
}
