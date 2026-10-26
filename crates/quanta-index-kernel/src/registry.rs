use std::{collections::BTreeMap, sync::Arc};

use quanta_index_core::index::{
    AttemptContext, BaseMember, CoverageBinding, DeltaInput, GroupActivation, IndexError,
    IndexKind, InputBinding, InputDomain, InstanceSpec, MapRoot, MemberBinding, ObjectRef,
    OwnerInputBinding, OwnerKey, OwnerMemberBinding, OwnerMutationSet, PlanContext, PlanDecision,
    ProjectionBinding, SchemaId, SourceCut, SourceDelta, StorageFormatId, WorkBudget,
    effort::ResourceNeed,
    ports::{
        IndexBuildPort, IndexFuture, IndexOwnerBuildPort, IndexProjectionBuildPort,
        ObjectMapBatchPort,
    },
};

use crate::build_staging::{self, BuildFuture};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterDescriptor {
    pub kind: IndexKind,
    pub schema: SchemaId,
    pub format: StorageFormatId,
}

trait ErasedAdapter: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    fn resource_demand(&self) -> ResourceNeed;
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError>;
    fn build<'a>(
        &'a self,
        base: BaseMember,
        delta: Option<&'a SourceDelta>,
        plan: PlanContext,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<BuildFuture<'a, MemberBinding>, IndexError>;
}

struct Adapter<A, F> {
    descriptor: AdapterDescriptor,
    resource_demand: ResourceNeed,
    backend: A,
    decode_delta: F,
}

impl<A, F> ErasedAdapter for Adapter<A, F>
where
    A: IndexBuildPort + 'static,
    F: Fn(&SourceDelta) -> Result<A::Delta, IndexError> + Send + Sync + 'static,
{
    fn descriptor(&self) -> AdapterDescriptor {
        self.descriptor
    }

    fn resource_demand(&self) -> ResourceNeed {
        self.resource_demand
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.backend.validate_instance(instance)
    }

    fn build<'a>(
        &'a self,
        base: BaseMember,
        delta: Option<&'a SourceDelta>,
        plan: PlanContext,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<BuildFuture<'a, MemberBinding>, IndexError> {
        build_staging::future(attempt, move |attempt| async move {
            attempt.budget.checkpoint()?;
            if self.backend.resource_demand() != self.resource_demand {
                return Err(IndexError::BackendViolation(
                    "build adapter resource demand changed after registration".into(),
                ));
            }
            let input = delta.map_or_else(
                || Ok(DeltaInput::Bootstrap),
                |delta| (self.decode_delta)(delta).map(DeltaInput::Patch),
            );
            attempt.budget.checkpoint()?;
            let decision = self.backend.plan(base, input?, &plan, attempt.budget)?;
            let binding = match decision {
                PlanDecision::Reuse(binding) => *binding,
                PlanDecision::Incremental(plan) => {
                    let candidate = self.backend.prepare(plan, attempt).await?;
                    self.backend.verify_and_seal(candidate, attempt).await?
                }
                PlanDecision::RebuildRequired => return Err(IndexError::RebuildRequired),
                PlanDecision::MaintenanceRequired => return Err(IndexError::MaintenanceRequired),
            };
            if self.backend.resource_demand() != self.resource_demand {
                return Err(IndexError::BackendViolation(
                    "build adapter resource demand changed during execution".into(),
                ));
            }
            validate_member(&binding, &plan.instance, plan.input)?;
            Ok(binding)
        })
    }
}

fn validate_member(
    binding: &MemberBinding,
    instance: &InstanceSpec,
    input: InputBinding,
) -> Result<(), IndexError> {
    if binding.instance != instance.instance
        || binding.format != instance.format
        || binding.input != input
    {
        return Err(IndexError::BackendViolation(format!(
            "instance {:?} returned mismatched member binding",
            instance.instance
        )));
    }
    Ok(())
}

trait ErasedProjectionAdapter: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    fn resource_demand(&self) -> ResourceNeed;
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError>;
    fn build<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<MemberBinding>,
        projection: &'a ProjectionBinding,
        affected: Option<OwnerMutationSet>,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<BuildFuture<'a, MemberBinding>, IndexError>;
}

struct ProjectionAdapter<A> {
    descriptor: AdapterDescriptor,
    resource_demand: ResourceNeed,
    backend: A,
}

impl<A: IndexProjectionBuildPort + 'static> ErasedProjectionAdapter for ProjectionAdapter<A> {
    fn descriptor(&self) -> AdapterDescriptor {
        self.descriptor
    }
    fn resource_demand(&self) -> ResourceNeed {
        self.resource_demand
    }
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.backend.validate_instance(instance)
    }
    fn build<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<MemberBinding>,
        projection: &'a ProjectionBinding,
        affected: Option<OwnerMutationSet>,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<BuildFuture<'a, MemberBinding>, IndexError> {
        build_staging::future(attempt, move |attempt| async move {
            attempt.budget.checkpoint()?;
            if self.backend.resource_demand() != self.resource_demand {
                return Err(IndexError::BackendViolation(
                    "projection build demand changed after registration".into(),
                ));
            }
            let candidate = self
                .backend
                .prepare_projection(instance, prior, *projection, affected, attempt)
                .await?;
            let binding = self.backend.verify_projection(candidate, attempt).await?;
            if self.backend.resource_demand() != self.resource_demand {
                return Err(IndexError::BackendViolation(
                    "projection build demand changed during execution".into(),
                ));
            }
            validate_member(
                &binding,
                instance,
                InputBinding {
                    root: projection.owners,
                    schema: instance.schema,
                    policy: instance.policy,
                    context: projection.materialization_context(),
                },
            )?;
            Ok(binding)
        })
    }
}

