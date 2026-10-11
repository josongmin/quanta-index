use std::{collections::BTreeMap, sync::Arc};

use quanta_index_core::index::{
    AttemptContext, BaseMember, DeltaInput, GroupActivation, IndexError, IndexInstanceId,
    IndexKind, InputBinding, InputDomain, InstanceSpec, MemberBinding, OwnerInputBinding, OwnerKey,
    OwnerMemberBinding, PlanContext, PlanDecision, SchemaId, SourceCut, SourceDelta,
    StorageFormatId,
    ports::{IndexBuildPort, IndexFuture, IndexOwnerBuildPort},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterDescriptor {
    pub kind: IndexKind,
    pub schema: SchemaId,
    pub format: StorageFormatId,
}

trait ErasedAdapter: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    fn build<'a>(
        &'a self,
        base: BaseMember,
        delta: Option<&SourceDelta>,
        plan: PlanContext,
        attempt: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding>;
}

struct Adapter<A, F> {
    descriptor: AdapterDescriptor,
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

    fn build<'a>(
        &'a self,
        base: BaseMember,
        delta: Option<&SourceDelta>,
        plan: PlanContext,
        attempt: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        let input = delta.map_or_else(
            || Ok(DeltaInput::Bootstrap),
            |delta| (self.decode_delta)(delta).map(DeltaInput::Patch),
        );
        Box::pin(async move {
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

trait ErasedOwnerAdapter: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    fn build<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        attempt: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding>;
}

struct OwnerAdapter<A> {
    descriptor: AdapterDescriptor,
    backend: A,
}

impl<A: IndexOwnerBuildPort + 'static> ErasedOwnerAdapter for OwnerAdapter<A> {
    fn descriptor(&self) -> AdapterDescriptor {
        self.descriptor
    }

    fn build<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        attempt: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding> {
        Box::pin(async move {
            let candidate = self
                .backend
                .prepare_owner(instance, owner, input, attempt)
                .await?;
            let binding = self.backend.verify_owner(candidate, attempt).await?;
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
    by_kind: BTreeMap<IndexKind, Arc<dyn ErasedAdapter>>,
    owner_by_kind: BTreeMap<IndexKind, Arc<dyn ErasedOwnerAdapter>>,
}

impl AdapterRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
        if self.owner_by_kind.contains_key(&descriptor.kind) {
            return Err(IndexError::InvalidInput("duplicate adapter kind".into()));
        }
        match self.by_kind.entry(descriptor.kind) {
            std::collections::btree_map::Entry::Occupied(_) => {
                Err(IndexError::InvalidInput("duplicate adapter kind".into()))
            }
            std::collections::btree_map::Entry::Vacant(slot) => {
                let _registered = slot.insert(Arc::new(Adapter {
                    descriptor,
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
        if self.by_kind.contains_key(&descriptor.kind) {
            return Err(IndexError::InvalidInput("duplicate adapter kind".into()));
        }
        match self.owner_by_kind.entry(descriptor.kind) {
            std::collections::btree_map::Entry::Occupied(_) => {
                Err(IndexError::InvalidInput("duplicate adapter kind".into()))
            }
            std::collections::btree_map::Entry::Vacant(slot) => {
                let _registered = slot.insert(Arc::new(OwnerAdapter {
                    descriptor,
                    backend,
                }));
                Ok(())
            }
        }
    }

    pub(crate) fn supports(&self, instance: &InstanceSpec) -> bool {
        let descriptor = match instance.activation {
            GroupActivation::CoreRequired => self
                .by_kind
                .get(&instance.kind)
                .map(|adapter| adapter.descriptor()),
            GroupActivation::AsyncCoverage => self
                .owner_by_kind
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
        attempt: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding> {
        match self.owner_by_kind.get(&instance.kind) {
            Some(adapter) if self.supports(instance) => {
                adapter.build(instance, owner, input, attempt)
            }
            _ => Box::pin(async {
                Err(IndexError::InvalidInput(
                    "unregistered owner index descriptor".into(),
                ))
            }),
        }
    }

    pub(crate) fn build<'a>(
        &'a self,
        instance: &InstanceSpec,
        base: BaseMember,
        delta: Option<&SourceDelta>,
        source: SourceCut,
        input: InputBinding,
        attempt: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        match self.by_kind.get(&instance.kind) {
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
            _ => Box::pin(async {
                Err(IndexError::InvalidInput(
                    "unregistered index descriptor".into(),
                ))
            }),
        }
    }

    pub(crate) fn validate_profile(&self, instances: &[InstanceSpec]) -> Result<(), IndexError> {
        let mut ids = std::collections::BTreeSet::<IndexInstanceId>::new();
        for instance in instances {
            if !ids.insert(instance.instance) {
                return Err(IndexError::InvalidInput("duplicate index instance".into()));
            }
            if !self.supports(instance) {
                return Err(IndexError::InvalidInput(
                    "unregistered index descriptor".into(),
                ));
            }
            if instance.activation == GroupActivation::CoreRequired
                && !matches!(instance.input_domain, InputDomain::SourceLocal)
            {
                return Err(IndexError::InvalidInput(
                    "required projection adapter is not supported".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use futures::executor::block_on;

    use quanta_index_core::index::{
        AttemptId, AttemptRef, Digest, MapEdit, MapPage, MapRoot, MapWrite, ObjectRef,
        OperationKey, OwnerMutationSet, RegisteredObject, SourceCut, SourceDelta, StoredObject,
        WorkBudget, WorkLimits, WorkspaceKey,
        ports::{ImmutableObjectPort, ObjectCustodyPort, PersistentMapPort},
    };

    use super::*;

    struct NoIo;

    impl ImmutableObjectPort for NoIo {
        fn install_batch(
            &self,
            _: &[quanta_index_core::index::ObjectBatchInput],
            _: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<Vec<RegisteredObject>, IndexError> {
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
        ) -> Result<Vec<MapRoot>, IndexError> {
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
        fn lookup(
            &self,
            _: MapRoot,
            _: &[u8],
            _: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            Err(IndexError::BackendViolation("unexpected test IO".into()))
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

    struct NumberBackend;
    impl IndexBuildPort for NumberBackend {
        type Delta = u64;
        type Plan = MemberBinding;
        type Candidate = MemberBinding;
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

        fn prepare_owner<'a>(
            &'a self,
            instance: &'a InstanceSpec,
            owner: OwnerKey,
            input: OwnerInputBinding,
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

    #[test]
    fn distinct_typed_deltas_and_two_instances_of_one_kind() -> Result<(), IndexError> {
        let mut registry = AdapterRegistry::new();
        registry.register(descriptor(1), NumberBackend, |delta: &SourceDelta| {
            Ok(delta.changed.count)
        })?;
        registry.register(descriptor(2), TextBackend, |_delta: &SourceDelta| {
            Ok(String::from("text"))
        })?;
        let specs = vec![spec(1, 3), spec(1, 4), spec(2, 5)];
        registry.validate_profile(&specs)?;
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
        let mut context = AttemptContext {
            attempt,
            objects: &io,
            maps: &io,
            custody: &io,
            budget: &mut budget,
        };
        for spec in specs {
            let input = InputBinding {
                root: source.owners,
                schema: spec.schema,
                policy: spec.policy,
                context: Digest([0; 32]),
            };
            let binding = block_on(registry.build(
                &spec,
                BaseMember::Absent,
                Some(&delta),
                source,
                input,
                &mut context,
            ))?;
            assert_eq!(binding.instance, spec.instance);
            assert_eq!(binding.input, input);
            assert_eq!(
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
            );
        }
        registry.register_owner(descriptor(3), OwnerBackend)?;
        let mut async_spec = spec(3, 6);
        async_spec.activation = GroupActivation::AsyncCoverage;
        registry.validate_profile(&[async_spec.clone()])?;
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
        let owner_binding =
            block_on(registry.build_owner(&async_spec, owner, owner_input, &mut context))?;
        assert_eq!(owner_binding.instance, async_spec.instance);
        assert_eq!(owner_binding.input, owner_input);
        assert!(
            registry
                .register_owner(descriptor(1), OwnerBackend)
                .is_err()
        );
        assert!(
            registry
                .register(descriptor(3), NumberBackend, |_delta: &SourceDelta| Ok(0))
                .is_err()
        );
        assert!(
            registry
                .register(descriptor(1), NumberBackend, |_delta: &SourceDelta| Ok(0))
                .is_err()
        );
        Ok(())
    }
}
