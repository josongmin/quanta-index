//! Read-only validation of the selected operation's durable request and profile.
//! The caller authenticates their physical object bytes and selected catalog row.

use quanta_index_core::index::{
    CatalogView, HistoryTransition, ImmutableOperation, IndexError, StoredObject, UpdateKind,
    WorkBudget,
};

use crate::{AdapterRegistry, profile, request_decode, request_preparation};

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
        budget,
    )
}

fn verify_native_boot_inputs_with_policy(
    operation: &ImmutableOperation,
    publication: &CatalogView,
    request: &StoredObject,
    profile: &StoredObject,
    registry: &AdapterRegistry,
    candidate: bool,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
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
    registry.validate_profile(&loaded.instances, budget)?;
    budget.checkpoint()
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
    if target.event_predecessor != 0 || target.event_sequence != 1 {
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

/// Bind the independently identified current SourceAdvance request to its
/// actual Published metadata. This supports the current FULL/DELTA reservation;
/// Original FULL remains the strict initial-event predicate above. No Source,
/// activation, installation or deployment permission is issued here.
pub fn verify_native_published_source_target_v1(
    target: &quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceTargetV1,
    source_request: &quanta_index_core::index::UpdateRequest,
    operation: &ImmutableOperation,
    published: quanta_index_core::index::ViewRef,
    selected_profile: quanta_index_core::index::ObjectRef,
    identified_request: quanta_index_core::index::ObjectRef,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    use quanta_index_core::index::SourceOperationMode;
    budget.checkpoint()?;
    let UpdateKind::SourceAdvance(transition) = &source_request.kind else {
        return Err(IndexError::DigestConflict);
    };
    let [event] = transition.events.as_slice() else {
        return Err(IndexError::DigestConflict);
    };
    if target.operation != operation.key
        || source_request.operation != operation.key
        || identified_request != operation.request
        || target.source_mode != SourceOperationMode::Advance
        || operation.source_mode != target.source_mode
        || source_request.expected_view != target.expected_view
        || transition.expected_source != target.expected_source
        || selected_profile != target.expected_profile
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