trait ErasedOwnerAdapter: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    fn resource_demand(&self) -> ResourceNeed;
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError>;
    fn validate_coverage(&self, coverage: &CoverageBinding) -> Result<(), IndexError>;
    fn prepare_searchable<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<&'a CoverageBinding>,
        owners: MapRoot,
        materialized: Option<(OwnerKey, ObjectRef)>,
        object_maps: Option<&'a Arc<dyn ObjectMapBatchPort>>,
        attempt: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, quanta_index_core::index::SearchablePreparation>;
    fn build<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        prior: Option<OwnerMemberBinding>,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<BuildFuture<'a, OwnerMemberBinding>, IndexError>;
}

struct OwnerAdapter<A> {
    descriptor: AdapterDescriptor,
    resource_demand: ResourceNeed,
    backend: A,
}

impl<A: IndexOwnerBuildPort + 'static> ErasedOwnerAdapter for OwnerAdapter<A> {
    fn validate_coverage(&self, coverage: &CoverageBinding) -> Result<(), IndexError> {
        self.backend.validate_coverage(coverage)
    }

    fn prepare_searchable<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<&'a CoverageBinding>,
        owners: MapRoot,
        materialized: Option<(OwnerKey, ObjectRef)>,
        object_maps: Option<&'a Arc<dyn ObjectMapBatchPort>>,
        attempt: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, quanta_index_core::index::SearchablePreparation> {
        Box::pin(async move {
            attempt.budget.checkpoint()?;
            if self.backend.resource_demand() != self.resource_demand {
                return Err(IndexError::BackendViolation(
                    "owner layout demand changed after registration".into(),
                ));
            }
            let searchable = self
                .backend
                .prepare_searchable(instance, prior, owners, materialized, object_maps, attempt)
                .await?;
            if self.backend.resource_demand() != self.resource_demand {
                return Err(IndexError::BackendViolation(
                    "owner layout demand changed during execution".into(),
                ));
            }
            Ok(searchable)
        })
    }
    fn descriptor(&self) -> AdapterDescriptor {
        self.descriptor
    }

    fn resource_demand(&self) -> ResourceNeed {
        self.resource_demand
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.backend.validate_instance(instance)
    }

    fn build<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        prior: Option<OwnerMemberBinding>,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<BuildFuture<'a, OwnerMemberBinding>, IndexError> {
        build_staging::future(attempt, move |attempt| async move {
            if self.backend.resource_demand() != self.resource_demand {
                return Err(IndexError::BackendViolation(
                    "owner adapter resource demand changed after registration".into(),
                ));
            }
            let candidate = self
                .backend
                .prepare_owner(instance, owner, input, prior, attempt)
                .await?;
            let binding = self.backend.verify_owner(candidate, attempt).await?;
            if self.backend.resource_demand() != self.resource_demand {
                return Err(IndexError::BackendViolation(
                    "owner adapter resource demand changed during execution".into(),
                ));
            }
            validate_owner_member(&binding, instance, owner, input)?;
            Ok(binding)
        })
    }
}

pub(crate) fn validate_owner_member(
    binding: &OwnerMemberBinding,
    instance: &InstanceSpec,
    owner: OwnerKey,
    input: OwnerInputBinding,
) -> Result<(), IndexError> {
    if binding.instance != instance.instance
        || binding.owner != owner
        || binding.input != input
        || binding.format != instance.format
    {
        return Err(IndexError::BackendViolation(
            "owner adapter returned mismatched binding".into(),
        ));
    }
    Ok(())
}

#[derive(Default)]
pub struct AdapterRegistry {
    core: BTreeMap<IndexKind, Arc<dyn ErasedAdapter>>,
    projection: BTreeMap<IndexKind, Arc<dyn ErasedProjectionAdapter>>,
    owners: BTreeMap<IndexKind, Arc<dyn ErasedOwnerAdapter>>,
}

