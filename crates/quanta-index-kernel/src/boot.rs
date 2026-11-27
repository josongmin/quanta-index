//! Read-only validation of the selected operation's durable request and profile.
//! The caller authenticates their physical object bytes and selected catalog row.

use quanta_index_core::index::{
    CatalogView, Digest, HistoryTransition, ImmutableOperation, IndexError, IndexInstanceId,
    IndexProfile, InputDomain, InstanceSpec, MapRoot, MemoryLifetime, ObjectRef, RegisteredObject,
    StoredObject, UpdateKind, ViewRef, WorkBudget,
};

use crate::{AdapterDescriptor, AdapterRegistry, profile, request_decode, request_preparation};

/// One previously admitted Text identity accepted only while authenticating a
/// held predecessor as a Source witness. Candidate and listener stay strict.
#[derive(Clone, Copy)]
pub struct RetainedSourceTextPolicyV20 {
    pub descriptor: AdapterDescriptor,
    pub previous: Digest,
    pub current: Digest,
}

/// Borrow the sealed History FULL after the separately authenticated initial
/// publication. This only validates its exact predecessor and profile change.
pub fn with_native_boot_history_full_v1<T>(
    prior: &CatalogView,
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    prior_profile: &StoredObject,
    target_profile: &StoredObject,
    budget: &mut WorkBudget,
    verify_source: impl FnOnce(&HistoryTransition, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    verify_immutable_publication(operation, publication, request)?;
    let decoded = request_decode::decode(&request.payload, budget)?;
    let UpdateKind::HistoryAdvance(transition) = &decoded.kind else {
        return Err(IndexError::SnapshotUnavailable);
    };
    verify_immutable_request(operation, publication, request, &decoded)?;
    if decoded.expected_view != prior.identity
        || transition.expected.is_some()
        || transition.profile != publication.profile
        || publication.source != prior.source
        || publication.projections != prior.projections
        || prior_profile.registration.reference != prior.profile
        || !prior_profile.registration.children.is_empty()
        || target_profile.registration.reference != publication.profile
        || !target_profile.registration.children.is_empty()
    {
        return Err(IndexError::DigestConflict);
    }
    let previous = profile::decode_payload(&prior_profile.payload, budget)?;
    let target = profile::decode_payload(&target_profile.payload, budget)?;
    let prior_index = crate::member_staging::ProfileIndex::new(&previous.instances, budget)?;
    let target_index = crate::member_staging::ProfileIndex::new(&target.instances, budget)?;
    crate::update::validate_history_target_profile_v1(
        &prior_index,
        &target_index,
        transition,
        budget,
    )?;
    verify_source(transition, budget)
}

/// Borrow an authenticated first Source/History publication for independent
/// Native Original and installed-member verification by the BOOT reader.
pub fn with_native_boot_source_history_initial_v1<T>(
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    target_profile: &StoredObject,
    budget: &mut WorkBudget,
    verify_source: impl FnOnce(&HistoryTransition, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<Option<T>, IndexError> {
    verify_immutable_publication(operation, publication, request)?;
    let decoded = request_decode::decode(&request.payload, budget)?;
    verify_immutable_request(operation, publication, request, &decoded)?;
    let history = match &decoded.kind {
        UpdateKind::SourceHistoryBootstrap(transition) => {
            if transition.source.expected_source.owners != quanta_index_core::index::MapRoot(None)
                || transition.source.expected_source.paths != quanta_index_core::index::MapRoot(None)
                || transition.source.expected_source.frontiers != quanta_index_core::index::MapRoot(None)
            {
                return Err(IndexError::DigestConflict);
            }
            &transition.history
        }
        UpdateKind::HistoryAdvance(transition) if transition.expected.is_none() => transition,
        _ => return Ok(None),
    };
    if history.expected.is_some()
        || history.profile != publication.profile
        || target_profile.registration.reference != publication.profile
        || !target_profile.registration.children.is_empty()
    {
        return Err(IndexError::DigestConflict);
    }
    let profile = profile::decode_payload(&target_profile.payload, budget)?;
    let mut history_specs = profile
        .instances
        .iter()
        .filter(|spec| spec.input_domain == quanta_index_core::index::InputDomain::History);
    let first = history_specs.next().ok_or(IndexError::DigestConflict)?;
    if history_specs.next().is_some()
        || first.instance != history.instance
        || first.activation != quanta_index_core::index::GroupActivation::CoreRequired
    {
        return Err(IndexError::DigestConflict);
    }
    verify_source(history, budget).map(Some)
}

/// Validate the original request binding and current publication profile.
///
/// This proves neither historical source availability nor the physical closure
/// rooted by the publication; those are verified by the caller's object reader.
pub fn verify_native_boot_inputs(
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    profile: &StoredObject,
    registry: &AdapterRegistry,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    verify_native_boot_inputs_with_policy(
        operation,
        publication,
        request,
        profile,
        registry,
        false,
        None,
        budget,
    )
    .map(|_| ())
}

/// Return a retained Text instance and its actual signed policy only after
/// the original immutable request/profile and the current registry shape are
/// checked. The caller must also authenticate that member's physical binding.
pub fn verify_native_boot_retained_source_inputs_v20(
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    profile: &StoredObject,
    registry: &AdapterRegistry,
    text: RetainedSourceTextPolicyV20,
    budget: &mut WorkBudget,
) -> Result<Option<(IndexInstanceId, Digest)>, IndexError> {
    verify_native_boot_inputs_with_policy(
        operation,
        publication,
        request,
        profile,
        registry,
        false,
        Some(text),
        budget,
    )
}

/// Authenticate a new candidate's selected BOOT. A retained predecessor uses
/// `verify_native_boot_inputs` so its historical RepoMap profile can be read.
pub fn verify_native_boot_candidate_inputs(
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    profile: &StoredObject,
    registry: &AdapterRegistry,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    verify_native_boot_inputs_with_policy(
        operation,
        publication,
        request,
        profile,
        registry,
        true,
        None,
        budget,
    )
    .map(|_| ())
}

fn verify_native_boot_inputs_with_policy(
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    profile: &StoredObject,
    registry: &AdapterRegistry,
    candidate: bool,
    retained_text: Option<RetainedSourceTextPolicyV20>,
    budget: &mut WorkBudget,
) -> Result<Option<(IndexInstanceId, Digest)>, IndexError> {
    budget.checkpoint()?;
    verify_immutable_publication(operation, publication, request)?;
    let decoded = request_decode::decode(&request.payload, budget)?;
    verify_immutable_request(operation, publication, request, &decoded)?;
    if candidate {
        crate::update::reject_retired_repomap_request(&decoded)?;
    }
    if profile.registration.reference != publication.profile
        || !profile.registration.children.is_empty()
    {
        return Err(IndexError::CorruptObject(
            "boot profile reference or edges mismatch".into(),
        ));
    }
    let loaded = profile::decode_payload(&profile.payload, budget)?;
    if candidate {
        crate::update::reject_retired_repomap_profile(&loaded, budget)?;
    }
    let historical = if let Some(text) = retained_text {
        if candidate {
            return Err(IndexError::InvalidInput(
                "invalid retained Text policy gate".into(),
            ));
        }
        let mut found = None;
        if text.previous != text.current {
            for instance in &loaded.instances {
                if instance.kind == text.descriptor.kind && instance.policy == text.previous {
                    if instance.schema != text.descriptor.schema
                        || instance.format != text.descriptor.format
                        || instance.input_domain != InputDomain::SourceLocal
                        || found.replace(instance.instance).is_some()
                    {
                        return Err(IndexError::InvalidInput(
                            "retained Text profile identity mismatch".into(),
                        ));
                    }
                }
            }
        }
        if let Some(instance_id) = found {
            if loaded.instances.iter().any(|instance| {
                instance.kind == text.descriptor.kind && instance.instance != instance_id
            }) {
                return Err(IndexError::InvalidInput(
                    "retained Text profile has another instance".into(),
                ));
            }
            let bytes = loaded
                .instances
                .len()
                .checked_mul(std::mem::size_of::<InstanceSpec>())
                .ok_or(IndexError::ResourceLimited)?;
            let _memory = budget
                .reserve_memory(
                    u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?
                .ok_or(IndexError::ResourceLimited)?;
            let mut validation = Vec::new();
            validation
                .try_reserve_exact(loaded.instances.len())
                .map_err(|_| IndexError::ResourceLimited)?;
            validation.extend(loaded.instances.iter().cloned());
            let item = validation
                .iter_mut()
                .find(|item| item.instance == instance_id)
                .ok_or(IndexError::BackendViolation(
                    "retained Text instance disappeared".into(),
                ))?;
            item.policy = text.current;
            registry.validate_retained_text_profile_v20(&validation, instance_id, budget)?;
            Some((instance_id, text.previous))
        } else {
            let current = loaded.instances.iter().find(|instance| {
                instance.kind == text.descriptor.kind
                    && instance.policy == text.current
                    && instance.schema == text.descriptor.schema
                    && instance.format == text.descriptor.format
                    && instance.input_domain == InputDomain::SourceLocal
            });
            if let Some(current) = current.filter(|current| {
                registry.retained_text_companion_absent_v20(
                    &loaded.instances,
                    text.descriptor.kind,
                    current.instance,
                )
            }) {
                registry.validate_retained_text_profile_v20(
                    &loaded.instances,
                    current.instance,
                    budget,
                )?;
                Some((current.instance, text.current))
            } else {
                registry.validate_profile(&loaded.instances, budget)?;
                None
            }
        }
    } else {
        registry.validate_profile(&loaded.instances, budget)?;
        None
    };
    budget.checkpoint()?;
    Ok(historical)
}

fn verify_immutable_publication(
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
) -> Result<(), IndexError> {
    if publication.identity.workspace != operation.key.workspace
        || publication.identity.epoch != operation.key.epoch
        || publication.identity_digest().map_err(|error| {
            IndexError::CorruptObject(format!("boot publication identity: {error}"))
        })? != publication.identity.digest
    {
        return Err(IndexError::CorruptObject(
            "boot publication identity mismatch".into(),
        ));
    }
    if request.registration.reference != operation.request {
        return Err(IndexError::CorruptObject(
            "boot request reference mismatch".into(),
        ));
    }
    Ok(())
}

fn verify_immutable_request(
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    decoded: &quanta_index_core::index::UpdateRequest,
) -> Result<(), IndexError> {
    // Materialization rebases over intervening publications and its claimed
    // owner scope is checked below. Other update kinds require an exact base.
    let exact_base = !matches!(&decoded.kind, UpdateKind::Materialize { .. });
    if decoded.operation != operation.key
        || decoded.kind.source_mode() != operation.source_mode
        || decoded.expected_view.workspace != operation.key.workspace
        || decoded.expected_view.epoch != operation.key.epoch
        || (exact_base
            && decoded.expected_view.sequence.checked_add(1) != Some(publication.identity.sequence))
        || request.registration.children.as_slice()
            != request_preparation::children(decoded).as_slice()
    {
        return Err(IndexError::CorruptObject(
            "boot request body or edges mismatch".into(),
        ));
    }
    if let UpdateKind::Materialize { claim } = &decoded.kind
        && (claim.intent.workspace != operation.key.workspace
            || claim.intent.epoch != operation.key.epoch)
    {
        return Err(IndexError::CorruptObject(
            "boot materialization claim scope mismatch".into(),
        ));
    }
    if let UpdateKind::Reconfigure { profile: target } = &decoded.kind
        && *target != publication.profile
    {
        return Err(IndexError::CorruptObject(
            "boot reconfigure profile differs from publication".into(),
        ));
    }
    Ok(())
}

/// Join an authenticated Original Source target to Q's actual immutable
/// initial FULL request and its persisted Published profile.
///
/// Call alongside
/// `verify_native_boot_inputs` and physical closure validation under the held
/// Q root lease; this predicate issues no producer or selection authority.
pub fn verify_native_boot_source_target_v1(
    target: &quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1,
    source_request: &quanta_index_core::index::UpdateRequest,
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let decoded = request_decode::decode(&request.payload, budget)?;
    if &*decoded != source_request || request.registration.reference != operation.request {
        return Err(IndexError::DigestConflict);
    }
    verify_native_published_full_target_v1(
        target,
        source_request,
        operation,
        publication.identity,
        publication.profile,
        request.registration.reference,
        budget,
    )
}

/// Bind an independently identified Original FULL request to the actual
/// retained Published operation relation.
///
/// `published` and `operation` must
/// come from the live read-only Catalog, and `identified_request` from the
/// canonical object identity backend. This does not recreate a retired view,
/// establish its historical contents, or issue recovery authority.
pub fn verify_native_published_full_target_v1(
    target: &quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1,
    source_request: &quanta_index_core::index::UpdateRequest,
    operation: &ImmutableOperation,
    published: quanta_index_core::index::ViewRef,
    selected_profile: quanta_index_core::index::ObjectRef,
    identified_request: quanta_index_core::index::ObjectRef,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    if target.source_mode != quanta_index_core::index::SourceOperationMode::Advance
        || target.event_predecessor != 0
        || target.event_sequence != 1
    {
        return Err(IndexError::DigestConflict);
    }
    verify_native_published_source_target_v1(
        target,
        source_request,
        operation,
        published,
        selected_profile,
        identified_request,
        budget,
    )
}

/// Bind an independently identified SourceAdvance or SourceResync request to
/// its actual Published metadata. Original FULL uses the stricter predicate
/// above. This check issues no Source, installation or deployment permission.
pub fn verify_native_published_source_target_v1(
    target: &quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1,
    source_request: &quanta_index_core::index::UpdateRequest,
    operation: &ImmutableOperation,
    published: quanta_index_core::index::ViewRef,
    selected_profile: quanta_index_core::index::ObjectRef,
    identified_request: quanta_index_core::index::ObjectRef,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    if !matches!(
        &source_request.kind,
        UpdateKind::SourceAdvance(_) | UpdateKind::SourceResync { .. }
    )
        || selected_profile != target.expected_profile
    {
        return Err(IndexError::DigestConflict);
    }
    verify_native_source_event(
        target,
        source_request,
        operation,
        published,
        identified_request,
        budget,
    )
}

/// Bind a combined publication to the signed Source reservation and its exact
/// Catalog predecessor. The retained result profile is the History request's
/// profile. The signed target identifies the profile before publication.
pub fn verify_native_published_source_history_target_v1(
    target: &quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1,
    source_request: &quanta_index_core::index::UpdateRequest,
    operation: &ImmutableOperation,
    published: &CatalogView,
    prior: &CatalogView,
    identified_request: ObjectRef,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    verify_native_source_event(
        target,
        source_request,
        operation,
        published.identity,
        identified_request,
        budget,
    )?;
    let profile = match &source_request.kind {
        UpdateKind::SourceHistoryBootstrap(pair)
            if pair.history.expected.is_none()
                && target.event_predecessor == 0
                && target.event_sequence == 1
                && prior.source.owners == MapRoot(None)
                && prior.source.paths == MapRoot(None)
                && prior.source.frontiers == MapRoot(None)
                && pair.history.profile != prior.profile =>
        {
            pair.history.profile
        }
        UpdateKind::SourceHistoryAdvance(pair)
        | UpdateKind::SourceHistoryResync { transition: pair, .. } => pair.history.profile,
        _ => return Err(IndexError::DigestConflict),
    };
    if prior.identity != target.expected_view
        || prior.profile != target.expected_profile
        || prior.source != target.expected_source
        || prior.projections != published.projections
        || published.profile != profile
    {
        return Err(IndexError::DigestConflict);
    }
    budget.checkpoint()
}

/// Sequence zero has no Published operation row. Verify the exact deterministic
/// Catalog genesis created by the Kernel, including the canonical empty profile.
pub fn native_boot_source_genesis_v1(
    expected: ViewRef,
    budget: &mut WorkBudget,
    identify: impl FnOnce(&[u8], &[ObjectRef], &mut WorkBudget) -> Result<RegisteredObject, IndexError>,
) -> Result<CatalogView, IndexError> {
    if expected.sequence != 0 {
        return Err(IndexError::DigestConflict);
    }
    let empty = IndexProfile {
        instances: Vec::new(),
    };
    let payload = crate::binding_metadata::encode_profile(&empty, budget)?;
    let object = identify(&payload.bytes, &[], budget)?;
    let genesis =
        crate::identity::genesis_view(expected.workspace, expected.epoch, object.reference)?;
    if !object.children.is_empty() || genesis.identity != expected {
        return Err(IndexError::DigestConflict);
    }
    budget.checkpoint()?;
    Ok(genesis)
}

fn verify_native_source_event(
    target: &quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1,
    source_request: &quanta_index_core::index::UpdateRequest,
    operation: &ImmutableOperation,
    published: ViewRef,
    identified_request: ObjectRef,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    use quanta_index_core::index::SourceOperationMode;
    budget.checkpoint()?;
    let transition = source_request
        .kind
        .source_transition_v1()
        .ok_or(IndexError::DigestConflict)?;
    let [event] = transition.events.as_slice() else {
        return Err(IndexError::DigestConflict);
    };
    let valid_mode = match target.source_mode {
        SourceOperationMode::Advance => true,
        SourceOperationMode::Resync { prior_epoch } => {
            prior_epoch.checked_add(1) == Some(target.operation.stream_epoch)
                && target.event_predecessor == 0
                && target.event_sequence == 1
        }
        SourceOperationMode::Unchanged => false,
    };
    if target.operation != operation.key
        || source_request.operation != operation.key
        || identified_request != operation.request
        || !valid_mode
        || source_request.kind.source_mode() != target.source_mode
        || operation.source_mode != target.source_mode
        || source_request.expected_view != target.expected_view
        || transition.expected_source != target.expected_source
        || target.expected_view.workspace != target.operation.workspace
        || target.expected_view.epoch != target.operation.epoch
        || published.workspace != target.operation.workspace
        || published.epoch != target.operation.epoch
        || published.digest.0 == [0; 32]
        || target.expected_view.sequence.checked_add(1) != Some(published.sequence)
        || event.producer != target.operation.producer
        || event.stream != target.operation.stream
        || event.stream_epoch != target.operation.stream_epoch
        || event.predecessor != target.event_predecessor
        || event.sequence != target.event_sequence
    {
        return Err(IndexError::DigestConflict);
    }
    budget.checkpoint()
}

/// Match the retained selected request with its Original capture using the
/// canonical decoder.
///
/// Call with `verify_native_boot_inputs` and physical
/// closure validation; equality alone grants no recovery authority.
pub fn verify_native_boot_captured_request_v1(
    expected: &quanta_index_core::index::UpdateRequest,
    operation: &ImmutableOperation,
    request: &StoredObject,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let decoded = request_decode::decode(&request.payload, budget)?;
    if &*decoded != expected
        || decoded.operation != operation.key
        || decoded.kind.source_mode() != operation.source_mode
        || request.registration.reference != operation.request
    {
        return Err(IndexError::DigestConflict);
    }
    budget.checkpoint()
}

/// Borrow a canonical History-only request from an authenticated publication.
/// The caller must independently resolve its exact prior Catalog publication
/// and prove that Source and projection roots were preserved.
pub fn with_native_boot_history_request_v1<T>(
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    budget: &mut WorkBudget,
    visit: impl FnOnce(
        &quanta_index_core::index::UpdateRequest,
        &mut WorkBudget,
    ) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    verify_immutable_publication(operation, publication, request)?;
    let decoded = request_decode::decode(&request.payload, budget)?;
    verify_immutable_request(operation, publication, request, &decoded)?;
    let profile = match &decoded.kind {
        UpdateKind::HistoryAdvance(transition) => transition.profile,
        UpdateKind::HistoryAppend(append) => append.profile,
        _ => return Err(IndexError::SnapshotUnavailable),
    };
    if profile != publication.profile {
        return Err(IndexError::DigestConflict);
    }
    visit(&decoded, budget)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use quanta_index_core::index::{
        Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile, InputDomain, MapRoot,
        ObjectRef, PublicationGroupId, RegisteredObject, SchemaId, SourceCut, StorageFormatId,
        UpdateRequest, WorkLimits,
    };

    use super::*;
    use crate::request_decode::tests::scoped;
    use crate::request_preparation::tests::requests;

    struct CurrentTreeOnly(Digest);

    impl quanta_index_core::index::ports::IndexBuildPort for CurrentTreeOnly {
        type Delta = ();
        type Plan = ();
        type Candidate = ();

        fn resource_demand(&self) -> quanta_index_core::index::effort::ResourceNeed {
            quanta_index_core::index::effort::ResourceNeed::default()
        }

        fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
            if instance.policy != self.0 || instance.input_domain != InputDomain::SourceLocal {
                return Err(IndexError::InvalidInput(
                    "current Tree recipe required".into(),
                ));
            }
            Ok(())
        }

        fn plan(
            &self,
            _: quanta_index_core::index::BaseMember,
            _: quanta_index_core::index::DeltaInput<()>,
            _: &quanta_index_core::index::PlanContext,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::PlanDecision<()>, IndexError> {
            Err(IndexError::BackendViolation(
                "boot test must not build".into(),
            ))
        }

        fn prepare<'a>(
            &'a self,
            _: (),
            _: &'a mut quanta_index_core::index::AttemptContext<'_>,
        ) -> quanta_index_core::index::ports::IndexFuture<'a, ()> {
            Box::pin(async {
                Err(IndexError::BackendViolation(
                    "boot test must not build".into(),
                ))
            })
        }

        fn verify_and_seal<'a>(
            &'a self,
            _: (),
            _: &'a mut quanta_index_core::index::AttemptContext<'_>,
        ) -> quanta_index_core::index::ports::IndexFuture<'a, quanta_index_core::index::MemberBinding>
        {
            Box::pin(async {
                Err(IndexError::BackendViolation(
                    "boot test must not build".into(),
                ))
            })
        }
    }

    #[test]
    fn retained_v20_tree_profile_is_only_a_prior_source_witness()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = request_rows()
            .into_iter()
            .next()
            .ok_or(IndexError::Conflict)?;
        let (operation, publication, stored_request, _) = fixture(&request)?;
        let descriptor = AdapterDescriptor {
            kind: IndexKind([72; 16]),
            schema: SchemaId([74; 16]),
            format: StorageFormatId([75; 16]),
        };
        let previous = Digest([11; 32]);
        let current = Digest([12; 32]);
        let instance = InstanceSpec {
            instance: IndexInstanceId([71; 16]),
            kind: descriptor.kind,
            group: PublicationGroupId([73; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::SourceLocal,
            schema: descriptor.schema,
            policy: previous,
            format: descriptor.format,
        };
        let mut registry = AdapterRegistry::new();
        registry.register(descriptor, CurrentTreeOnly(current), |_| Ok(()))?;
        let literal = AdapterDescriptor {
            kind: IndexKind([82; 16]),
            schema: SchemaId([84; 16]),
            format: StorageFormatId([85; 16]),
        };
        registry.register(literal, CurrentTreeOnly(current), |_| Ok(()))?;
        registry.require_companion(descriptor.kind, literal)?;
        let stored_profile = object(
            publication.profile,
            serde_json::to_vec(&IndexProfile {
                instances: vec![instance.clone()],
            })?,
            Vec::new(),
        )?;
        let gate = RetainedSourceTextPolicyV20 {
            descriptor,
            previous,
            current,
        };
        require(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &stored_profile,
                &registry,
                &mut budget()?,
            )
            .is_err(),
        )?;
        require(
            verify_native_boot_candidate_inputs(
                &operation,
                &publication,
                &stored_request,
                &stored_profile,
                &registry,
                &mut budget()?,
            )
            .is_err(),
        )?;
        require(
            verify_native_boot_retained_source_inputs_v20(
                &operation,
                &publication,
                &stored_request,
                &stored_profile,
                &registry,
                gate,
                &mut budget()?,
            )? == Some((instance.instance, previous)),
        )?;
        for wrong in [Digest([13; 32]), Digest([0; 32])] {
            let invalid = object(
                publication.profile,
                serde_json::to_vec(&IndexProfile {
                    instances: vec![InstanceSpec {
                        policy: wrong,
                        ..instance.clone()
                    }],
                })?,
                Vec::new(),
            )?;
            require(
                verify_native_boot_retained_source_inputs_v20(
                    &operation,
                    &publication,
                    &stored_request,
                    &invalid,
                    &registry,
                    gate,
                    &mut budget()?,
                )
                .is_err(),
            )?;
        }
        let current_without_literal = object(
            publication.profile,
            serde_json::to_vec(&IndexProfile {
                instances: vec![InstanceSpec {
                    policy: current,
                    ..instance.clone()
                }],
            })?,
            Vec::new(),
        )?;
        require(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &current_without_literal,
                &registry,
                &mut budget()?,
            )
            .is_err(),
        )?;
        require(
            verify_native_boot_candidate_inputs(
                &operation,
                &publication,
                &stored_request,
                &current_without_literal,
                &registry,
                &mut budget()?,
            )
            .is_err(),
        )?;
        require(
            verify_native_boot_retained_source_inputs_v20(
                &operation,
                &publication,
                &stored_request,
                &current_without_literal,
                &registry,
                gate,
                &mut budget()?,
            )? == Some((instance.instance, current)),
        )?;
        let literal_instance = InstanceSpec {
            instance: IndexInstanceId([81; 16]),
            kind: literal.kind,
            group: PublicationGroupId([83; 16]),
            activation: GroupActivation::CoreRequired,
            input_domain: InputDomain::SourceLocal,
            schema: literal.schema,
            policy: current,
            format: literal.format,
        };
        let mixed_current = InstanceSpec {
            instance: IndexInstanceId([91; 16]),
            policy: current,
            ..instance.clone()
        };
        let current_tree = InstanceSpec {
            policy: current,
            ..instance.clone()
        };
        let another_current = InstanceSpec {
            instance: IndexInstanceId([91; 16]),
            group: PublicationGroupId([93; 16]),
            ..current_tree.clone()
        };
        let current_with_companion = object(
            publication.profile,
            serde_json::to_vec(&IndexProfile {
                instances: vec![
                    current_tree.clone(),
                    another_current.clone(),
                    literal_instance.clone(),
                ],
            })?,
            Vec::new(),
        )?;
        require(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &current_with_companion,
                &registry,
                &mut budget()?,
            )
            .is_ok(),
        )?;
        require(
            verify_native_boot_retained_source_inputs_v20(
                &operation,
                &publication,
                &stored_request,
                &current_with_companion,
                &registry,
                gate,
                &mut budget()?,
            )?
            .is_none(),
        )?;
        let current_twins_without_literal = object(
            publication.profile,
            serde_json::to_vec(&IndexProfile {
                instances: vec![current_tree, another_current],
            })?,
            Vec::new(),
        )?;
        require(
            verify_native_boot_retained_source_inputs_v20(
                &operation,
                &publication,
                &stored_request,
                &current_twins_without_literal,
                &registry,
                gate,
                &mut budget()?,
            )
            .is_err(),
        )?;
        for specs in [
            vec![instance.clone(), mixed_current],
            vec![
                instance.clone(),
                InstanceSpec {
                    format: StorageFormatId([86; 16]),
                    ..literal_instance.clone()
                },
            ],
            vec![
                instance.clone(),
                InstanceSpec {
                    kind: IndexKind([87; 16]),
                    ..literal_instance.clone()
                },
            ],
            vec![
                instance.clone(),
                InstanceSpec {
                    policy: Digest([88; 32]),
                    ..literal_instance
                },
            ],
        ] {
            let invalid = object(
                publication.profile,
                serde_json::to_vec(&IndexProfile { instances: specs })?,
                Vec::new(),
            )?;
            require(
                verify_native_boot_retained_source_inputs_v20(
                    &operation,
                    &publication,
                    &stored_request,
                    &invalid,
                    &registry,
                    gate,
                    &mut budget()?,
                )
                .is_err(),
            )?;
        }
        Ok(())
    }

    #[test]
    fn retained_v20_segment_profiles_are_only_previous_source_witnesses()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = request_rows()
            .into_iter()
            .next()
            .ok_or(IndexError::Conflict)?;
        let (operation, publication, stored_request, _) = fixture(&request)?;
        for format in [StorageFormatId([76; 16]), StorageFormatId([77; 16])] {
            let descriptor = AdapterDescriptor {
                kind: IndexKind([72; 16]),
                schema: SchemaId([74; 16]),
                format,
            };
            let policy = Digest([12; 32]);
            let text = InstanceSpec {
                instance: IndexInstanceId([71; 16]),
                kind: descriptor.kind,
                group: PublicationGroupId([73; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: descriptor.schema,
                policy,
                format,
            };
            let literal = AdapterDescriptor {
                kind: IndexKind([82; 16]),
                schema: SchemaId([84; 16]),
                format: StorageFormatId([85; 16]),
            };
            let mut registry = AdapterRegistry::new();
            registry.register(descriptor, CurrentTreeOnly(policy), |_| Ok(()))?;
            registry.register(literal, CurrentTreeOnly(policy), |_| Ok(()))?;
            registry.require_companion(descriptor.kind, literal)?;
            let gate = RetainedSourceTextPolicyV20 {
                descriptor,
                previous: policy,
                current: policy,
            };
            let profile =
                |instances: Vec<InstanceSpec>| -> Result<StoredObject, Box<dyn std::error::Error>> {
                    Ok(object(
                        publication.profile,
                        serde_json::to_vec(&IndexProfile { instances })?,
                        Vec::new(),
                    )?)
                };
            let lone = profile(vec![text.clone()])?;
            require(
                verify_native_boot_inputs(
                    &operation,
                    &publication,
                    &stored_request,
                    &lone,
                    &registry,
                    &mut budget()?,
                )
                .is_err(),
            )?;
            require(
                verify_native_boot_candidate_inputs(
                    &operation,
                    &publication,
                    &stored_request,
                    &lone,
                    &registry,
                    &mut budget()?,
                )
                .is_err(),
            )?;
            require(
                verify_native_boot_retained_source_inputs_v20(
                    &operation,
                    &publication,
                    &stored_request,
                    &lone,
                    &registry,
                    gate,
                    &mut budget()?,
                )? == Some((text.instance, policy)),
            )?;
            for malformed in [
                InstanceSpec {
                    policy: Digest([13; 32]),
                    ..text.clone()
                },
                InstanceSpec {
                    format: StorageFormatId([75; 16]),
                    ..text.clone()
                },
            ] {
                require(
                    verify_native_boot_retained_source_inputs_v20(
                        &operation,
                        &publication,
                        &stored_request,
                        &profile(vec![malformed])?,
                        &registry,
                        gate,
                        &mut budget()?,
                    )
                    .is_err(),
                )?;
            }
            let sibling = InstanceSpec {
                instance: IndexInstanceId([81; 16]),
                kind: literal.kind,
                group: PublicationGroupId([83; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: literal.schema,
                policy,
                format: literal.format,
            };
            let paired = profile(vec![text.clone(), sibling.clone()])?;
            require(
                verify_native_boot_inputs(
                    &operation,
                    &publication,
                    &stored_request,
                    &paired,
                    &registry,
                    &mut budget()?,
                )
                .is_ok(),
            )?;
            require(
                verify_native_boot_retained_source_inputs_v20(
                    &operation,
                    &publication,
                    &stored_request,
                    &paired,
                    &registry,
                    gate,
                    &mut budget()?,
                )?
                .is_none(),
            )?;
            require(
                verify_native_boot_retained_source_inputs_v20(
                    &operation,
                    &publication,
                    &stored_request,
                    &profile(vec![
                        text.clone(),
                        InstanceSpec {
                            format: StorageFormatId([86; 16]),
                            ..sibling
                        },
                    ])?,
                    &registry,
                    gate,
                    &mut budget()?,
                )
                .is_err(),
            )?;
            require(
                verify_native_boot_retained_source_inputs_v20(
                    &operation,
                    &publication,
                    &stored_request,
                    &profile(vec![
                        text.clone(),
                        InstanceSpec {
                            instance: IndexInstanceId([91; 16]),
                            group: PublicationGroupId([93; 16]),
                            ..text
                        },
                    ])?,
                    &registry,
                    gate,
                    &mut budget()?,
                )
                .is_err(),
            )?;
        }
        Ok(())
    }

    fn request_rows() -> impl IntoIterator<Item = UpdateRequest> {
        let mut rows = requests();
        for request in &mut rows {
            request.expected_view.sequence = 7;
        }
        rows
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 100_000_000,
                mutations: 100_000,
            },
            deadline,
        ))
    }

    fn object(
        reference: ObjectRef,
        payload: Vec<u8>,
        children: Vec<ObjectRef>,
    ) -> Result<StoredObject, IndexError> {
        let bytes = u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?;
        Ok(StoredObject::new(
            RegisteredObject::unscoped(reference, bytes, children),
            payload,
            None,
        ))
    }

    fn require(condition: bool) -> Result<(), IndexError> {
        if condition {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "native boot independent test oracle mismatch".into(),
            ))
        }
    }

    fn fixture(
        request: &UpdateRequest,
    ) -> Result<
        (ImmutableOperation, CatalogView, StoredObject, StoredObject),
        Box<dyn std::error::Error>,
    > {
        let request_ref = ObjectRef(Digest([11; 32]));
        let profile_ref = match &request.kind {
            UpdateKind::Reconfigure { profile } => *profile,
            UpdateKind::HistoryAdvance(transition) => transition.profile,
            UpdateKind::HistoryAppend(append) => append.profile,
            UpdateKind::SourceHistoryAdvance(transition)
            | UpdateKind::SourceHistoryResync { transition, .. } => transition.history.profile,
            UpdateKind::SourceHistoryBootstrap(transition) => transition.history.profile,
            UpdateKind::SourceAdvance(_)
            | UpdateKind::SourceResync { .. }
            | UpdateKind::ProjectionAdvance(_)
            | UpdateKind::CompactSourceLocal { .. }
            | UpdateKind::Materialize { .. } => ObjectRef(Digest([12; 32])),
        };
        let mut publication = CatalogView {
            identity: quanta_index_core::index::ViewRef {
                sequence: request
                    .expected_view
                    .sequence
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?,
                ..request.expected_view
            },
            profile: profile_ref,
            source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: MapRoot(None),
            coverage: MapRoot(None),
        };
        publication.identity.digest = publication.identity_digest()?;
        let operation = ImmutableOperation {
            key: request.operation,
            request: request_ref,
            source_mode: request.kind.source_mode(),
        };
        let request_object = object(
            request_ref,
            serde_json::to_vec(request)?,
            request_preparation::children(request).as_slice().to_vec(),
        )?;
        let profile_object = object(
            profile_ref,
            serde_json::to_vec(&IndexProfile { instances: vec![] })?,
            vec![],
        )?;
        Ok((operation, publication, request_object, profile_object))
    }

    #[test]
    fn all_existing_request_wires_bind_to_selected_publication()
    -> Result<(), Box<dyn std::error::Error>> {
        for request in request_rows() {
            let (operation, publication, stored_request, stored_profile) = fixture(&request)?;
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &stored_profile,
                &AdapterRegistry::new(),
                &mut budget()?,
            )?;
        }
        Ok(())
    }

    #[test]
    fn native_initial_boot_requires_first_source_and_exact_required_history_profile()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut request = request_rows()
            .into_iter()
            .find(|request| matches!(&request.kind, UpdateKind::SourceHistoryBootstrap(_)))
            .ok_or(IndexError::Conflict)?;
        let UpdateKind::SourceHistoryBootstrap(transition) = &mut request.kind else {
            return Err(IndexError::Conflict.into());
        };
        transition.source.expected_source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let instance = transition.history.instance;
        let (operation, publication, stored_request, _) = fixture(&request)?;
        let target = IndexProfile {
            instances: vec![quanta_index_core::index::InstanceSpec {
                instance,
                kind: IndexKind([1; 16]),
                group: PublicationGroupId([1; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::History,
                schema: SchemaId([1; 16]),
                policy: Digest([1; 32]),
                format: StorageFormatId([1; 16]),
            }],
        };
        let stored_profile = object(publication.profile, serde_json::to_vec(&target)?, vec![])?;
        let mut visited = false;
        require(
            with_native_boot_source_history_initial_v1(
                &operation,
                &publication,
                &stored_request,
                &stored_profile,
                &mut budget()?,
                |history, _| {
                    visited = history.instance == instance;
                    Ok(())
                },
            )?
            .is_some()
                && visited,
        )?;
        let mut wrong = request.clone();
        let UpdateKind::SourceHistoryBootstrap(transition) = &mut wrong.kind else {
            return Err(IndexError::Conflict.into());
        };
        transition.source.expected_source.owners = MapRoot(Some(ObjectRef(Digest([9; 32]))));
        let wrong_request = object(
            operation.request,
            serde_json::to_vec(&wrong)?,
            request_preparation::children(&wrong).as_slice().to_vec(),
        )?;
        require(matches!(
            with_native_boot_source_history_initial_v1(
                &operation,
                &publication,
                &wrong_request,
                &stored_profile,
                &mut budget()?,
                |_, _| Ok(()),
            ),
            Err(IndexError::DigestConflict)
        ))?;
        Ok(())
    }

    #[test]
    fn non_materialize_requires_exact_next_sequence_and_rejects_overflow()
    -> Result<(), Box<dyn std::error::Error>> {
        for request in request_rows()
            .into_iter()
            .filter(|request| !matches!(request.kind, UpdateKind::Materialize { .. }))
        {
            let (operation, publication, stored_request, stored_profile) = fixture(&request)?;
            // Fixed request sequence 7 accepts publication 8. The same and
            // skipped publication sequences must both be refused.
            require(request.expected_view.sequence == 7)?;
            require(publication.identity.sequence == 8)?;
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &stored_profile,
                &AdapterRegistry::new(),
                &mut budget()?,
            )?;
            for sequence in [7, 9] {
                let mut wrong_publication = publication.clone();
                wrong_publication.identity.sequence = sequence;
                wrong_publication.identity.digest = wrong_publication.identity_digest()?;
                require(matches!(
                    verify_native_boot_inputs(
                        &operation,
                        &wrong_publication,
                        &stored_request,
                        &stored_profile,
                        &AdapterRegistry::new(),
                        &mut budget()?,
                    ),
                    Err(IndexError::CorruptObject(_))
                ))?;
            }
            let mut overflow = request.clone();
            overflow.expected_view.sequence = u64::MAX;
            let overflow_request = object(
                operation.request,
                serde_json::to_vec(&overflow)?,
                request_preparation::children(&overflow).as_slice().to_vec(),
            )?;
            require(matches!(
                verify_native_boot_inputs(
                    &operation,
                    &publication,
                    &overflow_request,
                    &stored_profile,
                    &AdapterRegistry::new(),
                    &mut budget()?,
                ),
                Err(IndexError::CorruptObject(_))
            ))?;
        }
        Ok(())
    }

    #[test]
    fn forged_request_and_publication_bindings_fail_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        for request in request_rows() {
            let (operation, publication, stored_request, stored_profile) = fixture(&request)?;
            let check = |operation: &ImmutableOperation,
                         publication: &CatalogView,
                         request: &StoredObject,
                         profile: &StoredObject| {
                verify_native_boot_inputs(
                    operation,
                    publication,
                    request,
                    profile,
                    &AdapterRegistry::new(),
                    &mut budget()?,
                )
            };
            let mut wrong_operation = operation;
            wrong_operation.request = ObjectRef(Digest([33; 32]));
            require(matches!(
                check(
                    &wrong_operation,
                    &publication,
                    &stored_request,
                    &stored_profile
                ),
                Err(IndexError::CorruptObject(_))
            ))?;
            wrong_operation = operation;
            wrong_operation.source_mode = match operation.source_mode {
                quanta_index_core::index::SourceOperationMode::Unchanged => {
                    quanta_index_core::index::SourceOperationMode::Advance
                }
                quanta_index_core::index::SourceOperationMode::Advance
                | quanta_index_core::index::SourceOperationMode::Resync { .. } => {
                    quanta_index_core::index::SourceOperationMode::Unchanged
                }
            };
            require(matches!(
                check(
                    &wrong_operation,
                    &publication,
                    &stored_request,
                    &stored_profile
                ),
                Err(IndexError::CorruptObject(_))
            ))?;
            wrong_operation = operation;
            wrong_operation.key.sequence = wrong_operation.key.sequence.wrapping_sub(1);
            require(matches!(
                check(
                    &wrong_operation,
                    &publication,
                    &stored_request,
                    &stored_profile
                ),
                Err(IndexError::CorruptObject(_))
            ))?;
            let wrong_edges = object(operation.request, stored_request.payload.clone(), vec![])?;
            if !stored_request.registration.children.is_empty() {
                require(matches!(
                    check(&operation, &publication, &wrong_edges, &stored_profile),
                    Err(IndexError::CorruptObject(_))
                ))?;
            }
            let mut wrong_publication = publication.clone();
            wrong_publication.identity.digest = Digest([44; 32]);
            require(matches!(
                check(
                    &operation,
                    &wrong_publication,
                    &stored_request,
                    &stored_profile
                ),
                Err(IndexError::CorruptObject(_))
            ))?;
            let wrong_profile = object(
                ObjectRef(Digest([55; 32])),
                stored_profile.payload.clone(),
                vec![],
            )?;
            require(matches!(
                check(&operation, &publication, &stored_request, &wrong_profile),
                Err(IndexError::CorruptObject(_))
            ))?;
            let mut wrong_expected = request.clone();
            wrong_expected.expected_view.workspace.slot.0 = [99; 16];
            let wrong_scope = object(
                operation.request,
                serde_json::to_vec(&wrong_expected)?,
                stored_request.registration.children.clone(),
            )?;
            require(matches!(
                check(&operation, &publication, &wrong_scope, &stored_profile),
                Err(IndexError::CorruptObject(_))
            ))?;
            if !matches!(request.kind, UpdateKind::Materialize { .. }) {
                let mut future_base = request.clone();
                future_base.expected_view.sequence = publication.identity.sequence;
                let wrong_sequence = object(
                    operation.request,
                    serde_json::to_vec(&future_base)?,
                    stored_request.registration.children.clone(),
                )?;
                require(matches!(
                    check(&operation, &publication, &wrong_sequence, &stored_profile),
                    Err(IndexError::CorruptObject(_))
                ))?;
            }
        }
        Ok(())
    }

    #[test]
    fn profile_configuration_and_reconfigure_target_are_checked()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = request_rows()
            .into_iter()
            .next()
            .ok_or(IndexError::Conflict)?;
        let (operation, publication, stored_request, stored_profile) = fixture(&request)?;
        let with_child = object(
            publication.profile,
            stored_profile.payload.clone(),
            vec![operation.request],
        )?;
        require(matches!(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &with_child,
                &AdapterRegistry::new(),
                &mut budget()?,
            ),
            Err(IndexError::CorruptObject(_))
        ))?;
        let unsupported = IndexProfile {
            instances: vec![quanta_index_core::index::InstanceSpec {
                instance: IndexInstanceId([1; 16]),
                kind: IndexKind([1; 16]),
                group: PublicationGroupId([1; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: SchemaId([1; 16]),
                policy: Digest([1; 32]),
                format: StorageFormatId([1; 16]),
            }],
        };
        let unsupported_profile = object(
            publication.profile,
            serde_json::to_vec(&unsupported)?,
            vec![],
        )?;
        require(matches!(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &unsupported_profile,
                &AdapterRegistry::new(),
                &mut budget()?,
            ),
            Err(IndexError::InvalidInput(_))
        ))?;
        let reconfigure = request_rows()
            .into_iter()
            .find(|request| matches!(request.kind, UpdateKind::Reconfigure { .. }))
            .ok_or(IndexError::Conflict)?;
        let (operation, mut publication, stored_request, _) = fixture(&reconfigure)?;
        publication.profile = ObjectRef(Digest([66; 32]));
        publication.identity.digest = publication.identity_digest()?;
        let wrong_target = object(publication.profile, stored_profile.payload.clone(), vec![])?;
        require(matches!(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &wrong_target,
                &AdapterRegistry::new(),
                &mut budget()?,
            ),
            Err(IndexError::CorruptObject(_))
        ))?;
        Ok(())
    }

    #[test]
    fn candidate_boot_refuses_retired_repomap_profile_before_registry()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = request_rows()
            .into_iter()
            .next()
            .ok_or(IndexError::Conflict)?;
        let (operation, publication, stored_request, stored_profile) = fixture(&request)?;
        verify_native_boot_candidate_inputs(
            &operation,
            &publication,
            &stored_request,
            &stored_profile,
            &AdapterRegistry::new(),
            &mut budget()?,
        )?;
        for (producer, policy) in [
            (
                quanta_index_core::index::ProducerId(*b"semantica-r5-v1!"),
                Digest([7; 32]),
            ),
            (
                quanta_index_core::index::ProducerId([8; 16]),
                quanta_index_contract::native_protocol::native_repomap_rank_policy(),
            ),
        ] {
            let retired = IndexProfile {
                instances: vec![quanta_index_core::index::InstanceSpec {
                    instance: IndexInstanceId([1; 16]),
                    kind: IndexKind([1; 16]),
                    group: PublicationGroupId([1; 16]),
                    activation: GroupActivation::CoreRequired,
                    input_domain: InputDomain::Projection {
                        producer,
                        projection: quanta_index_core::index::ProjectionId([2; 16]),
                    },
                    schema: SchemaId([1; 16]),
                    policy,
                    format: StorageFormatId([1; 16]),
                }],
            };
            let retired_profile = object(
                publication.profile,
                serde_json::to_vec(&retired)?,
                Vec::new(),
            )?;
            let error = verify_native_boot_candidate_inputs(
                &operation,
                &publication,
                &stored_request,
                &retired_profile,
                &AdapterRegistry::new(),
                &mut budget()?,
            )
            .expect_err("candidate BOOT must refuse a retired RepoMap profile");
            if !matches!(error, IndexError::InvalidInput(ref message) if message.contains("retired RepoMap"))
            {
                return Err(format!("candidate BOOT refused for another reason: {error}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn cancellation_precedes_metadata_decode() -> Result<(), Box<dyn std::error::Error>> {
        let request = request_rows()
            .into_iter()
            .next()
            .ok_or(IndexError::Conflict)?;
        let (operation, publication, stored_request, stored_profile) = fixture(&request)?;
        let mut cancelled = budget()?;
        cancelled.request().cancel_handle().cancel();
        require(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &stored_profile,
                &AdapterRegistry::new(),
                &mut cancelled,
            ) == Err(IndexError::Cancelled),
        )?;
        Ok(())
    }

    #[test]
    fn materialization_claim_cannot_cross_original_workspace_epoch()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut request = request_rows()
            .into_iter()
            .find(|request| matches!(request.kind, UpdateKind::Materialize { .. }))
            .ok_or(IndexError::Conflict)?;
        let (operation, publication, _, stored_profile) = fixture(&request)?;
        // Materialization rebases on the live claim, not the request's view
        // sequence; an old or future request sequence is not a source proof.
        request.expected_view.sequence = publication
            .identity
            .sequence
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let future_view = object(
            operation.request,
            serde_json::to_vec(&request)?,
            request_preparation::children(&request).as_slice().to_vec(),
        )?;
        verify_native_boot_inputs(
            &operation,
            &publication,
            &future_view,
            &stored_profile,
            &AdapterRegistry::new(),
            &mut budget()?,
        )?;
        let UpdateKind::Materialize { claim } = &mut request.kind else {
            return Err(IndexError::Conflict.into());
        };
        claim.intent.epoch = claim.intent.epoch.wrapping_sub(1);
        let bad_claim = object(
            operation.request,
            serde_json::to_vec(&request)?,
            request_preparation::children(&request).as_slice().to_vec(),
        )?;
        require(matches!(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &bad_claim,
                &stored_profile,
                &AdapterRegistry::new(),
                &mut budget()?,
            ),
            Err(IndexError::CorruptObject(_))
        ))?;
        Ok(())
    }

    #[test]
    fn corrupt_request_and_profile_refund_decode_custody() -> Result<(), Box<dyn std::error::Error>>
    {
        let request = request_rows()
            .into_iter()
            .next()
            .ok_or(IndexError::Conflict)?;
        let (operation, publication, _, stored_profile) = fixture(&request)?;
        let malformed_request = object(operation.request, b"{".to_vec(), vec![])?;
        let (mut work, ledger) = scoped(u64::MAX, false)?;
        require(matches!(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &malformed_request,
                &stored_profile,
                &AdapterRegistry::new(),
                &mut work,
            ),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(ledger.live() == 0)?;
        let (_, _, stored_request, _) = fixture(&request)?;
        let malformed_profile = object(publication.profile, b"{".to_vec(), vec![])?;
        require(matches!(
            verify_native_boot_inputs(
                &operation,
                &publication,
                &stored_request,
                &malformed_profile,
                &AdapterRegistry::new(),
                &mut work,
            ),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(ledger.live() == 0)?;
        Ok(())
    }
    #[test]
    fn source_history_recovery_binds_the_prior_and_published_profiles()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1;
        use quanta_index_core::index::{SourceEvent, SourceOperationMode, SourceTextAdmission};

        for mut request in request_rows().into_iter().filter(|request| {
            matches!(
                &request.kind,
                UpdateKind::SourceHistoryBootstrap(_) | UpdateKind::SourceHistoryAdvance(_)
            )
        }) {
            let key = request.operation;
            let source = match &mut request.kind {
                UpdateKind::SourceHistoryBootstrap(pair) => &mut pair.source,
                UpdateKind::SourceHistoryAdvance(pair) => &mut pair.source,
                _ => return Err(IndexError::Conflict.into()),
            };
            source.expected_source = SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            };
            source.events = vec![SourceEvent {
                producer: key.producer,
                stream: key.stream,
                stream_epoch: key.stream_epoch,
                predecessor: 0,
                sequence: 1,
                payload: Digest([71; 32]),
            }];
            let expected_source = source.expected_source;
            let (operation, publication, _, _) = fixture(&request)?;
            let mut prior = publication.clone();
            prior.identity = request.expected_view;
            prior.profile = ObjectRef(Digest([72; 32]));
            prior.source = expected_source;
            let target = NativeCutoverEvidenceTargetV1 {
                operation: key,
                expected_view: prior.identity,
                expected_source,
                expected_profile: prior.profile,
                source_mode: SourceOperationMode::Advance,
                event_predecessor: 0,
                event_sequence: 1,
                owner_identity_policy_digest: Digest([73; 32]),
                text_admission: SourceTextAdmission::Excluded,
                canonical_target_digest: Digest([74; 32]),
            };
            let work = budget()?;
            verify_native_published_source_history_target_v1(
                &target,
                &request,
                &operation,
                &publication,
                &prior,
                operation.request,
                &work,
            )?;
            let mut wrong_prior = prior.clone();
            wrong_prior.profile = publication.profile;
            require(matches!(
                verify_native_published_source_history_target_v1(
                    &target,
                    &request,
                    &operation,
                    &publication,
                    &wrong_prior,
                    operation.request,
                    &work,
                ),
                Err(IndexError::DigestConflict)
            ))?;
            wrong_prior = prior.clone();
            wrong_prior.identity.digest = Digest([75; 32]);
            require(matches!(
                verify_native_published_source_history_target_v1(
                    &target,
                    &request,
                    &operation,
                    &publication,
                    &wrong_prior,
                    operation.request,
                    &work,
                ),
                Err(IndexError::DigestConflict)
            ))?;
            let mut wrong_publication = publication.clone();
            wrong_publication.profile = prior.profile;
            require(matches!(
                verify_native_published_source_history_target_v1(
                    &target,
                    &request,
                    &operation,
                    &wrong_publication,
                    &prior,
                    operation.request,
                    &work,
                ),
                Err(IndexError::DigestConflict)
            ))?;
            require(matches!(
                verify_native_published_source_history_target_v1(
                    &target,
                    &request,
                    &operation,
                    &publication,
                    &prior,
                    prior.profile,
                    &work,
                ),
                Err(IndexError::DigestConflict)
            ))?;
        }
        Ok(())
    }

    #[test]
    fn source_resync_recovery_requires_exact_epoch_event_and_publication()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1;
        use quanta_index_core::index::{SourceEvent, SourceOperationMode, SourceTextAdmission};

        for with_history in [false, true] {
            let mut request = request_rows()
                .into_iter()
                .find(|request| {
                    if with_history {
                        matches!(request.kind, UpdateKind::SourceHistoryResync { .. })
                    } else {
                        matches!(request.kind, UpdateKind::SourceResync { .. })
                    }
                })
                .ok_or(IndexError::Conflict)?;
            let (prior_epoch, transition) = match &mut request.kind {
                UpdateKind::SourceResync {
                    prior_epoch,
                    transition,
                } => (prior_epoch, transition),
                UpdateKind::SourceHistoryResync {
                    prior_epoch,
                    transition,
                } => (prior_epoch, &mut transition.source),
                _ => return Err(IndexError::Conflict.into()),
            };
            *prior_epoch = 7;
            request.operation.stream_epoch = 8;
            let key = request.operation;
            transition.events = vec![SourceEvent {
                producer: key.producer,
                stream: key.stream,
                stream_epoch: key.stream_epoch,
                predecessor: 0,
                sequence: 1,
                payload: transition.owner_digest()?,
            }];
            let expected_source = transition.expected_source;
            let (operation, publication, _, _) = fixture(&request)?;
            let mut prior = publication.clone();
            prior.identity = request.expected_view;
            prior.source = expected_source;
            if with_history {
                prior.profile = ObjectRef(Digest([76; 32]));
            }
            let target = NativeCutoverEvidenceTargetV1 {
                operation: key,
                expected_view: prior.identity,
                expected_source,
                expected_profile: prior.profile,
                source_mode: SourceOperationMode::Resync { prior_epoch: 7 },
                event_predecessor: 0,
                event_sequence: 1,
                owner_identity_policy_digest: Digest([77; 32]),
                text_admission: SourceTextAdmission::Excluded,
                canonical_target_digest: Digest([78; 32]),
            };
            let work = budget()?;
            let validate = |target: &NativeCutoverEvidenceTargetV1,
                            request: &UpdateRequest,
                            operation: &ImmutableOperation,
                            request_ref| {
                if with_history {
                    verify_native_published_source_history_target_v1(
                        target,
                        request,
                        operation,
                        &publication,
                        &prior,
                        request_ref,
                        &work,
                    )
                } else {
                    verify_native_published_source_target_v1(
                        target,
                        request,
                        operation,
                        publication.identity,
                        publication.profile,
                        request_ref,
                        &work,
                    )
                }
            };
            validate(&target, &request, &operation, operation.request)?;
            require(matches!(
                validate(&target, &request, &operation, ObjectRef(Digest([79; 32]))),
                Err(IndexError::DigestConflict)
            ))?;
            let mut wrong_request = request.clone();
            let mut wrong_target = target;
            let mut wrong_operation = operation;
            match &mut wrong_request.kind {
                UpdateKind::SourceResync { prior_epoch, .. }
                | UpdateKind::SourceHistoryResync { prior_epoch, .. } => {
                    *prior_epoch = 6;
                }
                _ => return Err(IndexError::Conflict.into()),
            }
            wrong_target.source_mode = wrong_request.kind.source_mode();
            wrong_operation.source_mode = wrong_target.source_mode;
            require(matches!(
                validate(
                    &wrong_target,
                    &wrong_request,
                    &wrong_operation,
                    operation.request
                ),
                Err(IndexError::DigestConflict)
            ))?;
            let mut shifted_request = request.clone();
            let shifted_source = match &mut shifted_request.kind {
                UpdateKind::SourceResync { transition, .. } => transition,
                UpdateKind::SourceHistoryResync { transition, .. } => &mut transition.source,
                _ => return Err(IndexError::Conflict.into()),
            };
            let event = shifted_source
                .events
                .first_mut()
                .ok_or(IndexError::Conflict)?;
            event.predecessor = 1;
            event.sequence = 2;
            let shifted_target = NativeCutoverEvidenceTargetV1 {
                event_predecessor: 1,
                event_sequence: 2,
                ..target
            };
            require(matches!(
                validate(
                    &shifted_target,
                    &shifted_request,
                    &operation,
                    operation.request
                ),
                Err(IndexError::DigestConflict)
            ))?;
            if !with_history {
                require(matches!(
                    verify_native_published_full_target_v1(
                        &target,
                        &request,
                        &operation,
                        publication.identity,
                        publication.profile,
                        operation.request,
                        &work,
                    ),
                    Err(IndexError::DigestConflict)
                ))?;
            }
        }
        Ok(())
    }

    #[test]
    fn original_full_target_binds_actual_request_and_published_profile()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1;
        use quanta_index_core::index::{SourceEvent, SourceOperationMode};
        let mut request = request_rows()
            .into_iter()
            .find(|request| matches!(request.kind, UpdateKind::SourceAdvance(_)))
            .ok_or("missing SourceAdvance fixture")?;
        let key = request.operation;
        let UpdateKind::SourceAdvance(transition) = &mut request.kind else {
            return Err("unexpected request fixture".into());
        };
        transition.events = vec![SourceEvent {
            producer: key.producer,
            stream: key.stream,
            stream_epoch: key.stream_epoch,
            predecessor: 0,
            sequence: 1,
            payload: Digest([71; 32]),
        }];
        let prior_source = transition.expected_source;
        let (operation, publication, stored_request, _) = fixture(&request)?;
        let target = NativeCutoverEvidenceTargetV1 {
            operation: key,
            expected_view: request.expected_view,
            expected_source: prior_source,
            expected_profile: publication.profile,
            source_mode: SourceOperationMode::Advance,
            event_predecessor: 0,
            event_sequence: 1,
            owner_identity_policy_digest: Digest([72; 32]),
            text_admission: quanta_index_core::index::SourceTextAdmission::Excluded,
            canonical_target_digest: Digest([73; 32]),
        };
        verify_native_boot_source_target_v1(
            &target,
            &request,
            &operation,
            &publication,
            &stored_request,
            &mut budget()?,
        )?;
        let mut wrong = target;
        wrong.expected_view.sequence = wrong.expected_view.sequence.saturating_add(1);
        require(matches!(
            verify_native_boot_source_target_v1(
                &wrong,
                &request,
                &operation,
                &publication,
                &stored_request,
                &mut budget()?,
            ),
            Err(IndexError::DigestConflict)
        ))?;
        wrong = target;
        wrong.expected_source.paths = MapRoot(Some(ObjectRef(Digest([81; 32]))));
        require(matches!(
            verify_native_boot_source_target_v1(
                &wrong,
                &request,
                &operation,
                &publication,
                &stored_request,
                &mut budget()?,
            ),
            Err(IndexError::DigestConflict)
        ))?;
        wrong = target;
        wrong.expected_profile = ObjectRef(Digest([82; 32]));
        require(matches!(
            verify_native_boot_source_target_v1(
                &wrong,
                &request,
                &operation,
                &publication,
                &stored_request,
                &mut budget()?,
            ),
            Err(IndexError::DigestConflict)
        ))?;
        wrong = target;
        wrong.event_sequence = 2;
        require(matches!(
            verify_native_boot_source_target_v1(
                &wrong,
                &request,
                &operation,
                &publication,
                &stored_request,
                &mut budget()?,
            ),
            Err(IndexError::DigestConflict)
        ))?;
        // The selection axes stay identical. Only the Original capture body
        // changes; those mismatches must not pass the target-only checks.
        for change_event in [false, true] {
            let mut wrong_capture = request.clone();
            let UpdateKind::SourceAdvance(transition) = &mut wrong_capture.kind else {
                return Err("unexpected request fixture".into());
            };
            if change_event {
                let event = transition.events.first_mut().ok_or("missing FULL event")?;
                event.payload = Digest([91; 32]);
            } else {
                transition.owners.digest = Digest([92; 32]);
            }
            require(matches!(
                verify_native_boot_source_target_v1(
                    &target,
                    &wrong_capture,
                    &operation,
                    &publication,
                    &stored_request,
                    &mut budget()?,
                ),
                Err(IndexError::DigestConflict)
            ))?;
        }
        let mut multiple = request.clone();
        let UpdateKind::SourceAdvance(transition) = &mut multiple.kind else {
            return Err("unexpected request fixture".into());
        };
        transition.events.push(SourceEvent {
            producer: key.producer,
            stream: key.stream,
            stream_epoch: key.stream_epoch,
            predecessor: 1,
            sequence: 2,
            payload: Digest([83; 32]),
        });
        let (_, _, multiple_request, _) = fixture(&multiple)?;
        require(matches!(
            verify_native_boot_source_target_v1(
                &target,
                &request,
                &operation,
                &publication,
                &multiple_request,
                &mut budget()?,
            ),
            Err(IndexError::DigestConflict)
        ))?;
        Ok(())
    }
    #[test]
    fn captured_request_verifier_checks_each_existing_update_kind()
    -> Result<(), Box<dyn std::error::Error>> {
        for request in request_rows() {
            let (operation, _publication, stored, _profile) = fixture(&request)?;
            verify_native_boot_captured_request_v1(&request, &operation, &stored, &mut budget()?)?;
            let mut different = request;
            different.expected_view.sequence = different.expected_view.sequence.saturating_add(1);
            require(matches!(
                verify_native_boot_captured_request_v1(
                    &different,
                    &operation,
                    &stored,
                    &mut budget()?,
                ),
                Err(IndexError::DigestConflict)
            ))?;
        }
        Ok(())
    }
}
