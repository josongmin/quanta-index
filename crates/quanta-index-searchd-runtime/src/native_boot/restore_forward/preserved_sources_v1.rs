//! Join the complete held Legacy Source census to Original-issued imported
//! snapshots and the candidate's actual Native FULL publications.

use super::*;
use crate::native_product::legacy_source_census_v1::{
    collect_active_source_heads_v1, verify_active_source_head_files_v1,
};
use quanta_index_core::index::{
    Digest, IndexProfile, MapValue, MemoryLifetime, NativeProtectedSourceRoutesV1, WorkUsage,
    member_map_key, with_native_protected_source_routes_v1,
};

pub(super) struct VerifiedPreservedSourcesV1 {
    pub has_history_rows: bool,
    repomap_files: Vec<(
        quanta_index_repomap::VerifiedActiveRepoMapFilesV1,
        MemoryLease,
    )>,
}

impl VerifiedPreservedSourcesV1 {
    pub(super) fn require_current_v1(
        &self,
        legacy: &legacy_read_factory::LegacyReadAuthorityV1,
        budget: &mut WorkBudget,
    ) -> Result<()> {
        for (files, _) in &self.repomap_files {
            files.require_current_v1(legacy.root(), legacy.objects(), budget)?;
        }
        legacy.require_current_and_retired(budget)
    }
}

pub(super) fn verify_preserved_sources_v1(
    legacy: &legacy_read_factory::LegacyReadAuthorityV1,
    native: &NativeBootAuthority,
    command: &NativeStateCommandV1,
    registry: &AdapterRegistry,
    source: &RecoverySourceInputs<'_>,
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    maximum: u64,
    budget: &mut WorkBudget,
) -> Result<Option<VerifiedPreservedSourcesV1>> {
    let expected = command.identity.ok_or(IndexError::SnapshotUnavailable)?;
    let Some(reference) = expected.protected_source_routes else {
        if joined.imported_legacy_source_count_v1() != 0 {
            return Err(IndexError::DigestConflict.into());
        }
        return Ok(None);
    };
    let root = native.lease.state_root_identity_v1();
    let snapshot = verify_native_boot_catalog(root, native.io.as_ref(), budget)?;
    let selected = snapshot
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?;
    if identity(snapshot.root_nonce, selected)? != expected {
        return Err(IndexError::DigestConflict.into());
    }
    authenticate(
        native, &snapshot, selected, registry, maximum, None, false, budget,
    )?;
    let route = selected
        .protected_source_routes
        .ok_or(IndexError::SnapshotUnavailable)?;
    let objects = root.join("native-objects");
    let io = Arc::new(NativeObjectReadIo::new(Arc::clone(&native.physical)));
    let lock = io.open(&objects.join(".object-namespace.lease"), budget)?;
    let reader = FileObjectReader::open_existing(&objects, lock, io, budget)?;
    if snapshot.namespace != Some(reader.namespace_binding()?) {
        return Err(IndexError::DigestConflict.into());
    }
    let stored = reader.get(reference, budget)?;
    let census = collect_active_source_heads_v1(
        legacy.root(),
        legacy.catalog(),
        legacy.objects(),
        legacy.metadata(),
        budget,
    )?;
    let publication = source
        .legacy_publication
        .ok_or(MissingLegacyPublicationInputV1)?;
    let primary = census
        .heads
        .iter()
        .find(|head| {
            head.repo_id == publication.activation.active.generation.lexical.repo_id
                && head.revision_id == publication.activation.active.generation.lexical.revision_id
        })
        .ok_or(IndexError::SnapshotUnavailable)?;
    if primary.active != publication.activation.active
        || primary.applied_ack.as_ref() != Some(&publication.receipt)
        || (publication.history.is_some() && !census.catalog.has_history_rows)
    {
        return Err(IndexError::DigestConflict.into());
    }
    let repomap_files = with_native_protected_source_routes_v1(
        reference,
        Some(route.count),
        &stored,
        budget,
        |routes, budget| {
            (|| -> Result<_> {
                verify_sources_v1(
                    legacy,
                    native,
                    &reader,
                    snapshot.root_nonce,
                    selected,
                    routes,
                    &census.heads,
                    registry,
                    joined,
                    budget,
                )?;
                verify_repomaps_v1(
                    legacy,
                    &reader,
                    routes,
                    &census.repomap_heads,
                    &publication.repomap,
                    budget,
                )
            })()
            .map_err(|cause| {
                cause
                    .downcast::<IndexError>()
                    .unwrap_or_else(|cause| IndexError::Storage(cause.to_string()))
            })
        },
    )?;
    legacy.require_current_root_v1()?;
    require_retired(native)?;
    Ok(Some(VerifiedPreservedSourcesV1 {
        has_history_rows: census.catalog.has_history_rows,
        repomap_files,
    }))
}

