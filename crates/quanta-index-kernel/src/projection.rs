use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapRoot, ObjectChildren, ObjectRef, OperationKey,
    OrdinalMapEdit, OwnerKey, OwnerMutationCursor, OwnerMutationSet, ProducerCompleteness,
    ProjectionBinding, ProjectionCertificate, ProjectionId, ProjectionOwnerContribution,
    ProjectionTransition, RegisteredObject, SourceCut, decode_projection_binding,
    decode_projection_certificate, decode_projection_owner, owner_map_key,
    ports::{ImmutableObjectPort, MapEditCursor},
};

use crate::{binding_metadata, member_staging::PreparedEdit};

pub(crate) struct ProjectionChange {
    pub owner: OwnerKey,
    pub prior: Option<ObjectRef>,
    pub next: Option<(ObjectRef, ProjectionOwnerContribution)>,
}

pub(crate) struct ProjectionDelta {
    pub root: MapRoot,
    pub prior: MapRoot,
    pub next: ProjectionBinding,
    pub changed: OwnerMutationSet,
}

pub(crate) fn next_change(
    mutations: &mut OwnerMutationCursor<'_>,
    delta: &ProjectionDelta,
    context: &mut AttemptContext<'_>,
) -> Result<Option<ProjectionChange>, IndexError> {
    let Some(mutation) = mutations.next_mutation(context.budget)? else {
        return Ok(None);
    };
    let key = owner_map_key(mutation.owner);
    let prior = context.maps.lookup_object(delta.prior, &key, context.budget)?;
    let next = context
        .maps
        .lookup_object(delta.next.owners, &key, context.budget)?;
    if prior != mutation.expected || next != mutation.next {
        return Err(IndexError::CorruptObject(
            "projection mutation does not match projection roots".into(),
        ));
    }
    let next = next
        .map(|reference| read_contribution(reference, context).map(|value| (reference, value)))
        .transpose()?;
    Ok(Some(ProjectionChange {
        owner: mutation.owner,
        prior,
        next,
    }))
}

struct ProjectionEditCursor<'a> {
    mutations: OwnerMutationCursor<'a>,
}

impl MapEditCursor for ProjectionEditCursor<'_> {
    fn next_edit(
        &mut self,
        budget: &mut quanta_index_core::index::WorkBudget,
    ) -> Result<Option<OrdinalMapEdit>, IndexError> {
        self.mutations
            .next_mutation(budget)?
            .map(|mutation| {
                OrdinalMapEdit::copy_object_key(
                    0,
                    &owner_map_key(mutation.owner),
                    mutation.expected,
                    mutation.next,
                    budget,
                )
            })
            .transpose()
    }
}

pub(crate) fn projection_key(projection: ProjectionId) -> [u8; 27] {
    let mut key = [0; 27];
    for (target, byte) in key
        .iter_mut()
        .zip(b"projection\0".iter().chain(&projection.0))
    {
        *target = *byte;
    }
    key
}

pub(crate) fn contribution_children(contribution: ProjectionOwnerContribution) -> ObjectChildren {
    ObjectChildren::from_references([contribution.source, contribution.output, None, None])
}

pub(crate) fn certificate_children(certificate: &ProjectionCertificate) -> ObjectChildren {
    ObjectChildren::from_references([
        certificate.target_source.owners.0,
        certificate.target_source.paths.0,
        certificate.target_source.frontiers.0,
        certificate.affected.root.0,
    ])
}

pub(crate) enum ProjectionChildren {
    Small(ObjectChildren),
    Five([ObjectRef; 5]),
}

impl ProjectionChildren {
    fn from_references(mut values: [Option<ObjectRef>; 5]) -> Self {
        values.sort_unstable();
        let mut previous = None;
        let unique = values.into_iter().flatten().filter(|reference| {
            let fresh = previous != Some(*reference);
            previous = Some(*reference);
            fresh
        });
        let mut references = [ObjectRef(Digest([0; 32])); 5];
        let mut count = 0usize;
        // Five input slots bound the output; zip never truncates a unique edge.
        for (slot, reference) in references.iter_mut().zip(unique) {
            *slot = reference;
            count = count.saturating_add(1);
        }
        let [first, second, third, fourth, fifth] = references;
        match count {
            0 => Self::Small(ObjectChildren::Empty),
            1 => Self::Small(ObjectChildren::One([first])),
            2 => Self::Small(ObjectChildren::Two([first, second])),
            3 => Self::Small(ObjectChildren::Three([first, second, third])),
            4 => Self::Small(ObjectChildren::Four([first, second, third, fourth])),
            _ => Self::Five([first, second, third, fourth, fifth]),
        }
    }

