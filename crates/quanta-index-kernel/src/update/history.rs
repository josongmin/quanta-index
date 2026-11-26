//! History profile and prior-member rules under the original Kernel attempt.
//! The registered History port owns sealed rows, replay and lineage validation.

use quanta_index_core::index::{
    AttemptContext, GroupActivation, HistoryAppendV1, HistoryGenerationAdvanceV1,
    HistorySourceCutV1, HistoryTransition, IndexError, InputDomain, InstanceSpec, MemberBinding,
    ObjectRef, WorkBudget,
};

use crate::{member_staging::ProfileIndex, registry::AdapterRegistry};

pub(crate) fn validate_target_profile(
    prior: &ProfileIndex<'_>,
    target: &ProfileIndex<'_>,
    transition: &HistoryTransition,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let spec = target.get(&transition.instance).copied().ok_or_else(|| {
        IndexError::InvalidInput("History transition instance is absent from target profile".into())
    })?;
    require_history_spec(spec)?;
    match (
        transition.expected.as_ref(),
        prior.get(&transition.instance),
    ) {
        (Some(_), Some(previous)) if *previous == spec => {}
        (None, None) => {}
        _ => {
            return Err(IndexError::InvalidInput(
                "History FULL requires a new instance; DELTA preserves its complete prior spec"
                    .into(),
            ));
        }
    }
    for instance in prior.union(target) {
        budget.checkpoint()?;
        if instance != transition.instance && prior.get(&instance) != target.get(&instance) {
            return Err(IndexError::InvalidInput(
                "History transition cannot change another profile instance".into(),
            ));
        }
    }
    budget.checkpoint()
}

pub(crate) fn validate_append_profile(
    prior: &ProfileIndex<'_>,
    target: &ProfileIndex<'_>,
    append: &HistoryAppendV1,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let spec = prior.get(&append.instance).copied().ok_or_else(|| {
        IndexError::InvalidInput("History append requires a prior instance".into())
    })?;
    require_history_spec(spec)?;
    if target.get(&append.instance) != Some(&spec) {
        return Err(IndexError::InvalidInput(
            "History append changed its instance spec".into(),
        ));
    }
    for instance in prior.union(target) {
        budget.checkpoint()?;
        if prior.get(&instance) != target.get(&instance) {
            return Err(IndexError::InvalidInput(
                "History append changed the selected profile".into(),
            ));
        }
    }
    budget.checkpoint()
}

pub(super) fn validate_generation_profile(
    prior: &ProfileIndex<'_>,
    target: &ProfileIndex<'_>,
    advance: &HistoryGenerationAdvanceV1,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    let anchor = prior.get(&advance.prior_instance).copied().ok_or_else(|| {
        IndexError::InvalidInput("History generation requires a prior instance".into())
    })?;
    require_history_spec(anchor)?;
    let successor = target.get(&advance.instance).copied().ok_or_else(|| {
        IndexError::InvalidInput("History generation requires a target instance".into())
    })?;
    if *successor
        != (InstanceSpec {
            instance: advance.instance,
            ..anchor.clone()
        })
        || (advance.instance != advance.prior_instance && prior.get(&advance.instance).is_some())
    {
        return Err(IndexError::InvalidInput(
            "History generation changed the prior instance recipe".into(),
        ));
    }
    for instance in prior.union(target) {
        budget.checkpoint()?;
        if instance != advance.instance && prior.get(&instance) != target.get(&instance) {
            return Err(IndexError::InvalidInput(
                "History generation changed an unrelated profile instance".into(),
            ));
        }
    }
    budget.checkpoint()
}

pub(super) fn prepare_generation_member(
    registry: &AdapterRegistry,
    spec: &InstanceSpec,
    prior_spec: &InstanceSpec,
    prior: &MemberBinding,
    advance: &HistoryGenerationAdvanceV1,
    next_view_sequence: u64,
    context: &mut AttemptContext<'_>,
) -> Result<ObjectRef, IndexError> {
    validate_prior_member(Some(prior_spec), prior_spec, Some(prior), None)?;
    validate_append_prior_cut(prior, &advance.expected)?;
    let binding =
        registry.advance_history_generation(spec, prior, advance, next_view_sequence, context)?;
    super::stage_required_member(&binding, context)
}

