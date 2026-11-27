//! Typed sealed History verification; file Source derivation is not involved.
use super::*;
use quanta_index_core::index::{HistoryAppendV1, HistoryGenerationAdvanceV1, HistoryTransition, ports::IndexHistoryBuildPort};

pub(super) trait ErasedHistoryAdapter: Send + Sync {
    fn descriptor(&self) -> AdapterDescriptor;
    fn resource_demand(&self) -> ResourceNeed;
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError>;
    fn verify(
        &self,
        instance: &InstanceSpec,
        prior: Option<&MemberBinding>,
        transition: &HistoryTransition,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError>;
    fn append(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        append: &HistoryAppendV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError>;
    fn advance_generation(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        advance: &HistoryGenerationAdvanceV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError>;
}

struct HistoryAdapter<A> {
    descriptor: AdapterDescriptor,
    demand: ResourceNeed,
    backend: A,
}

impl<A: IndexHistoryBuildPort> ErasedHistoryAdapter for HistoryAdapter<A> {
    fn descriptor(&self) -> AdapterDescriptor {
        self.descriptor
    }
    fn resource_demand(&self) -> ResourceNeed {
        self.demand
    }
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        if instance.activation != GroupActivation::CoreRequired
            || instance.input_domain != InputDomain::History
        {
            return Err(IndexError::InvalidInput(
                "History requires its own required input domain".into(),
            ));
        }
        self.backend.validate_instance(instance)
    }
    fn verify(
        &self,
        instance: &InstanceSpec,
        prior: Option<&MemberBinding>,
        transition: &HistoryTransition,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        context.budget.checkpoint()?;
        self.validate_instance(instance)?;
        if self.backend.resource_demand() != self.demand {
            return Err(IndexError::BackendViolation(
                "History demand changed after registration".into(),
            ));
        }
        let binding = self
            .backend
            .verify_history(instance, prior, transition, next_view_sequence, context)?;
        context.budget.checkpoint()?;
        if self.backend.resource_demand() != self.demand {
            return Err(IndexError::BackendViolation(
                "History demand changed during verification".into(),
            ));
        }
        if transition.instance != instance.instance
            || binding.instance != instance.instance
            || binding.format != instance.format
            || binding.input.schema != instance.schema
            || binding.input.policy != instance.policy
            || binding.input.context
                != transition
                    .next
                    .input_context_v1()
                    .map_err(|error| IndexError::InvalidInput(error.into()))?
            || binding.logical_commitment != transition.next.snapshot_digest
        {
            return Err(IndexError::BackendViolation(
                "History verifier returned mismatched input/member".into(),
            ));
        }
        Ok(binding)
    }
    fn append(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        append: &HistoryAppendV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        context.budget.checkpoint()?;
        self.validate_instance(instance)?;
        if self.backend.resource_demand() != self.demand {
            return Err(IndexError::BackendViolation("History demand changed after registration".into()));
        }
        let binding = self.backend.append_history(
            instance, prior, append, next_view_sequence, context,
        )?;
        context.budget.checkpoint()?;
        if self.backend.resource_demand() != self.demand {
            return Err(IndexError::BackendViolation("History demand changed during append".into()));
        }
        if append.instance != instance.instance
            || binding.instance != instance.instance
            || binding.format != instance.format
            || binding.input.schema != instance.schema
            || binding.input.policy != instance.policy
            || binding.input.context == prior.input.context
            || binding.logical_commitment == prior.logical_commitment
        {
            return Err(IndexError::BackendViolation("History append returned unchanged or mismatched member".into()));
        }
        Ok(binding)
    }

    fn advance_generation(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        advance: &HistoryGenerationAdvanceV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        context.budget.checkpoint()?;
        self.validate_instance(instance)?;
        if self.backend.resource_demand() != self.demand {
            return Err(IndexError::BackendViolation("History demand changed after registration".into()));
        }
        let binding = self.backend.advance_history_generation(instance, prior, advance, next_view_sequence, context)?;
        context.budget.checkpoint()?;
        if self.backend.resource_demand() != self.demand
            || binding.instance != advance.instance
            || binding.instance != instance.instance
            || binding.format != instance.format
            || binding.input.schema != instance.schema
            || binding.input.policy != instance.policy
            || binding.input.context == prior.input.context
            || binding.logical_commitment == prior.logical_commitment
        {
            return Err(IndexError::BackendViolation("History generation returned unchanged or mismatched member".into()));
        }
        Ok(binding)
    }
}

impl AdapterRegistry {
    pub fn register_history<A: IndexHistoryBuildPort + 'static>(
        &mut self,
        descriptor: AdapterDescriptor,
        backend: A,
    ) -> Result<(), IndexError> {
        if self.core.contains_key(&descriptor.kind)
            || self.projection.contains_key(&descriptor.kind)
            || self.owners.contains_key(&descriptor.kind)
            || self.history.contains_key(&descriptor.kind)
        {
            return Err(IndexError::InvalidInput("duplicate adapter kind".into()));
        }
        let demand = backend.resource_demand();
        if demand.provider_units != 0 {
            return Err(IndexError::InvalidInput(
                "History verification has no external provider".into(),
            ));
        }
        let old = self.history.insert(
            descriptor.kind,
            Arc::new(HistoryAdapter {
                descriptor,
                demand,
                backend,
            }),
        );
        debug_assert!(old.is_none());
        Ok(())
    }

    pub(crate) fn verify_history(
        &self,
        instance: &InstanceSpec,
        prior: Option<&MemberBinding>,
        transition: &HistoryTransition,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        let adapter = self
            .history
            .get(&instance.kind)
            .filter(|_| self.supports(instance))
            .ok_or_else(|| IndexError::InvalidInput("unregistered History adapter".into()))?;
        adapter.verify(instance, prior, transition, next_view_sequence, context)
    }

    pub(crate) fn append_history(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        append: &HistoryAppendV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        let adapter = self.history.get(&instance.kind)
            .filter(|_| self.supports(instance))
            .ok_or_else(|| IndexError::InvalidInput("unregistered History adapter".into()))?;
        adapter.append(instance, prior, append, next_view_sequence, context)
    }

    pub(crate) fn advance_history_generation(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        advance: &HistoryGenerationAdvanceV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError> {
        let adapter = self.history.get(&instance.kind)
            .filter(|_| self.supports(instance))
            .ok_or_else(|| IndexError::InvalidInput("unregistered History adapter".into()))?;
        adapter.advance_generation(instance, prior, advance, next_view_sequence, context)
    }
}