    pub(crate) fn as_slice(&self) -> &[ObjectRef] {
        match self {
            Self::Small(children) => children.as_slice(),
            Self::Five(children) => children.as_slice(),
        }
    }
}

pub(crate) fn binding_children(binding: &ProjectionBinding) -> ProjectionChildren {
    ProjectionChildren::from_references([
        Some(binding.certificate),
        binding.owners.0,
        binding.source.owners.0,
        binding.source.paths.0,
        binding.source.frontiers.0,
    ])
}

pub(crate) fn read_contribution(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<ProjectionOwnerContribution, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "projection contribution reference mismatch".into(),
        ));
    }
    let contribution = decode_projection_owner(&stored.payload, context.budget)?;
    if stored.registration.children.as_slice() != contribution_children(contribution).as_slice()
        || contribution.source_owner.is_some() != contribution.source.is_some()
    {
        return Err(IndexError::CorruptObject(
            "projection contribution edges or source owner mismatch".into(),
        ));
    }
    Ok(contribution)
}

pub(crate) fn read_binding(
    reference: ObjectRef,
    projection: ProjectionId,
    context: &mut AttemptContext<'_>,
) -> Result<ProjectionBinding, IndexError> {
    read_binding_from(reference, projection, context.objects, context.budget)
}

pub(crate) fn read_binding_from(
    reference: ObjectRef,
    projection: ProjectionId,
    objects: &dyn ImmutableObjectPort,
    budget: &mut quanta_index_core::index::WorkBudget,
) -> Result<ProjectionBinding, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "projection binding reference mismatch".into(),
        ));
    }
    let binding = decode_projection_binding(&stored.payload, budget)?;
    if binding.projection != projection
        || stored.registration.children.as_slice() != binding_children(&binding).as_slice()
    {
        return Err(IndexError::CorruptObject(
            "projection binding key or edges mismatch".into(),
        ));
    }
    Ok(binding)
}

pub(crate) fn lookup_binding(
    root: MapRoot,
    projection: ProjectionId,
    context: &mut AttemptContext<'_>,
) -> Result<Option<(ObjectRef, ProjectionBinding)>, IndexError> {
    context
        .maps
        .lookup_object(root, &projection_key(projection), context.budget)?
        .map(|reference| {
            read_binding(reference, projection, context).map(|binding| (reference, binding))
        })
        .transpose()
}

pub(crate) fn staged_registration(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<RegisteredObject, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "staged projection object reference mismatch".into(),
        ));
    }
    Ok(stored.registration.clone())
}

pub(crate) fn validate_events(
    operation: OperationKey,
    transition: &ProjectionTransition,
) -> Result<(), IndexError> {
    if transition.events.is_empty() {
        return Err(IndexError::InvalidInput(
            "projection advance requires a producer event".into(),
        ));
    }
    let digest = transition.payload_digest().map_err(|error| {
        IndexError::InvalidInput(format!("projection payload encoding: {error}"))
    })?;
    let mut last_stream = None;
    let mut operation_bound = false;
    for event in &transition.events {
        if event.producer != transition.producer
            || last_stream.is_some_and(|previous| previous >= event.stream)
            || event.payload != digest
            || event.sequence
                != event
                    .predecessor
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?
        {
            return Err(IndexError::InvalidInput(
                "projection event identity or payload mismatch".into(),
            ));
        }
        last_stream = Some(event.stream);
        if event.producer == operation.producer
            && event.stream == operation.stream
            && event.stream_epoch == operation.stream_epoch
        {
            // Journal keys and producer frontiers have independent sequence
            // spaces; only the stream identity binds this event to the request.
            operation_bound = true;
        }
    }
    if transition.producer != operation.producer || !operation_bound {
        return Err(IndexError::InvalidInput(
            "operation is not bound to projection producer frontier".into(),
        ));
    }
    Ok(())
}