pub(super) fn prepare_required_member(
    registry: &AdapterRegistry,
    prior_spec: Option<&InstanceSpec>,
    spec: &InstanceSpec,
    prior: Option<(ObjectRef, MemberBinding)>,
    transition: Option<&HistoryTransition>,
    append: Option<&HistoryAppendV1>,
    next_view_sequence: u64,
    context: &mut AttemptContext<'_>,
) -> Result<ObjectRef, IndexError> {
    context.budget.checkpoint()?;
    if let Some(append) = append {
        if transition.is_some() || append.instance != spec.instance || prior_spec != Some(spec) {
            return Err(IndexError::InvalidInput(
                "History append requires an unchanged prior instance".into(),
            ));
        }
        require_history_spec(spec)?;
        let (_, prior_binding) = prior.ok_or_else(|| {
            IndexError::CorruptObject("History append prior member is absent".into())
        })?;
        validate_prior_member(prior_spec, spec, Some(&prior_binding), None)?;
        validate_append_prior_cut(&prior_binding, &append.expected)?;
        let binding =
            registry.append_history(spec, &prior_binding, append, next_view_sequence, context)?;
        return super::stage_required_member(&binding, context);
    }
    validate_prior_member(
        prior_spec,
        spec,
        prior.as_ref().map(|(_, binding)| binding),
        transition,
    )?;
    let Some(transition) = transition else {
        // The parent loaded and authenticated this exact member object. An
        // unrelated update must neither restage it nor manufacture an empty one.
        let (reference, _) = prior.ok_or_else(|| {
            IndexError::CorruptObject("unchanged required History member is absent".into())
        })?;
        context.budget.checkpoint()?;
        return Ok(reference);
    };
    let binding = registry.verify_history(
        spec,
        prior.as_ref().map(|(_, binding)| binding),
        transition,
        context,
    )?;
    // The sealed input is a carrier; its reference need not equal the
    // verified member artifact derived by the canonical History port.
    super::stage_required_member(&binding, context)
}

fn validate_append_prior_cut(
    prior: &MemberBinding,
    expected: &HistorySourceCutV1,
) -> Result<(), IndexError> {
    let expected_context = expected
        .input_context_v1()
        .map_err(|error| IndexError::InvalidInput(error.into()))?;
    if prior.input.context != expected_context
        || prior.logical_commitment != expected.snapshot_digest
    {
        return Err(IndexError::Conflict);
    }
    Ok(())
}

fn require_history_spec(spec: &InstanceSpec) -> Result<(), IndexError> {
    if spec.activation != GroupActivation::CoreRequired || spec.input_domain != InputDomain::History
    {
        return Err(IndexError::InvalidInput(
            "History member requires its own CoreRequired input domain".into(),
        ));
    }
    Ok(())
}

