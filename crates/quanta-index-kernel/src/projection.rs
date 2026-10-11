use quanta_index_core::index::{
    AttemptContext, Digest, IndexError, MapEdit, MapRoot, ObjectRef, OperationKey, OrdinalMapEdit,
    OwnerKey, OwnerMutationCursor, OwnerMutationSet, ProducerCompleteness, ProjectionBinding,
    ProjectionCertificate, ProjectionId, ProjectionOwnerContribution, ProjectionTransition,
    RegisteredObject, SourceCut, owner_map_key, ports::MapEditCursor,
};

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
    let prior = context.maps.lookup(delta.prior, &key, context.budget)?;
    let next = context
        .maps
        .lookup(delta.next.owners, &key, context.budget)?;
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
        Ok(self
            .mutations
            .next_mutation(budget)?
            .map(|mutation| OrdinalMapEdit {
                root: 0,
                edit: MapEdit {
                    key: owner_map_key(mutation.owner),
                    expected: mutation.expected,
                    next: mutation.next,
                },
            }))
    }
}

pub(crate) fn projection_key(projection: ProjectionId) -> Vec<u8> {
    let mut key = b"projection\0".to_vec();
    key.extend_from_slice(&projection.0);
    key
}

fn decode<T: serde::de::DeserializeOwned>(
    payload: &[u8],
    name: &'static str,
) -> Result<T, IndexError> {
    serde_json::from_slice(payload)
        .map_err(|error| IndexError::CorruptObject(format!("{name}: {error}")))
}

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    serde_json::to_vec(value)
        .map_err(|error| IndexError::InvalidInput(format!("projection encoding: {error}")))
}

pub(crate) fn contribution_children(contribution: ProjectionOwnerContribution) -> Vec<ObjectRef> {
    contribution.object_children()
}

pub(crate) fn certificate_children(certificate: &ProjectionCertificate) -> Vec<ObjectRef> {
    certificate.object_children()
}

pub(crate) fn binding_children(binding: &ProjectionBinding) -> Vec<ObjectRef> {
    binding.object_children()
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
    let contribution: ProjectionOwnerContribution =
        decode(&stored.payload, "projection contribution")?;
    if stored.registration.children != contribution_children(contribution)
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
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "projection binding reference mismatch".into(),
        ));
    }
    let binding: ProjectionBinding = decode(&stored.payload, "projection binding")?;
    if binding.projection != projection
        || stored.registration.children != binding_children(&binding)
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
        .lookup(root, &projection_key(projection), context.budget)?
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
    Ok(stored.registration)
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
    let actual_ref = context.maps.lookup(base_root, &key, context.budget)?;
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
    let attestation: ProjectionCertificate = decode(&stored.payload, "projection certificate")?;
    if registration.children != certificate_children(&attestation)
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
            .lookup(prior_owners, &owner_key, context.budget)?;
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
            let actual = context.maps.lookup(
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
        let [root]: [MapRoot; 1] = roots.try_into().map_err(|_roots| {
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
    let reference = context.put(&encode(&next)?, &binding_children(&next))?;
    let root = context.apply(
        base_root,
        &[MapEdit {
            key,
            expected: actual_ref,
            next: Some(reference),
        }],
    )?;
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