pub(crate) fn apply_projection(
    operation: OperationKey,
    base_root: MapRoot,
    current_source: SourceCut,
    transition: &ProjectionTransition,
    context: &mut AttemptContext<'_>,
) -> Result<ProjectionDelta, IndexError> {
    let count = transition.owners.count;
    validate_events(operation, transition)?;
    if transition.target_source != current_source {
        return Err(IndexError::Conflict);
    }
    let ProducerCompleteness::CompleteAffectedDomain { certificate } = transition.completeness
    else {
        return Err(IndexError::RebuildRequired);
    };
    let key = projection_key(transition.projection);
    let actual_ref = context.maps.lookup_object(base_root, &key, context.budget)?;
    let actual = actual_ref
        .map(|reference| read_binding(reference, transition.projection, context))
        .transpose()?;
    if actual != transition.expected {
        return Err(IndexError::Conflict);
    }
    if actual.is_some_and(|binding| binding.producer != transition.producer) {
        return Err(IndexError::InvalidInput(
            "projection producer changed without reconfiguration".into(),
        ));
    }
    if actual.is_some_and(|binding| {
        binding.schema != transition.schema || binding.producer_policy != transition.producer_policy
    }) {
        return Err(IndexError::RebuildRequired);
    }
    let registration = staged_registration(certificate, context)?;
    let stored = context.objects.get(certificate, context.budget)?;
    let attestation = decode_projection_certificate(&stored.payload, context.budget)?;
    let expected_edges = certificate_children(&attestation);
    if registration.children.as_slice() != expected_edges.as_slice()
        || attestation.projection != transition.projection
        || attestation.producer != transition.producer
        || attestation.prior != transition.expected
        || attestation.target_source != transition.target_source
        || attestation.schema != transition.schema
        || attestation.producer_policy != transition.producer_policy
        || attestation.dependency_context != transition.dependency_context
        || attestation.affected != transition.owners
    {
        return Err(IndexError::InvalidInput(
            "projection certificate does not bind complete affected transition".into(),
        ));
    }
    let prior_owners = actual.map_or(MapRoot(None), |binding| binding.owners);
    let maps = context.maps;
    let objects = context.objects;
    let mut mutations = OwnerMutationCursor::new(transition.owners, maps, objects);
    while let Some(mutation) = mutations.next_mutation(context.budget)? {
        let owner_key = owner_map_key(mutation.owner);
        let old = context
            .maps
            .lookup_object(prior_owners, &owner_key, context.budget)?;
        if old != mutation.expected || (old.is_none() && mutation.next.is_none()) {
            return Err(IndexError::Conflict);
        }
        let next = mutation
            .next
            .map(|reference| {
                read_contribution(reference, context).and_then(|contribution| {
                    if contribution.owner != mutation.owner
                        || contribution.projection != transition.projection
                        || contribution.producer != transition.producer
                        || contribution.source_owner.is_some() != contribution.source.is_some()
                    {
                        return Err(IndexError::InvalidInput(
                            "projection contribution owner/producer mismatch".into(),
                        ));
                    }
                    Ok((reference, contribution))
                })
            })
            .transpose()?;
        if let Some((_, contribution)) = next
            && let (Some(source_owner), Some(source_reference)) =
                (contribution.source_owner, contribution.source)
        {
            let actual = context.maps.lookup_object(
                current_source.owners,
                &owner_map_key(source_owner),
                context.budget,
            )?;
            if actual != Some(source_reference) {
                return Err(IndexError::Conflict);
            }
        }
    }
    let owners = if count == 0 {
        prior_owners
    } else {
        let mut edits = ProjectionEditCursor {
            mutations: OwnerMutationCursor::new(transition.owners, context.maps, context.objects),
        };
        let roots = context.apply_stream(&[prior_owners], &mut edits)?;
        let [root]: [MapRoot; 1] = roots.as_slice().try_into().map_err(|_roots| {
            IndexError::BackendViolation("projection map stream lost root".into())
        })?;
        root
    };
    let next = ProjectionBinding {
        projection: transition.projection,
        producer: transition.producer,
        owners,
        source: current_source,
        schema: transition.schema,
        producer_policy: transition.producer_policy,
        dependency_context: transition.dependency_context,
        certificate,
    };
    let payload = binding_metadata::encode_projection_binding(&next, context.budget)?;
    let reference = context.put(&payload.bytes, binding_children(&next).as_slice())?;
    let edit = PreparedEdit::new(&key, actual_ref, Some(reference), context.budget)?;
    let root = context.apply(base_root, edit.as_slice())?;
    Ok(ProjectionDelta {
        root,
        prior: prior_owners,
        next,
        changed: transition.owners,
    })
}

pub(crate) fn projection_context(binding: &ProjectionBinding) -> Digest {
    binding.materialization_context()
}