/// Pure preconditions, before invoking the History port or staging any object.
fn validate_prior_member(
    prior_spec: Option<&InstanceSpec>,
    spec: &InstanceSpec,
    prior: Option<&MemberBinding>,
    transition: Option<&HistoryTransition>,
) -> Result<(), IndexError> {
    require_history_spec(spec)?;
    if let Some(binding) = prior {
        let previous = prior_spec.ok_or_else(|| {
            IndexError::CorruptObject("History member exists outside the published profile".into())
        })?;
        if binding.instance != previous.instance
            || binding.format != previous.format
            || binding.input.schema != previous.schema
            || binding.input.policy != previous.policy
        {
            return Err(IndexError::CorruptObject(
                "published History member differs from its profile".into(),
            ));
        }
    }
    let Some(transition) = transition else {
        if prior_spec != Some(spec) {
            return Err(IndexError::InvalidInput(
                "new or changed History instance requires a History transition".into(),
            ));
        }
        return prior.map(|_| ()).ok_or_else(|| {
            IndexError::CorruptObject("unchanged required History member is absent".into())
        });
    };
    if transition.instance != spec.instance {
        return Err(IndexError::InvalidInput(
            "History transition instance differs from selected profile".into(),
        ));
    }
    match transition.expected.as_ref() {
        None => {
            if prior_spec.is_some() || prior.is_some() {
                return Err(IndexError::Conflict);
            }
        }
        Some(expected) => {
            if prior_spec != Some(spec) {
                return Err(IndexError::InvalidInput(
                    "History DELTA must preserve the complete prior instance spec".into(),
                ));
            }
            let binding = prior.ok_or_else(|| {
                IndexError::CorruptObject("History DELTA required prior member is absent".into())
            })?;
            let expected_context = expected
                .input_context_v1()
                .map_err(|error| IndexError::InvalidInput(error.into()))?;
            if binding.input.context != expected_context
                || binding.logical_commitment != expected.snapshot_digest
            {
                return Err(IndexError::Conflict);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        validate_append_prior_cut, validate_append_profile, validate_prior_member,
        validate_target_profile,
    };
    use crate::{member_staging::ProfileIndex, staging_payload::tests::scoped_budget};
    use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::index::{
        AuxiliaryGenerationKeyV1, Digest, GroupActivation, HistoryAppendV1, HistorySourceCutV1,
        HistoryTransition, IndexError, IndexInstanceId, IndexKind, InputBinding, InputDomain,
        InstanceSpec, MapRoot, MemberBinding, ObjectRef, PublicationGroupId, SchemaId,
        StorageFormatId,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn spec(id: u8) -> InstanceSpec {
        InstanceSpec {
            instance: IndexInstanceId([id; 16]),
            kind: IndexKind([2; 16]),
            group: PublicationGroupId([3; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::History,
            schema: SchemaId([4; 16]),
            policy: Digest([5; 32]),
            format: StorageFormatId([6; 16]),
        }
    }

    fn transition() -> Result<HistoryTransition, Box<dyn std::error::Error>> {
        Ok(HistoryTransition {
            instance: spec(7).instance,
            expected: None,
            next: HistorySourceCutV1 {
                generation: AuxiliaryGenerationKeyV1 {
                    repo_id: RepoId::new("history-repo")?,
                    revision_id: RevisionId::new("history-revision")?,
                    generation: ManifestGeneration::new(9),
                },
                durable_epoch: AuxEpochV1::new(3),
                snapshot_digest: Digest([10; 32]),
                replay_start: Digest([11; 32]),
            },
            input: ObjectRef(Digest([12; 32])),
            profile: ObjectRef(Digest([13; 32])),
        })
    }

    fn prior_binding(tx: &HistoryTransition) -> Result<MemberBinding, Box<dyn std::error::Error>> {
        let spec = spec(7);
        Ok(MemberBinding {
            instance: spec.instance,
            format: spec.format,
            artifact: ObjectRef(Digest([14; 32])),
            input: InputBinding {
                root: MapRoot(Some(ObjectRef(Digest([15; 32])))),
                schema: spec.schema,
                policy: spec.policy,
                context: tx.next.input_context_v1()?,
            },
            logical_commitment: tx.next.snapshot_digest,
        })
    }

    #[test]
    fn history_full_profile_adds_only_its_instance_without_extra_index_backing() -> TestResult {
        let (budget, ledger) = scoped_budget(u64::MAX)?;
        let mut sibling = spec(2);
        sibling.input_domain = InputDomain::SourceLocal;
        let before = [sibling.clone()];
        let after = [spec(7), sibling];
        let prior = ProfileIndex::new(&before, &budget)?;
        let target = ProfileIndex::new(&after, &budget)?;
        let funded = ledger.live();
        validate_target_profile(&prior, &target, &transition()?, &budget)?;
        assert_eq!(ledger.live(), funded);
        Ok(())
    }

    #[test]
    fn history_profile_refuses_unrelated_change_addition_or_removal() -> TestResult {
        let (budget, _) = scoped_budget(u64::MAX)?;
        let before = [spec(2)];
        let tx = transition()?;
        let prior = ProfileIndex::new(&before, &budget)?;
        let mut changed = spec(2);
        changed.policy = Digest([99; 32]);
        for after in [
            vec![spec(7), changed],
            vec![spec(7), spec(2), spec(8)],
            vec![spec(7)],
        ] {
            let target = ProfileIndex::new(&after, &budget)?;
            assert!(matches!(
                validate_target_profile(&prior, &target, &tx, &budget),
                Err(IndexError::InvalidInput(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn history_profile_requires_history_domain_and_full_delta_identity() -> TestResult {
        let (budget, _) = scoped_budget(u64::MAX)?;
        let empty = ProfileIndex::new(&[], &budget)?;
        let old = [spec(7)];
        let prior = ProfileIndex::new(&old, &budget)?;
        let target = ProfileIndex::new(&old, &budget)?;
        let mut tx = transition()?;
        assert!(validate_target_profile(&prior, &target, &tx, &budget).is_err());
        tx.expected = Some(tx.next.clone());
        validate_target_profile(&prior, &target, &tx, &budget)?;
        assert!(validate_target_profile(&empty, &target, &tx, &budget).is_err());
        for axis in 0..5 {
            let mut bad = spec(7);
            match axis {
                0 => bad.kind = IndexKind([99; 16]),
                1 => bad.group = PublicationGroupId([99; 16]),
                2 => bad.schema = SchemaId([99; 16]),
                3 => bad.policy = Digest([99; 32]),
                _ => bad.format = StorageFormatId([99; 16]),
            }
            let rows = [bad];
            let changed = ProfileIndex::new(&rows, &budget)?;
            assert!(validate_target_profile(&prior, &changed, &tx, &budget).is_err());
        }
        tx.expected = None;
        assert!(validate_target_profile(&empty, &empty, &tx, &budget).is_err());
        let mut wrong_domain = spec(7);
        wrong_domain.input_domain = InputDomain::SourceLocal;
        let mut wrong_activation = spec(7);
        wrong_activation.activation = GroupActivation::AsyncCoverage;
        for bad in [wrong_domain, wrong_activation] {
            let rows = [bad];
            let target = ProfileIndex::new(&rows, &budget)?;
            assert!(validate_target_profile(&empty, &target, &tx, &budget).is_err());
        }
        Ok(())
    }

    #[test]
    fn history_delta_expected_cut_binds_actual_context_and_snapshot() -> TestResult {
        let spec = spec(7);
        let mut tx = transition()?;
        let binding = prior_binding(&tx)?;
        tx.expected = Some(tx.next.clone());
        tx.next.durable_epoch = AuxEpochV1::new(4);
        tx.next.snapshot_digest = Digest([16; 32]);
        tx.next.replay_start = Digest([17; 32]);
        validate_prior_member(Some(&spec), &spec, Some(&binding), Some(&tx))?;
        for axis in 0..4 {
            let mut wrong = tx.clone();
            let expected = wrong.expected.as_mut().ok_or("expected cut absent")?;
            match axis {
                0 => expected.replay_start = Digest([90; 32]),
                1 => expected.durable_epoch = AuxEpochV1::new(4),
                2 => expected.generation.generation = ManifestGeneration::new(10),
                _ => expected.snapshot_digest = Digest([91; 32]),
            }
            assert!(matches!(
                validate_prior_member(Some(&spec), &spec, Some(&binding), Some(&wrong)),
                Err(IndexError::Conflict)
            ));
        }
        let mut forged = binding;
        forged.logical_commitment = Digest([92; 32]);
        assert!(matches!(
            validate_prior_member(Some(&spec), &spec, Some(&forged), Some(&tx)),
            Err(IndexError::Conflict)
        ));
        Ok(())
    }

    #[test]
    fn history_append_preserves_profile_and_binds_prior_cut() -> TestResult {
        let (budget, _) = scoped_budget(u64::MAX)?;
        let tx = transition()?;
        let append = HistoryAppendV1 {
            instance: tx.instance,
            expected: tx.next.clone(),
            input: tx.input,
            profile: tx.profile,
        };
        let rows = [spec(7), spec(2)];
        let prior = ProfileIndex::new(&rows, &budget)?;
        let target = ProfileIndex::new(&rows, &budget)?;
        validate_append_profile(&prior, &target, &append, &budget)?;
        let binding = prior_binding(&tx)?;
        validate_append_prior_cut(&binding, &append.expected)?;

        let missing_rows = [spec(2)];
        let missing = ProfileIndex::new(&missing_rows, &budget)?;
        assert!(matches!(
            validate_append_profile(&missing, &target, &append, &budget),
            Err(IndexError::InvalidInput(_))
        ));
        let changed_rows = [spec(7)];
        let changed = ProfileIndex::new(&changed_rows, &budget)?;
        assert!(matches!(
            validate_append_profile(&prior, &changed, &append, &budget),
            Err(IndexError::InvalidInput(_))
        ));
        let mut changed_spec_rows = rows.clone();
        changed_spec_rows[0].policy = Digest([93; 32]);
        let changed_spec = ProfileIndex::new(&changed_spec_rows, &budget)?;
        assert!(matches!(
            validate_append_profile(&prior, &changed_spec, &append, &budget),
            Err(IndexError::InvalidInput(_))
        ));
        for axis in 0..4 {
            let mut wrong = append.expected.clone();
            match axis {
                0 => wrong.replay_start = Digest([90; 32]),
                1 => wrong.durable_epoch = AuxEpochV1::new(4),
                2 => wrong.generation.generation = ManifestGeneration::new(10),
                _ => wrong.snapshot_digest = Digest([91; 32]),
            }
            assert!(matches!(
                validate_append_prior_cut(&binding, &wrong),
                Err(IndexError::Conflict)
            ));
        }
        Ok(())
    }

    #[test]
    fn history_member_refuses_orphans_shape_drift_and_empty_reconfigure() -> TestResult {
        let spec = spec(7);
        let tx = transition()?;
        let binding = prior_binding(&tx)?;
        validate_prior_member(None, &spec, None, Some(&tx))?;
        validate_prior_member(Some(&spec), &spec, Some(&binding), None)?;
        assert!(validate_prior_member(None, &spec, None, None).is_err());
        assert!(validate_prior_member(Some(&spec), &spec, None, None).is_err());
        assert!(validate_prior_member(None, &spec, Some(&binding), Some(&tx)).is_err());
        assert!(validate_prior_member(Some(&spec), &spec, Some(&binding), Some(&tx)).is_err());
        let mut changed = spec.clone();
        changed.group = PublicationGroupId([99; 16]);
        assert!(validate_prior_member(Some(&spec), &changed, Some(&binding), None).is_err());
        for axis in 0..4 {
            let mut bad = binding.clone();
            match axis {
                0 => bad.instance = IndexInstanceId([90; 16]),
                1 => bad.format = StorageFormatId([90; 16]),
                2 => bad.input.schema = SchemaId([90; 16]),
                _ => bad.input.policy = Digest([90; 32]),
            }
            assert!(matches!(
                validate_prior_member(Some(&spec), &spec, Some(&bad), None),
                Err(IndexError::CorruptObject(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn history_profile_observes_original_cancellation_before_validation() -> TestResult {
        let (budget, _) = scoped_budget(u64::MAX)?;
        let empty = ProfileIndex::new(&[], &budget)?;
        let rows = [spec(7)];
        let target = ProfileIndex::new(&rows, &budget)?;
        budget.request().cancel_handle().cancel();
        assert!(matches!(
            validate_target_profile(&empty, &target, &transition()?, &budget),
            Err(IndexError::Cancelled)
        ));
        Ok(())
    }
}