fn verify_sources_v1(
    legacy: &legacy_read_factory::LegacyReadAuthorityV1,
    native: &NativeBootAuthority,
    reader: &FileObjectReader,
    nonce: [u8; 32],
    selected: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    old: &[crate::native_product::legacy_source_census_v1::ActiveSourceHeadV1],
    registry: &AdapterRegistry,
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    budget: &mut WorkBudget,
) -> Result<()> {
    if old.len() != routes.heads.len() || joined.imported_legacy_source_count_v1() != old.len() {
        return Err(IndexError::DigestConflict.into());
    }
    quanta_index_catalog::verify_native_protected_source_routes_if_current_v1(
        native.lease.state_root_identity_v1(),
        native.io.as_ref(),
        selected,
        routes,
        budget,
    )?;
    for (old, routed) in old.iter().zip(&routes.heads) {
        let (imported, target, request) = joined
            .imported_legacy_source_inputs_for_pair_v1(&old.repo_id, &old.revision_id)
            .ok_or(IndexError::SnapshotUnavailable)?;
        let ack = old
            .applied_ack
            .as_ref()
            .ok_or(IndexError::SnapshotUnavailable)?;
        if old.repo_id != routed.repo_id
            || old.revision_id != routed.revision_id
            || imported.origin.active != old.active
            || &imported.origin.applied_ack != ack
            || imported.candidate_root_nonce != Digest(nonce)
            || imported.source_operation != routed.source_operation
            || imported.source_publication != routed.source_publication
        {
            return Err(IndexError::DigestConflict.into());
        }
        verify_active_source_head_files_v1(
            legacy.root(),
            old,
            legacy.objects(),
            legacy.metadata(),
            budget,
        )?;
        let observed_files = quanta_index_lexical::legacy::visit_existing_complete_source_files_v1(
            &legacy.root().join("indexes/lexical"),
            &old.active.generation.lexical,
            rustix::process::geteuid().as_raw(),
            legacy.maximum_objects(),
            legacy.metadata(),
            legacy.objects(),
            budget,
            &mut |event, heads, _budget| {
                if event != &imported.origin.source_publication_event
                    || heads != imported.origin.source_heads
                {
                    return Err(IndexError::DigestConflict);
                }
                Ok(())
            },
            &mut |source, text_admitted, body, budget| {
                reader.with_repository_source_file_v1(
                    &imported.source_publication.source,
                    &source.file.source_repo_id,
                    source.file.repo_relative_path.as_str(),
                    budget,
                    |_owner, record, actual, budget| {
                        budget.charge(WorkUsage {
                            bytes: u64::try_from(body.len())
                                .map_err(|_| IndexError::ResourceLimited)?,
                            ..Default::default()
                        })?;
                        if actual != body
                            || text_admitted
                                != matches!(
                                    record.text,
                                    quanta_index_core::index::SourceTextAdmission::Utf8 { .. }
                                )
                        {
                            return Err(IndexError::DigestConflict);
                        }
                        Ok(())
                    },
                )
            },
        ).map_err(|cause| match cause {
            quanta_index_lexical::legacy::SourceVisitFailureV1::Visitor(cause) => cause,
            quanta_index_lexical::legacy::SourceVisitFailureV1::Physical(cause) => match cause {
                quanta_index_core::index::existing_file::ExistingFileReadFailureV1::AdmissionOrIo(cause) => cause,
                quanta_index_core::index::existing_file::ExistingFileReadFailureV1::Canonical(cause) =>
                    IndexError::Storage(cause.to_string()),
            },
        })?;
        if observed_files != imported.origin.source_heads.entries {
            return Err(IndexError::DigestConflict.into());
        }
        let actual = quanta_index_catalog::verify_native_published_operation(
            native.lease.state_root_identity_v1(),
            native.io.as_ref(),
            imported.source_operation.key,
            budget,
        )?;
        if actual.snapshot().root_nonce != nonce
            || actual.operation() != &imported.source_operation
            || actual.publication_metadata() != &imported.source_publication
            || actual.snapshot().selected.as_ref() != Some(selected)
        {
            return Err(IndexError::DigestConflict.into());
        }
        let stored_request = reader.get(imported.source_operation.request, budget)?;
        let profile = reader.get(imported.source_publication.profile, budget)?;
        quanta_index_kernel::verify_native_boot_inputs(
            &imported.source_operation,
            &imported.source_publication,
            &stored_request,
            &profile,
            registry,
            budget,
        )?;
        quanta_index_kernel::verify_native_boot_captured_request_v1(
            request,
            &imported.source_operation,
            &stored_request,
            budget,
        )?;
        quanta_index_kernel::verify_native_published_full_target_v1(
            target,
            request,
            &imported.source_operation,
            imported.source_publication.identity,
            imported.source_publication.profile,
            imported.source_operation.request,
            budget,
        )?;
        if reader.source_file_count_v1(&imported.source_publication.source, budget)?
            != imported.origin.source_heads.entries
        {
            return Err(IndexError::DigestConflict.into());
        }
    }
    Ok(())
}