impl AdapterRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Serial builders can execute any registered descriptor. Each class
    /// therefore reserves the componentwise maximum declared by its adapters.
    #[must_use]
    pub fn resource_demands(&self) -> (ResourceNeed, ResourceNeed) {
        fn maximum(current: ResourceNeed, next: ResourceNeed) -> ResourceNeed {
            ResourceNeed {
                cpu_units: current.cpu_units.max(next.cpu_units),
                io_bytes: current.io_bytes.max(next.io_bytes),
                memory_bytes: current.memory_bytes.max(next.memory_bytes),
                provider_units: current.provider_units.max(next.provider_units),
            }
        }
        let core = self
            .core
            .values()
            .fold(ResourceNeed::default(), |need, adapter| {
                maximum(need, adapter.resource_demand())
            });
        let core = self.projection.values().fold(core, |need, adapter| {
            maximum(need, adapter.resource_demand())
        });
        let enrichment = self
            .owners
            .values()
            .fold(ResourceNeed::default(), |need, adapter| {
                maximum(need, adapter.resource_demand())
            });
        (core, enrichment)
    }

    pub fn register<A, F>(
        &mut self,
        descriptor: AdapterDescriptor,
        backend: A,
        decode_delta: F,
    ) -> Result<(), IndexError>
    where
        A: IndexBuildPort + 'static,
        F: Fn(&SourceDelta) -> Result<A::Delta, IndexError> + Send + Sync + 'static,
    {
        if self.owners.contains_key(&descriptor.kind)
            || self.projection.contains_key(&descriptor.kind)
        {
            return Err(IndexError::InvalidInput("duplicate adapter kind".into()));
        }
        match self.core.entry(descriptor.kind) {
            std::collections::btree_map::Entry::Occupied(_) => {
                Err(IndexError::InvalidInput("duplicate adapter kind".into()))
            }
            std::collections::btree_map::Entry::Vacant(slot) => {
                let _registered = slot.insert(Arc::new(Adapter {
                    descriptor,
                    resource_demand: backend.resource_demand(),
                    backend,
                    decode_delta,
                }));
                Ok(())
            }
        }
    }

    pub fn register_owner<A: IndexOwnerBuildPort + 'static>(
        &mut self,
        descriptor: AdapterDescriptor,
        backend: A,
    ) -> Result<(), IndexError> {
        if self.core.contains_key(&descriptor.kind)
            || self.projection.contains_key(&descriptor.kind)
        {
            return Err(IndexError::InvalidInput("duplicate adapter kind".into()));
        }
        match self.owners.entry(descriptor.kind) {
            std::collections::btree_map::Entry::Occupied(_) => {
                Err(IndexError::InvalidInput("duplicate adapter kind".into()))
            }
            std::collections::btree_map::Entry::Vacant(slot) => {
                let _registered = slot.insert(Arc::new(OwnerAdapter {
                    descriptor,
                    resource_demand: backend.resource_demand(),
                    backend,
                }));
                Ok(())
            }
        }
    }

    pub fn register_projection<A: IndexProjectionBuildPort + 'static>(
        &mut self,
        descriptor: AdapterDescriptor,
        backend: A,
    ) -> Result<(), IndexError> {
        if self.core.contains_key(&descriptor.kind)
            || self.owners.contains_key(&descriptor.kind)
            || self.projection.contains_key(&descriptor.kind)
        {
            return Err(IndexError::InvalidInput("duplicate adapter kind".into()));
        }
        let adapter = ProjectionAdapter {
            descriptor,
            resource_demand: backend.resource_demand(),
            backend,
        };
        let _registered = self.projection.insert(descriptor.kind, Arc::new(adapter));
        Ok(())
    }

    pub(crate) fn build_projection<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<MemberBinding>,
        projection: &'a ProjectionBinding,
        affected: Option<OwnerMutationSet>,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<BuildFuture<'a, MemberBinding>, IndexError> {
        match self.projection.get(&instance.kind) {
            Some(adapter) if self.supports(instance) => {
                adapter.build(instance, prior, projection, affected, attempt)
            }
            _ => Err(IndexError::InvalidInput(
                "unregistered projection adapter".into(),
            )),
        }
    }

    pub(crate) fn supports(&self, instance: &InstanceSpec) -> bool {
        let descriptor = match instance.activation {
            GroupActivation::CoreRequired
                if matches!(instance.input_domain, InputDomain::Projection { .. }) =>
            {
                self.projection
                    .get(&instance.kind)
                    .map(|adapter| adapter.descriptor())
            }
            GroupActivation::CoreRequired => self
                .core
                .get(&instance.kind)
                .map(|adapter| adapter.descriptor()),
            GroupActivation::AsyncCoverage => self
                .owners
                .get(&instance.kind)
                .map(|adapter| adapter.descriptor()),
        };
        descriptor.is_some_and(|registered| {
            registered.schema == instance.schema && registered.format == instance.format
        })
    }

    pub(crate) fn build_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        prior: Option<OwnerMemberBinding>,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<BuildFuture<'a, OwnerMemberBinding>, IndexError> {
        match self.owners.get(&instance.kind) {
            Some(adapter) if self.supports(instance) => {
                adapter.build(instance, owner, input, prior, attempt)
            }
            _ => Err(IndexError::InvalidInput(
                "unregistered owner index descriptor".into(),
            )),
        }
    }

    pub(crate) fn prepare_searchable<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<&'a CoverageBinding>,
        owners: MapRoot,
        materialized: Option<(OwnerKey, ObjectRef)>,
        object_maps: Option<&'a Arc<dyn ObjectMapBatchPort>>,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<IndexFuture<'a, quanta_index_core::index::SearchablePreparation>, IndexError> {
        match self.owners.get(&instance.kind) {
            Some(adapter) if self.supports(instance) => Ok(adapter.prepare_searchable(
                instance,
                prior,
                owners,
                materialized,
                object_maps,
                attempt,
            )),
            _ => Err(IndexError::InvalidInput(
                "unregistered owner index descriptor".into(),
            )),
        }
    }

    pub(crate) fn validate_coverage(
        &self,
        instance: &InstanceSpec,
        coverage: &CoverageBinding,
    ) -> Result<(), IndexError> {
        match self.owners.get(&instance.kind) {
            Some(adapter) if self.supports(instance) => adapter.validate_coverage(coverage),
            _ => Err(IndexError::InvalidInput(
                "unregistered owner index descriptor".into(),
            )),
        }
    }

    pub(crate) fn build<'a>(
        &'a self,
        instance: &InstanceSpec,
        base: BaseMember,
        delta: Option<&'a SourceDelta>,
        source: SourceCut,
        input: InputBinding,
        attempt: &'a mut AttemptContext<'_>,
    ) -> Result<BuildFuture<'a, MemberBinding>, IndexError> {
        match self.core.get(&instance.kind) {
            Some(adapter) if self.supports(instance) => adapter.build(
                base,
                delta,
                PlanContext {
                    instance: instance.clone(),
                    source,
                    input,
                },
                attempt,
            ),
            _ => Err(IndexError::InvalidInput(
                "unregistered index descriptor".into(),
            )),
        }
    }

    pub(crate) fn validate_profile(
        &self,
        instances: &[InstanceSpec],
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        // One bounded shape validator owns its temporary backing. Invalid
        // identities and activation groups refuse before adapter validation.
        crate::profile::validate_instances(instances, budget)?;
        for instance in instances {
            budget.checkpoint()?;
            if !self.supports(instance) {
                return Err(IndexError::InvalidInput(
                    "unregistered index descriptor".into(),
                ));
            }
            match instance.activation {
                GroupActivation::CoreRequired
                    if matches!(instance.input_domain, InputDomain::Projection { .. }) =>
                {
                    self.projection
                        .get(&instance.kind)
                        .ok_or_else(|| {
                            IndexError::InvalidInput("unregistered projection adapter".into())
                        })?
                        .validate_instance(instance)?;
                }
                GroupActivation::CoreRequired => self
                    .core
                    .get(&instance.kind)
                    .ok_or_else(|| IndexError::InvalidInput("unregistered adapter".into()))?
                    .validate_instance(instance)?,
                GroupActivation::AsyncCoverage => self
                    .owners
                    .get(&instance.kind)
                    .ok_or_else(|| IndexError::InvalidInput("unregistered owner adapter".into()))?
                    .validate_instance(instance)?,
            }
        }
        budget.checkpoint()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use quanta_index_core::index::{
        AttemptId, AttemptRef, Digest, IndexInstanceId, MapEdit, MapPage, MapRoot, MapWrite,
        ObjectRef, OperationKey, OwnerMutationSet, RegisteredObject, SourceCut, SourceDelta,
        StoredObject, WorkBudget, WorkLimits, WorkspaceKey,
        ports::{ImmutableObjectPort, ObjectCustodyPort, PersistentMapPort},
    };

    use super::*;
    use quanta_index_core::index::ports::IndexFuture;

    macro_rules! check {
        ($condition:expr) => {{
            if $condition {
                Ok::<(), IndexError>(())
            } else {
                Err(IndexError::BackendViolation(format!(
                    "registry test check failed: {}",
                    stringify!($condition)
                )))
            }
        }};
    }

    macro_rules! check_eq {
        ($actual:expr, $expected:expr) => {{
            let actual = &$actual;
            let expected = &$expected;
            if actual == expected {
                Ok::<(), IndexError>(())
            } else {
                Err(IndexError::BackendViolation(format!(
                    "registry test {} == {}: actual={actual:?}, expected={expected:?}",
                    stringify!($actual),
                    stringify!($expected)
                )))
            }
        }};
    }

    pub(crate) struct NoIo;

    impl ImmutableObjectPort for NoIo {
        fn install_batch(
            &self,
            _: &[quanta_index_core::index::ObjectBatchInput],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test batch install".into(),
            ))
        }
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test identification".into(),
            ))
        }
        fn put(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected test IO".into()))
        }
        fn install(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected test IO".into()))
        }
        fn get(&self, _: ObjectRef, _: &mut WorkBudget) -> Result<StoredObject, IndexError> {
            Err(IndexError::BackendViolation("unexpected test IO".into()))
        }
    }
    impl PersistentMapPort for NoIo {
        fn apply_stream(
            &self,
            _: &[MapRoot],
            _: &mut dyn quanta_index_core::index::ports::MapEditCursor,
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test stream map write".into(),
            ))
        }
        fn apply_batch(
            &self,
            _: &[quanta_index_core::index::MapBatchRequest],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::MapBatchWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test batch map write".into(),
            ))
        }
        fn lookup_object(
            &self,
            _: MapRoot,
            _: &[u8],
            _: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            Err(IndexError::BackendViolation("unexpected test IO".into()))
        }
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<quanta_index_core::index::MapValue>, IndexError> {
            self.lookup_object(root, key, budget)
                .map(|value| value.map(Into::into))
        }

        fn scan(
            &self,
            _: MapRoot,
            _: Option<&[u8]>,
            _: u32,
            _: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            Err(IndexError::BackendViolation("unexpected test IO".into()))
        }
        fn apply(
            &self,
            _: MapRoot,
            _: &[MapEdit],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            Err(IndexError::BackendViolation("unexpected test IO".into()))
        }
    }
    impl ObjectCustodyPort for NoIo {
        fn reserve_objects(&self, _: AttemptRef, _: &[RegisteredObject]) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation("unexpected test IO".into()))
        }
        fn register_objects(
            &self,
            _: AttemptRef,
            _: &[RegisteredObject],
        ) -> Result<(), IndexError> {
            Err(IndexError::BackendViolation("unexpected test IO".into()))
        }
    }

    impl quanta_index_core::index::ScratchSpacePort for NoIo {
        fn allocate(
            &self,
            _: quanta_index_core::index::ScratchRequest,
            _: &mut WorkBudget,
        ) -> Result<Box<dyn quanta_index_core::index::ScratchAllocation>, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test scratch".into(),
            ))
        }
    }

    struct NumberBackend;
    impl IndexBuildPort for NumberBackend {
        type Delta = u64;
        type Plan = MemberBinding;
        type Candidate = MemberBinding;
        fn resource_demand(&self) -> ResourceNeed {
            ResourceNeed::default()
        }
        fn validate_instance(
            &self,
            instance: &quanta_index_core::index::InstanceSpec,
        ) -> Result<(), IndexError> {
            if instance.policy == Digest([0; 32]) {
                return Err(IndexError::InvalidInput(
                    "test adapter refuses zero policy".into(),
                ));
            }
            Ok(())
        }

        fn plan(
            &self,
            _: BaseMember,
            delta: DeltaInput<u64>,
            context: &PlanContext,
            _: &mut WorkBudget,
        ) -> Result<PlanDecision<Self::Plan>, IndexError> {
            let DeltaInput::Patch(number) = delta else {
                return Err(IndexError::InvalidInput("test expects patch".into()));
            };
            let byte = u8::try_from(number).map_err(|_conversion_error| {
                IndexError::InvalidInput("test number exceeds u8".into())
            })?;
            Ok(PlanDecision::Incremental(MemberBinding {
                instance: context.instance.instance,
                format: context.instance.format,
                artifact: ObjectRef(Digest([byte; 32])),
                input: context.input,
                logical_commitment: Digest([byte; 32]),
            }))
        }
        fn prepare<'a>(
            &'a self,
            plan: Self::Plan,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, Self::Candidate> {
            Box::pin(async move { Ok(plan) })
        }
        fn verify_and_seal<'a>(
            &'a self,
            candidate: Self::Candidate,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, MemberBinding> {
            Box::pin(async move { Ok(candidate) })
        }
    }

    struct TextBackend;
    impl IndexBuildPort for TextBackend {
        type Delta = String;
        type Plan = MemberBinding;
        type Candidate = MemberBinding;
        fn resource_demand(&self) -> ResourceNeed {
            ResourceNeed::default()
        }
        fn validate_instance(
            &self,
            instance: &quanta_index_core::index::InstanceSpec,
        ) -> Result<(), IndexError> {
            if instance.policy == Digest([0; 32]) {
                return Err(IndexError::InvalidInput(
                    "test adapter refuses zero policy".into(),
                ));
            }
            Ok(())
        }

        fn plan(
            &self,
            _: BaseMember,
            delta: DeltaInput<String>,
            context: &PlanContext,
            _: &mut WorkBudget,
        ) -> Result<PlanDecision<Self::Plan>, IndexError> {
            let DeltaInput::Patch(text) = delta else {
                return Err(IndexError::InvalidInput("test expects patch".into()));
            };
            let byte = u8::try_from(text.len()).map_err(|_conversion_error| {
                IndexError::InvalidInput("test text exceeds u8 bytes".into())
            })?;
            Ok(PlanDecision::Incremental(MemberBinding {
                instance: context.instance.instance,
                format: context.instance.format,
                artifact: ObjectRef(Digest([byte; 32])),
                input: context.input,
                logical_commitment: Digest([byte; 32]),
            }))
        }
        fn prepare<'a>(
            &'a self,
            plan: Self::Plan,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, Self::Candidate> {
            Box::pin(async move { Ok(plan) })
        }
        fn verify_and_seal<'a>(
            &'a self,
            candidate: Self::Candidate,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, MemberBinding> {
            Box::pin(async move { Ok(candidate) })
        }
    }

    struct OwnerBackend;
    impl IndexOwnerBuildPort for OwnerBackend {
        type Candidate = OwnerMemberBinding;

        fn resource_demand(&self) -> ResourceNeed {
            ResourceNeed::default()
        }

        fn validate_instance(
            &self,
            instance: &quanta_index_core::index::InstanceSpec,
        ) -> Result<(), IndexError> {
            if instance.policy == Digest([0; 32]) {
                return Err(IndexError::InvalidInput(
                    "test adapter refuses zero policy".into(),
                ));
            }
            Ok(())
        }

        fn prepare_owner<'a>(
            &'a self,
            instance: &'a InstanceSpec,
            owner: OwnerKey,
            input: OwnerInputBinding,
            _prior: Option<OwnerMemberBinding>,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, Self::Candidate> {
            Box::pin(async move {
                Ok(OwnerMemberBinding {
                    instance: instance.instance,
                    owner,
                    input,
                    format: instance.format,
                    artifact: ObjectRef(Digest([11; 32])),
                    logical_commitment: Digest([12; 32]),
                })
            })
        }

        fn verify_owner<'a>(
            &'a self,
            candidate: Self::Candidate,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, OwnerMemberBinding> {
            Box::pin(async move { Ok(candidate) })
        }
    }

    struct DriftingBuild {
        demand: Arc<AtomicU64>,
        prepare_calls: Arc<AtomicU64>,
        drift_during_prepare: bool,
    }

    impl IndexBuildPort for DriftingBuild {
        type Delta = ();
        type Plan = MemberBinding;
        type Candidate = MemberBinding;

        fn resource_demand(&self) -> ResourceNeed {
            ResourceNeed {
                provider_units: self.demand.load(Ordering::SeqCst),
                ..ResourceNeed::default()
            }
        }

        fn validate_instance(&self, _: &InstanceSpec) -> Result<(), IndexError> {
            Ok(())
        }

        fn plan(
            &self,
            _: BaseMember,
            _: DeltaInput<Self::Delta>,
            context: &PlanContext,
            _: &mut WorkBudget,
        ) -> Result<PlanDecision<Self::Plan>, IndexError> {
            Ok(PlanDecision::Incremental(MemberBinding {
                instance: context.instance.instance,
                format: context.instance.format,
                artifact: ObjectRef(Digest([10; 32])),
                input: context.input,
                logical_commitment: Digest([11; 32]),
            }))
        }

        fn prepare<'a>(
            &'a self,
            plan: Self::Plan,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, Self::Candidate> {
            Box::pin(async move {
                let _prior_calls = self.prepare_calls.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                if self.drift_during_prepare {
                    self.demand.store(1, Ordering::SeqCst);
                }
                Ok(plan)
            })
        }

        fn verify_and_seal<'a>(
            &'a self,
            candidate: Self::Candidate,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, MemberBinding> {
            Box::pin(async move { Ok(candidate) })
        }
    }

    struct DriftingOwner {
        demand: Arc<AtomicU64>,
        prepare_calls: Arc<AtomicU64>,
        drift_during_prepare: bool,
    }

    impl IndexOwnerBuildPort for DriftingOwner {
        type Candidate = OwnerMemberBinding;

        fn resource_demand(&self) -> ResourceNeed {
            ResourceNeed {
                provider_units: self.demand.load(Ordering::SeqCst),
                ..ResourceNeed::default()
            }
        }

        fn validate_instance(&self, _: &InstanceSpec) -> Result<(), IndexError> {
            Ok(())
        }

        fn prepare_owner<'a>(
            &'a self,
            instance: &'a InstanceSpec,
            owner: OwnerKey,
            input: OwnerInputBinding,
            _: Option<OwnerMemberBinding>,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, Self::Candidate> {
            Box::pin(async move {
                let _prior_calls = self.prepare_calls.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                if self.drift_during_prepare {
                    self.demand.store(1, Ordering::SeqCst);
                }
                Ok(OwnerMemberBinding {
                    instance: instance.instance,
                    owner,
                    input,
                    format: instance.format,
                    artifact: ObjectRef(Digest([12; 32])),
                    logical_commitment: Digest([13; 32]),
                })
            })
        }

        fn verify_owner<'a>(
            &'a self,
            candidate: Self::Candidate,
            _: &'a mut AttemptContext<'_>,
        ) -> IndexFuture<'a, OwnerMemberBinding> {
            Box::pin(async move { Ok(candidate) })
        }
    }

    pub(crate) fn demand_test_context<'a>(
        io: &'a NoIo,
        budget: &'a mut WorkBudget,
    ) -> AttemptContext<'a> {
        AttemptContext {
            attempt: AttemptRef {
                operation: OperationKey {
                    workspace: WorkspaceKey {
                        tenant: quanta_index_core::index::TenantId(id(1)),
                        collection: quanta_index_core::index::CollectionId(id(1)),
                        worktree: quanta_index_core::index::WorktreeId(id(1)),
                        slot: quanta_index_core::index::SlotId(id(1)),
                    },
                    epoch: 1,
                    producer: quanta_index_core::index::ProducerId(id(1)),
                    stream: quanta_index_core::index::StreamId(id(1)),
                    stream_epoch: 1,
                    sequence: 1,
                },
                attempt: AttemptId(id(1)),
                fence: 1,
            },
            objects: io,
            maps: io,
            custody: io,
            scratch: io,
            budget,
        }
    }

    fn demand_test_budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 8,
                writes: 8,
                bytes: 1024,
                mutations: 8,
            },
            Instant::now()
                .checked_add(Duration::from_secs(2))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn build_demand_is_frozen_before_execution_and_after_await() -> Result<(), IndexError> {
        let mut pool = futures::executor::LocalPool::new();
        pool.run_until(async {
            for drift_during_prepare in [false, true] {
                let demand = Arc::new(AtomicU64::new(0));
                let prepare_calls = Arc::new(AtomicU64::new(0));
                let mut registry = AdapterRegistry::new();
                registry.register(
                    descriptor(10),
                    DriftingBuild {
                        demand: Arc::clone(&demand),
                        prepare_calls: Arc::clone(&prepare_calls),
                        drift_during_prepare,
                    },
                    |_delta: &SourceDelta| Ok(()),
                )?;
                check_eq!(registry.resource_demands().0.provider_units, 0)?;
                if !drift_during_prepare {
                    demand.store(1, Ordering::SeqCst);
                }
                let instance = spec(10, 10);
                let source = SourceCut {
                    owners: MapRoot(None),
                    paths: MapRoot(None),
                    frontiers: MapRoot(None),
                };
                let input = InputBinding {
                    root: source.owners,
                    schema: instance.schema,
                    policy: instance.policy,
                    context: Digest([0; 32]),
                };
                let io = NoIo;
                let mut budget = demand_test_budget()?;
                let mut context = demand_test_context(&io, &mut budget);
                let result = registry
                    .build(
                        &instance,
                        BaseMember::Absent,
                        None,
                        source,
                        input,
                        &mut context,
                    )?
                    .await;
                let expected = if drift_during_prepare {
                    "build adapter resource demand changed during execution"
                } else {
                    "build adapter resource demand changed after registration"
                };
                check_eq!(result, Err(IndexError::BackendViolation(expected.into())))?;
                check_eq!(
                    prepare_calls.load(Ordering::SeqCst),
                    u64::from(drift_during_prepare)
                )?;
                check_eq!(registry.resource_demands().0.provider_units, 0)?;
            }
            Ok(())
        })
    }

    #[test]
    fn owner_demand_is_frozen_before_execution_and_after_await() -> Result<(), IndexError> {
        let mut pool = futures::executor::LocalPool::new();
        pool.run_until(async {
            for drift_during_prepare in [false, true] {
                let demand = Arc::new(AtomicU64::new(0));
                let prepare_calls = Arc::new(AtomicU64::new(0));
                let mut registry = AdapterRegistry::new();
                registry.register_owner(
                    descriptor(11),
                    DriftingOwner {
                        demand: Arc::clone(&demand),
                        prepare_calls: Arc::clone(&prepare_calls),
                        drift_during_prepare,
                    },
                )?;
                check_eq!(registry.resource_demands().1.provider_units, 0)?;
                if !drift_during_prepare {
                    demand.store(1, Ordering::SeqCst);
                }
                let mut instance = spec(11, 11);
                instance.activation = GroupActivation::AsyncCoverage;
                let owner = OwnerKey {
                    producer: quanta_index_core::index::ProducerId(id(2)),
                    stable_id: quanta_index_core::index::StableOwnerId(id(3)),
                };
                let input = OwnerInputBinding {
                    source: None,
                    projection: None,
                    producer_context: Digest([0; 32]),
                    derivation_policy: instance.policy,
                };
                let io = NoIo;
                let mut budget = demand_test_budget()?;
                let mut context = demand_test_context(&io, &mut budget);
                let result = registry
                    .build_owner(&instance, owner, input, None, &mut context)?
                    .await;
                let expected = if drift_during_prepare {
                    "owner adapter resource demand changed during execution"
                } else {
                    "owner adapter resource demand changed after registration"
                };
                check_eq!(result, Err(IndexError::BackendViolation(expected.into())))?;
                check_eq!(
                    prepare_calls.load(Ordering::SeqCst),
                    u64::from(drift_during_prepare)
                )?;
                check_eq!(registry.resource_demands().1.provider_units, 0)?;
            }
            Ok(())
        })
    }

    #[test]
    fn profile_admission_checks_adapter_policy_without_owner_or_build() -> Result<(), IndexError> {
        let mut registry = AdapterRegistry::new();
        registry.register(descriptor(1), NumberBackend, |_delta| Ok(1))?;
        registry.register_owner(descriptor(3), OwnerBackend)?;
        let mut core = spec(1, 1);
        registry.validate_profile(&[core.clone()], &demand_test_budget()?)?;
        core.policy = Digest([0; 32]);
        check!(matches!(
            registry.validate_profile(&[core], &demand_test_budget()?),
            Err(IndexError::InvalidInput(_))
        ))?;
        let mut owner = spec(3, 3);
        owner.activation = GroupActivation::AsyncCoverage;
        registry.validate_profile(&[owner.clone()], &demand_test_budget()?)?;
        owner.policy = Digest([0; 32]);
        check!(matches!(
            registry.validate_profile(&[owner], &demand_test_budget()?),
            Err(IndexError::InvalidInput(_))
        ))?;
        Ok(())
    }

    #[test]
    fn profile_validation_admits_shape_before_policy_and_refunds_original_budget()
    -> Result<(), IndexError> {
        use crate::staging_payload::tests::{require, scoped_budget};

        let registry = registered_number()?;
        let instances = [spec(1, 4), spec(1, 3)];
        let floor = u64::try_from(std::mem::size_of::<[&InstanceSpec; 2]>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let (denied, refused_ledger) = scoped_budget(floor - 1)?;
        require(matches!(
            registry.validate_profile(&instances, &denied),
            Err(IndexError::ResourceLimited)
        ))?;
        require(refused_ledger.live() == 0)?;
        let (budget, ledger) = scoped_budget(floor)?;
        registry.validate_profile(&instances, &budget)?;
        require(ledger.live() == 0)?;

        let mut invalid_policy = spec(1, 3);
        invalid_policy.policy = Digest([0; 32]);
        let (unfunded, unfunded_ledger) = scoped_budget(0)?;
        require(matches!(
            registry.validate_profile(&[invalid_policy.clone()], &unfunded),
            Err(IndexError::ResourceLimited)
        ))?;
        require(unfunded_ledger.live() == 0)?;
        require(matches!(
            registry.validate_profile(&[invalid_policy], &budget),
            Err(IndexError::InvalidInput(_))
        ))?;
        require(ledger.live() == 0)?;

        let oversized = vec![spec(1, 3); crate::profile::MAX_PROFILE_INSTANCES + 1];
        require(matches!(
            registry.validate_profile(&oversized, &unfunded),
            Err(IndexError::ResourceLimited)
        ))?;
        require(unfunded_ledger.live() == 0)?;
        registry.validate_profile(&[], &unfunded)?;
        budget.request().cancel_handle().cancel();
        require(matches!(
            registry.validate_profile(&[], &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(matches!(
            registry.validate_profile(&instances, &budget),
            Err(IndexError::Cancelled)
        ))?;
        require(ledger.live() == 0)
    }

    #[test]
    fn delta_decoder_runs_only_inside_admitted_original_control() -> Result<(), IndexError> {
        use crate::staging_payload::tests::{require, scoped_budget};

        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let delta = SourceDelta {
            prior: source,
            next: source,
            changed: OwnerMutationSet {
                root: MapRoot(Some(ObjectRef(Digest([8; 32])))),
                count: 1,
                digest: Digest([9; 32]),
            },
        };
        let instance = spec(1, 3);
        let input = InputBinding {
            root: source.owners,
            schema: instance.schema,
            policy: instance.policy,
            context: Digest([4; 32]),
        };
        let io = NoIo;
        let mut pool = futures::executor::LocalPool::new();
        // Refused grant, dropped before polling, cancelled before polling,
        // successful execution, cancellation inside the decoder, and its error.
        for case in 0..6 {
            let (mut budget, ledger) = scoped_budget(if case == 0 { 0 } else { u64::MAX })?;
            let control = budget.request().clone();
            let decoder_control = control.clone();
            let calls = Arc::new(AtomicU64::new(0));
            let decoder_calls = Arc::clone(&calls);
            let mut registry = AdapterRegistry::new();
            registry.register(descriptor(1), NumberBackend, move |_delta| {
                let _previous = decoder_calls.fetch_add(1, Ordering::SeqCst);
                if case == 4 {
                    decoder_control.cancel_handle().cancel();
                }
                if case == 5 {
                    Err(IndexError::InvalidInput("decoder refused input".into()))
                } else {
                    Ok(1)
                }
            })?;
            let mut context = demand_test_context(&io, &mut budget);
            let result = registry.build(
                &instance,
                BaseMember::Absent,
                Some(&delta),
                source,
                input,
                &mut context,
            );
            require(calls.load(Ordering::SeqCst) == 0)?;
            if case == 0 {
                require(matches!(result, Err(IndexError::ResourceLimited)))?;
            } else {
                let future = result?;
                require(ledger.live() > 0)?;
                if case == 1 {
                    drop(future);
                } else {
                    if case == 2 {
                        control.cancel_handle().cancel();
                    }
                    let outcome = pool.run_until(future);
                    if case == 2 || case == 4 {
                        require(matches!(outcome, Err(IndexError::Cancelled)))?;
                    } else if case == 5 {
                        require(matches!(outcome,
                            Err(IndexError::InvalidInput(message)) if message == "decoder refused input"
                        ))?;
                    } else {
                        require(outcome?.artifact == ObjectRef(Digest([1; 32])))?;
                    }
                }
            }
            require(calls.load(Ordering::SeqCst) == u64::from(case >= 3))?;
            require(ledger.live() == 0)?;
        }
        Ok(())
    }

    fn id(byte: u8) -> [u8; 16] {
        [byte; 16]
    }
    fn descriptor(byte: u8) -> AdapterDescriptor {
        AdapterDescriptor {
            kind: IndexKind(id(byte)),
            schema: SchemaId(id(byte)),
            format: StorageFormatId(id(byte)),
        }
    }
    fn spec(byte: u8, instance: u8) -> InstanceSpec {
        let descriptor = descriptor(byte);
        InstanceSpec {
            instance: IndexInstanceId(id(instance)),
            kind: descriptor.kind,
            group: quanta_index_core::index::PublicationGroupId(id(byte)),
            activation: quanta_index_core::index::GroupActivation::CoreRequired,
            input_domain: InputDomain::SourceLocal,
            schema: descriptor.schema,
            policy: Digest([byte; 32]),
            format: descriptor.format,
        }
    }

    fn registered_number() -> Result<AdapterRegistry, IndexError> {
        let mut registry = AdapterRegistry::new();
        registry.register(descriptor(1), NumberBackend, |delta: &SourceDelta| {
            Ok(delta.changed.count)
        })?;
        Ok(registry)
    }

    #[test]
    fn profile_refuses_unregistered_kind() -> Result<(), IndexError> {
        let registry = registered_number()?;
        check_eq!(
            registry.validate_profile(&[spec(2, 3)], &demand_test_budget()?),
            Err(IndexError::InvalidInput(
                "unregistered index descriptor".into()
            ))
        )?;
        Ok(())
    }

    #[test]
    fn profile_refuses_schema_drift() -> Result<(), IndexError> {
        let registry = registered_number()?;
        let mut changed = spec(1, 3);
        changed.schema = SchemaId(id(9));
        check_eq!(
            registry.validate_profile(&[changed], &demand_test_budget()?),
            Err(IndexError::InvalidInput(
                "unregistered index descriptor".into()
            ))
        )?;
        Ok(())
    }

    #[test]
    fn profile_refuses_format_drift() -> Result<(), IndexError> {
        let registry = registered_number()?;
        let mut changed = spec(1, 3);
        changed.format = StorageFormatId(id(9));
        check_eq!(
            registry.validate_profile(&[changed], &demand_test_budget()?),
            Err(IndexError::InvalidInput(
                "unregistered index descriptor".into()
            ))
        )?;
        Ok(())
    }

    #[test]
    fn profile_refuses_duplicate_instance_identity() -> Result<(), IndexError> {
        let registry = registered_number()?;
        check_eq!(
            registry.validate_profile(&[spec(1, 3), spec(1, 3)], &demand_test_budget()?),
            Err(IndexError::InvalidInput("duplicate index instance".into()))
        )?;
        Ok(())
    }

    #[test]
    fn profile_refuses_unregistered_required_projection_domain() -> Result<(), IndexError> {
        let registry = registered_number()?;
        let mut changed = spec(1, 3);
        changed.input_domain = InputDomain::Projection {
            projection: quanta_index_core::index::ProjectionId(id(4)),
            producer: quanta_index_core::index::ProducerId(id(5)),
        };
        check_eq!(
            registry.validate_profile(&[changed], &demand_test_budget()?),
            Err(IndexError::InvalidInput(
                "unregistered index descriptor".into()
            ))
        )?;
        Ok(())
    }

    #[test]
    fn distinct_typed_deltas_and_two_instances_of_one_kind() -> Result<(), IndexError> {
        let mut pool = futures::executor::LocalPool::new();
        pool.run_until(async {
            let mut registry = AdapterRegistry::new();
            registry.register(descriptor(1), NumberBackend, |delta: &SourceDelta| {
                Ok(delta.changed.count)
            })?;
            registry.register(descriptor(2), TextBackend, |_delta: &SourceDelta| {
                Ok(String::from("text"))
            })?;
            let specs = vec![spec(1, 3), spec(1, 4), spec(2, 5)];
            registry.validate_profile(&specs, &demand_test_budget()?)?;
            let source = SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            };
            let delta = SourceDelta {
                prior: source,
                next: source,
                changed: OwnerMutationSet {
                    root: MapRoot(Some(ObjectRef(Digest([8; 32])))),
                    count: 1,
                    digest: Digest([9; 32]),
                },
            };
            let io = NoIo;
            let mut budget = WorkBudget::new(
                WorkLimits {
                    reads: 8,
                    writes: 8,
                    bytes: 1024,
                    mutations: 8,
                },
                Instant::now()
                    .checked_add(Duration::from_secs(2))
                    .ok_or(IndexError::ResourceLimited)?,
            );
            let attempt = AttemptRef {
                operation: OperationKey {
                    workspace: WorkspaceKey {
                        tenant: quanta_index_core::index::TenantId(id(1)),
                        collection: quanta_index_core::index::CollectionId(id(1)),
                        worktree: quanta_index_core::index::WorktreeId(id(1)),
                        slot: quanta_index_core::index::SlotId(id(1)),
                    },
                    epoch: 1,
                    producer: quanta_index_core::index::ProducerId(id(1)),
                    stream: quanta_index_core::index::StreamId(id(1)),
                    stream_epoch: 1,
                    sequence: 1,
                },
                attempt: AttemptId(id(1)),
                fence: 1,
            };
            let scratch_directory = tempfile::tempdir().map_err(|error| {
                IndexError::Storage(format!("create registry test scratch: {error}"))
            })?;
            let scratch = quanta_index_storage::FileScratchSpace::open_standalone(
                scratch_directory.path().join("scratch"),
                16 * 1024 * 1024 * 1024,
            )?;
            let mut context = AttemptContext {
                attempt,
                objects: &io,
                maps: &io,
                custody: &io,
                scratch: &scratch,
                budget: &mut budget,
            };
            for spec in specs {
                let input = InputBinding {
                    root: source.owners,
                    schema: spec.schema,
                    policy: spec.policy,
                    context: Digest([0; 32]),
                };
                let binding = registry
                    .build(
                        &spec,
                        BaseMember::Absent,
                        Some(&delta),
                        source,
                        input,
                        &mut context,
                    )?
                    .await?;
                check_eq!(binding.instance, spec.instance)?;
                check_eq!(binding.input, input)?;
                check_eq!(
                    binding
                        .artifact
                        .0
                        .0
                        .first()
                        .copied()
                        .ok_or_else(|| IndexError::BackendViolation("empty test digest".into()))?,
                    if spec.kind == descriptor(1).kind {
                        1
                    } else {
                        4
                    }
                )?;
            }
            registry.register_owner(descriptor(3), OwnerBackend)?;
            let mut async_spec = spec(3, 6);
            async_spec.activation = GroupActivation::AsyncCoverage;
            registry.validate_profile(&[async_spec.clone()], &demand_test_budget()?)?;
            let owner = OwnerKey {
                producer: quanta_index_core::index::ProducerId(id(6)),
                stable_id: quanta_index_core::index::StableOwnerId(id(7)),
            };
            let owner_input = OwnerInputBinding {
                source: Some(ObjectRef(Digest([8; 32]))),
                projection: None,
                producer_context: Digest([0; 32]),
                derivation_policy: async_spec.policy,
            };
            let owner_binding = registry
                .build_owner(&async_spec, owner, owner_input, None, &mut context)?
                .await?;
            check_eq!(owner_binding.instance, async_spec.instance)?;
            check_eq!(owner_binding.input, owner_input)?;
            check!(
                registry
                    .register_owner(descriptor(1), OwnerBackend)
                    .is_err()
            )?;
            check!(
                registry
                    .register(descriptor(3), NumberBackend, |_delta: &SourceDelta| Ok(0))
                    .is_err()
            )?;
            check!(
                registry
                    .register(descriptor(1), NumberBackend, |_delta: &SourceDelta| Ok(0))
                    .is_err()
            )?;
            Ok(())
        })
    }
}