pub(crate) fn owner_context(contribution: ProjectionOwnerContribution) -> Digest {
    contribution.materialization_context()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::staging_payload::tests::{require, scoped_budget};
    use std::collections::BTreeSet;

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }

    #[test]
    fn inline_five_children_match_set_for_all_null_duplicate_and_zero_shapes()
    -> Result<(), IndexError> {
        let choices = [
            None,
            Some(reference(0)),
            Some(reference(1)),
            Some(reference(2)),
            Some(reference(3)),
            Some(reference(4)),
        ];
        for mut shape in 0..7776 {
            let mut values = [None; 5];
            for value in &mut values {
                *value = *choices
                    .get(shape % choices.len())
                    .ok_or_else(|| IndexError::BackendViolation("fixture choice".into()))?;
                shape /= choices.len();
            }
            let gold: BTreeSet<_> = values.into_iter().flatten().collect();
            let actual = ProjectionChildren::from_references(values);
            require(actual.as_slice().iter().copied().eq(gold.into_iter()))?;
        }
        Ok(())
    }

    #[test]
    fn projection_edges_match_old_contract_and_exclude_historical_roots() -> Result<(), IndexError>
    {
        use quanta_index_core::index::{ProducerId, SchemaId, StableOwnerId};
        let source = SourceCut {
            owners: MapRoot(Some(reference(3))),
            paths: MapRoot(Some(reference(2))),
            frontiers: MapRoot(Some(reference(1))),
        };
        let mut binding = ProjectionBinding {
            projection: ProjectionId([1; 16]),
            producer: ProducerId([2; 16]),
            owners: MapRoot(Some(reference(4))),
            source,
            schema: SchemaId([3; 16]),
            producer_policy: Digest([4; 32]),
            dependency_context: Digest([5; 32]),
            certificate: reference(5),
        };
        require(
            binding_children(&binding).as_slice()
                == [
                    reference(1),
                    reference(2),
                    reference(3),
                    reference(4),
                    reference(5),
                ],
        )?;
        require(binding_children(&binding).as_slice() == binding.object_children())?;
        binding.owners = source.owners;
        binding.certificate = reference(1);
        require(binding_children(&binding).as_slice() == binding.object_children())?;
        let mut historical = binding;
        historical.certificate = reference(99);
        let mut certificate = ProjectionCertificate {
            projection: binding.projection,
            producer: binding.producer,
            prior: Some(historical),
            target_source: source,
            schema: binding.schema,
            producer_policy: binding.producer_policy,
            dependency_context: binding.dependency_context,
            affected: OwnerMutationSet {
                root: MapRoot(Some(reference(4))),
                count: 1,
                digest: Digest([6; 32]),
            },
        };
        require(
            certificate_children(&certificate).as_slice()
                == [reference(1), reference(2), reference(3), reference(4)],
        )?;
        require(certificate_children(&certificate).as_slice() == certificate.object_children())?;
        certificate.affected.root = source.owners;
        require(certificate_children(&certificate).as_slice() == certificate.object_children())?;
        let mut contribution = ProjectionOwnerContribution {
            projection: binding.projection,
            producer: binding.producer,
            owner: OwnerKey {
                producer: binding.producer,
                stable_id: StableOwnerId([1; 16]),
            },
            source_owner: None,
            source: Some(reference(2)),
            dependency_stamp: Digest([7; 32]),
            output: Some(reference(1)),
        };
        require(contribution_children(contribution).as_slice() == [reference(1), reference(2)])?;
        for (source, output) in [
            (Some(reference(1)), Some(reference(1))),
            (None, Some(reference(0))),
            (None, None),
        ] {
            contribution.source = source;
            contribution.output = output;
            require(
                contribution_children(contribution).as_slice() == contribution.object_children(),
            )?;
        }
        Ok(())
    }

    #[test]
    fn projection_key_preserves_literal_namespace_and_only_copies_when_admitted()
    -> Result<(), IndexError> {
        let key = projection_key(ProjectionId([3; 16]));
        let gold: &[u8; 27] =
            b"projection\0\x03\x03\x03\x03\x03\x03\x03\x03\x03\x03\x03\x03\x03\x03\x03\x03";
        require(&key == gold)?;
        let (denied, denied_ledger) = scoped_budget(26)?;
        require(matches!(
            PreparedEdit::new(&key, None, None, &denied),
            Err(IndexError::ResourceLimited)
        ))?;
        require(denied_ledger.live() == 0)?;
        let (budget, ledger) = scoped_budget(27)?;
        let edit = PreparedEdit::new(&key, None, None, &budget)?;
        require(edit.as_edit().key.as_slice() == gold && ledger.live() == 27)?;
        drop(edit);
        require(ledger.live() == 0)?;
        Ok(())
    }
}