fn verify_repomaps_v1(
    legacy: &legacy_read_factory::LegacyReadAuthorityV1,
    reader: &FileObjectReader,
    routes: &NativeProtectedSourceRoutesV1,
    old: &[crate::native_product::legacy_source_census_v1::ActiveRepoMapHeadV1],
    disposition: &RepoMap,
    budget: &mut WorkBudget,
) -> Result<
    Vec<(
        quanta_index_repomap::VerifiedActiveRepoMapFilesV1,
        MemoryLease,
    )>,
> {
    if old.len() != routes.repomap_heads.len() {
        return Err(IndexError::DigestConflict.into());
    }
    let mut verified = Vec::new();
    for (old, routed) in old.iter().zip(&routes.repomap_heads) {
        if old.activation.repo_id != routed.repo_id.as_str()
            || old.activation.revision_id != routed.revision_id.as_str()
            || old.activation.epoch != routed.active.epoch().get()
            || old.activation.candidate_commitment
                != *routed.active.candidate_commitment().as_bytes()
        {
            return Err(IndexError::DigestConflict.into());
        }
        let memory = budget
            .reserve_memory(4_096, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        verified
            .try_reserve(1)
            .map_err(|_| IndexError::ResourceLimited)?;
        if reader.lookup_map_value_v1(
            routed.publication.members,
            &member_map_key(routed.instance),
            budget,
        )? != Some(MapValue::Object(routed.member_binding))
        {
            return Err(IndexError::DigestConflict.into());
        }
        let stored = reader.get(routed.member_binding, budget)?;
        let member = quanta_index_core::index::decode_member_binding(&stored.payload, budget)?;
        if stored.registration.reference != routed.member_binding
            || !stored.registration.children.iter().copied().eq([
                Some(member.artifact),
                member.input.root.0,
            ]
            .into_iter()
            .flatten())
        {
            return Err(IndexError::DigestConflict.into());
        }
        let profile = reader.get(routed.publication.profile, budget)?;
        if profile.registration.reference != routed.publication.profile
            || !profile.registration.children.is_empty()
        {
            return Err(IndexError::DigestConflict.into());
        }
        let bytes = u64::try_from(profile.payload.len())?;
        let _profile_memory = budget
            .reserve_memory(
                bytes
                    .checked_mul(32)
                    .and_then(|n| n.checked_add(65_536))
                    .ok_or(IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..Default::default()
        })?;
        let profile: IndexProfile = serde_json::from_slice(&profile.payload)?;
        let instance = profile
            .instances
            .iter()
            .find(|spec| spec.instance == routed.instance)
            .ok_or(IndexError::DigestConflict)?;
        let files = quanta_index_repomap::native::with_verified_native_legacy_repomap_member_v1(
            &member,
            instance,
            routed.publication.source,
            |reference, budget| reader.get(reference, budget),
            budget,
            |header, budget| {
                if header.repo_id != routed.repo_id
                    || header.revision_id != routed.revision_id
                    || header.old_active != routed.active
                    || header.generation.get() != old.candidate.manifest_generation
                {
                    return Err(IndexError::DigestConflict);
                }
                let receipts = match disposition {
                    RepoMap::Required {
                        state_root,
                        publish,
                        activation,
                        ..
                    } if std::path::Path::new(state_root) == legacy.root()
                        && publish.mutation.repo_id == routed.repo_id
                        && publish.mutation.revision_id == routed.revision_id =>
                    {
                        Some((publish, activation))
                    }
                    _ => None,
                };
                legacy
                    .visit_active_repomap_compiled_v1(
                        &old.candidate,
                        receipts,
                        budget,
                        &mut |payload, budget| {
                            use sha2::Digest as _;
                            budget.charge(WorkUsage {
                                bytes: u64::try_from(payload.len())
                                    .map_err(|_| IndexError::ResourceLimited)?,
                                ..Default::default()
                            })?;
                            if u64::try_from(payload.len())
                                .map_err(|_| IndexError::ResourceLimited)?
                                != header.payload_bytes
                                || Digest(sha2::Sha256::digest(payload).into())
                                    != header.payload_sha256
                            {
                                return Err(IndexError::DigestConflict.into());
                            }
                            Ok(())
                        },
                    )
                    .map_err(|cause| {
                        cause
                            .downcast::<IndexError>()
                            .unwrap_or_else(|cause| IndexError::Storage(cause.to_string()))
                    })
            },
        )?;
        verified.push((files, memory));
    }
    Ok(verified)
}
