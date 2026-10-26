//! Read-only validation of the selected operation's durable request and profile.
//! The caller authenticates their physical object bytes and selected catalog row.

use quanta_index_core::index::{
    CatalogView, ImmutableOperation, IndexError, StoredObject, UpdateKind, WorkBudget,
};

use crate::{AdapterRegistry, profile, request_decode, request_preparation};

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
    budget.checkpoint()?;
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
    let decoded = request_decode::decode(&request.payload, budget)?;
    // Materialization rebases over intervening publications and its claimed
    // owner scope is checked below. Other update kinds require an exact base.
    let exact_base = !matches!(&decoded.kind, UpdateKind::Materialize { .. });
    if decoded.operation != operation.key
        || decoded.kind.source_mode() != operation.source_mode
        || decoded.expected_view.workspace != operation.key.workspace
        || decoded.expected_view.epoch != operation.key.epoch
        || (exact_base && decoded.expected_view.sequence >= publication.identity.sequence)
        || request.registration.children.as_slice()
            != request_preparation::children(&decoded).as_slice()
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
    if profile.registration.reference != publication.profile
        || !profile.registration.children.is_empty()
    {
        return Err(IndexError::CorruptObject(
            "boot profile reference or edges mismatch".into(),
        ));
    }
    let loaded = profile::decode_payload(&profile.payload, budget)?;
    registry.validate_profile(&loaded.instances, budget)?;
    budget.checkpoint()
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

    fn request_rows() -> [UpdateRequest; 5] {
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
            UpdateKind::SourceAdvance(_)
            | UpdateKind::SourceResync { .. }
            | UpdateKind::ProjectionAdvance(_)
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
}
