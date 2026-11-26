//! Match a carried Native route and its actual Catalog ledger against the held
//! previous root. Candidate-owned objects cannot establish their own provenance.

use super::super::retired_policy_profile_v1::old_repomap_identity_v1;
use super::*;
use quanta_index_contract::index::{
    Digest, NativeImportedNativeSourceHeadV1, NativeSourceOriginV1, RetiredProjectionIdentityV1,
    RetiredProjectionPolicyErrorV1, RetiredProjectionPolicyViewV1, SourceCut, SourceTextAdmission,
    decode_retired_projection_policy_v1, preflight_retired_projection_policy_v1,
};
use quanta_index_contract::{RepoRelativePath, RevisionId, SourceFileKey, SourceFileRevision};
use quanta_index_core::index::{
    MapPageEntry, MapValue, MemoryLifetime, SourceRecordOwner, WorkUsage, owner_from_map_key,
    with_native_protected_source_routes_v1,
};
use sha2::{Digest as _, Sha256};

struct CensusJsonCountV1(usize);

impl std::io::Write for CensusJsonCountV1 {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or(std::io::ErrorKind::OutOfMemory)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct CensusJsonDigestV1<'a>(&'a mut Sha256);

impl std::io::Write for CensusJsonDigestV1<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn source_file_revision_v1(
    repo: &quanta_index_contract::RepoId,
    path: &str,
    bytes: &[u8],
) -> std::result::Result<SourceFileRevision, IndexError> {
    // The held Source lookup checks this too; keep the copy bound local to this producer.
    if path.len() > quanta_index_contract::index::MAX_SOURCE_PATH_BYTES {
        return Err(IndexError::DigestConflict);
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let source_sha256: [u8; 32] = Sha256::digest(bytes).into();
    let mut revision = String::from("native-source-sha256:");
    for byte in source_sha256 {
        revision.push(char::from(HEX[usize::from(byte >> 4)]));
        revision.push(char::from(HEX[usize::from(byte & 15)]));
    }
    let source = SourceFileRevision {
        file: SourceFileKey {
            source_repo_id: repo.clone(),
            repo_relative_path: RepoRelativePath::new(path.to_owned()),
        },
        revision_id: RevisionId::new(revision).map_err(|cause| {
            IndexError::InvalidInput(format!("Native Source file identity: {cause}").into())
        })?,
        source_sha256,
    };
    source
        .validate()
        .map_err(|cause| IndexError::InvalidInput(cause.into()))?;
    Ok(source)
}

/// Compare all old owner/path/body pairs to candidate Source and reject extras.
/// The returned digest uses the Native export's canonical per-head file census.
fn verify_source_files_v1(
    old_reader: &FileObjectReader,
    new_reader: &FileObjectReader,
    old_cut: &SourceCut,
    new_cut: &SourceCut,
    budget: &mut WorkBudget,
) -> std::result::Result<(u64, Digest), IndexError> {
    let expected = old_reader.source_file_count_v1(old_cut, budget)?;
    if new_reader.source_file_count_v1(new_cut, budget)? != expected {
        return Err(IndexError::DigestConflict);
    }
    let mut digest = Sha256::new();
    digest.update(b"quanta-index/native-source-file-census/v1\0");
    let mut after: Option<MapPageEntry> = None;
    let mut count = 0_u64;
    loop {
        let page = old_reader.scan_map(
            old_cut.owners,
            after.as_ref().map(|entry| entry.key()).transpose()?,
            128,
            budget,
        )?;
        for (key, value) in &page.entries {
            let owner = owner_from_map_key(key)?;
            let MapValue::Object(reference) = value else {
                return Err(IndexError::DigestConflict);
            };
            let stored = old_reader.get(*reference, budget)?;
            let record =
                SourceRecordOwner::decode_json(&stored.payload, budget, MemoryLifetime::Temporary)?;
            let repo = record
                .source_repo_id
                .as_ref()
                .ok_or(IndexError::DigestConflict)?;
            if stored.registration.children.as_slice() != [record.bytes] {
                return Err(IndexError::DigestConflict);
            }
            old_reader.with_repository_source_file_v1(
                old_cut,
                repo,
                &record.path,
                budget,
                |old_owner, original, old_bytes, budget| {
                    if old_owner != owner || original != &*record {
                        return Err(IndexError::DigestConflict);
                    }
                    new_reader.with_repository_source_file_v1(
                        new_cut,
                        repo,
                        &record.path,
                        budget,
                        |_new_owner, imported, new_bytes, budget| {
                            budget.charge(WorkUsage {
                                bytes: u64::try_from(old_bytes.len())
                                    .map_err(|_| IndexError::ResourceLimited)?,
                                ..Default::default()
                            })?;
                            if old_bytes != new_bytes
                                || matches!(original.text, SourceTextAdmission::Utf8 { .. })
                                    != matches!(imported.text, SourceTextAdmission::Utf8 { .. })
                                || original.language != imported.language
                            {
                                return Err(IndexError::DigestConflict);
                            }
                            Ok(())
                        },
                    )?;
                    let language = original.language.as_ref().map(|code| code.as_str());
                    let identity_bytes = repo
                        .as_str()
                        .len()
                        .checked_add(record.path.len())
                        .and_then(|length| length.checked_add(language.map_or(0, str::len)))
                        .and_then(|length| length.checked_mul(8))
                        .and_then(|length| length.checked_add(4096))
                        .ok_or(IndexError::ResourceLimited)?;
                    let _memory = budget
                        .reserve_memory(
                            u64::try_from(identity_bytes)
                                .map_err(|_| IndexError::ResourceLimited)?,
                            MemoryLifetime::Temporary,
                        )?
                        .ok_or(IndexError::ResourceLimited)?;
                    let source = source_file_revision_v1(repo, &record.path, old_bytes)?;
                    let mut encoded_len = CensusJsonCountV1(0);
                    serde_json::to_writer(&mut encoded_len, &source)
                        .map_err(|cause| IndexError::Storage(cause.to_string()))?;
                    if encoded_len.0 > identity_bytes {
                        return Err(IndexError::ResourceLimited);
                    }
                    digest.update(
                        u32::try_from(encoded_len.0)
                            .map_err(|_| IndexError::ResourceLimited)?
                            .to_le_bytes(),
                    );
                    serde_json::to_writer(CensusJsonDigestV1(&mut digest), &source)
                        .map_err(|cause| IndexError::Storage(cause.to_string()))?;
                    digest.update([u8::from(matches!(
                        original.text,
                        SourceTextAdmission::Utf8 { .. }
                    ))]);
                    digest.update(
                        u64::try_from(old_bytes.len())
                            .map_err(|_| IndexError::ResourceLimited)?
                            .to_le_bytes(),
                    );
                    if let Some(language) = language {
                        digest.update(
                            u32::try_from(language.len())
                                .map_err(|_| IndexError::ResourceLimited)?
                                .to_le_bytes(),
                        );
                        digest.update(language.as_bytes());
                    } else {
                        digest.update(u32::MAX.to_le_bytes());
                    }
                    Ok(())
                },
            )?;
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        if !page.has_more {
            break;
        }
        let next = page.cursor().last().ok_or(IndexError::ResourceLimited)?;
        if after
            .as_ref()
            .map(|entry| entry.key())
            .transpose()?
            .is_some_and(|prior| next.key().is_ok_and(|key| key <= prior))
        {
            return Err(IndexError::DigestConflict);
        }
        after = Some(next);
    }
    if count != expected {
        return Err(IndexError::DigestConflict);
    }
    Ok((count, Digest(digest.finalize().into())))
}

fn reject_retired_candidate_publications_v1(
    candidate: &NativeBootAuthority,
    reader: &FileObjectReader,
    expected: &quanta_index_catalog::NativeBootQuiescentCatalogV1,
    routes: Option<&quanta_index_core::index::NativeProtectedSourceRoutesV1>,
    policy: &RetiredProjectionPolicyViewV1<'_>,
    sorted_instances: &[quanta_index_core::index::IndexInstanceId],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    // There is no predecessor identity to compare. Static retired producer
    // and policy checks still run at candidate ingress and BOOT authentication.
    if policy.len_v1() == 0 {
        return Ok(());
    }
    let root = candidate.lease.state_root_identity_v1();
    let workspace = expected
        .snapshot
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?
        .operation
        .key
        .workspace;
    let mut visit = |snapshot: &quanta_index_catalog::NativeBootCatalogSnapshot,
                     _catalog: &quanta_index_catalog::NativeBootPublicationReader<'_>,
                     _head: &quanta_index_core::index::CatalogView,
                     operation: &quanta_index_core::index::ImmutableOperation,
                     publication: &quanta_index_core::index::CatalogView,
                     budget: &mut WorkBudget|
     -> Result<(), IndexError> {
        if snapshot.root_nonce != expected.snapshot.root_nonce
            || snapshot.selected != expected.snapshot.selected
            || snapshot.namespace != expected.snapshot.namespace
        {
            return Err(IndexError::DigestConflict);
        }
        // Retained publications may outlive request objects. Without those
        // bytes this scan cannot prove that an old ID was never published.
        let stored_request = reader.get(operation.request, budget)?;
        quanta_index_kernel::with_decoded_update_request_v1(
            &stored_request.payload,
            budget,
            |request, budget| -> Result<(), IndexError> {
                if request.operation != operation.key
                    || request.kind.source_mode() != operation.source_mode
                    || request.expected_view.workspace != operation.key.workspace
                    || request.expected_view.epoch != operation.key.epoch
                {
                    return Err(IndexError::DigestConflict);
                }
                if let quanta_index_core::index::UpdateKind::ProjectionAdvance(transition) =
                    &request.kind
                {
                    if super::super::retired_policy_profile_v1::retired_projection_present_v1(
                        policy,
                        transition.projection,
                        budget,
                    )? {
                        return Err(IndexError::DigestConflict);
                    }
                }
                Ok(())
            },
        )??;
        let profile = reader.get(publication.profile, budget)?;
        if !profile.registration.children.is_empty() {
            return Err(IndexError::DigestConflict);
        }
        super::super::retired_policy_profile_v1::reject_candidate_profile_overlap_v1(
            policy,
            sorted_instances,
            &profile.payload,
            budget,
        )?;
        reject_retired_map_keys_v1(
            reader,
            publication.projections,
            policy,
            sorted_instances,
            true,
            budget,
        )?;
        reject_retired_map_keys_v1(
            reader,
            publication.members,
            policy,
            sorted_instances,
            false,
            budget,
        )?;
        reject_retired_map_keys_v1(
            reader,
            publication.coverage,
            policy,
            sorted_instances,
            false,
            budget,
        )?;
        Ok(())
    };
    let observed = if let Some(routes) = routes {
        quanta_index_catalog::verify_native_source_root_publications_with_routes_v1(
            root,
            candidate.io.as_ref(),
            workspace,
            routes,
            budget,
            &mut visit,
        )?
    } else {
        quanta_index_catalog::verify_native_source_root_publications_v1(
            root,
            candidate.io.as_ref(),
            workspace,
            budget,
            &mut visit,
        )?
    };
    if observed.snapshot.root_nonce != expected.snapshot.root_nonce
        || observed.snapshot.selected != expected.snapshot.selected
        || observed.snapshot.namespace != expected.snapshot.namespace
        || observed.current_head != expected.current_head
    {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

fn reject_retired_map_keys_v1(
    reader: &FileObjectReader,
    root: quanta_index_core::index::MapRoot,
    policy: &RetiredProjectionPolicyViewV1<'_>,
    sorted_instances: &[quanta_index_core::index::IndexInstanceId],
    projection_map: bool,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if root.0.is_none() {
        return Ok(());
    }
    let mut after: Option<MapPageEntry> = None;
    loop {
        let page = reader.scan_map(
            root,
            after.as_ref().map(|entry| entry.key()).transpose()?,
            128,
            budget,
        )?;
        for (key, _) in &page.entries {
            budget.checkpoint()?;
            if projection_map {
                if key.len() == 27 && key.starts_with(b"projection\0") {
                    let mut projection = [0_u8; 16];
                    projection.copy_from_slice(&key[11..]);
                    if super::super::retired_policy_profile_v1::retired_projection_present_v1(
                        policy,
                        quanta_index_core::index::ProjectionId(projection),
                        budget,
                    )? {
                        return Err(IndexError::DigestConflict);
                    }
                }
            } else if key.len() == 23
                && key.starts_with(b"member\0")
                && sorted_instances
                    .binary_search_by(|instance| instance.0.as_slice().cmp(&key[7..]))
                    .is_ok()
            {
                return Err(IndexError::DigestConflict);
            }
        }
        if !page.has_more {
            return Ok(());
        }
        let next = page.cursor().last().ok_or(IndexError::ResourceLimited)?;
        if after
            .as_ref()
            .map(|entry| entry.key())
            .transpose()?
            .is_some_and(|prior| next.key().is_ok_and(|key| key <= prior))
        {
            return Err(IndexError::DigestConflict);
        }
        after = Some(next);
    }
}

pub(super) fn verify_preserved_native_v1(
    previous: &HeldPreviousNativeV1,
    candidate: &NativeBootAuthority,
    command: &NativeStateCommandV1,
    joined: &NativeCutoverJoinedSourceFactsV1<'_>,
    retire_repomap: bool,
    budget: &mut WorkBudget,
) -> Result<()> {
    let old_manifest = previous
        .observed_v1()
        .snapshot
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let expected = command.identity.ok_or(IndexError::SnapshotUnavailable)?;
    if old_manifest.protected_source_routes.is_none() && !retire_repomap {
        if expected.protected_source_routes.is_some() {
            return Err(IndexError::DigestConflict.into());
        }
        return Ok(());
    }
    let root = candidate.lease.state_root_identity_v1();
    let observed = verify_native_boot_catalog(root, candidate.io.as_ref(), budget)?;
    let manifest = observed
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?;
    if identity(observed.root_nonce, manifest)? != expected {
        return Err(IndexError::DigestConflict.into());
    }
    let route = manifest.protected_source_routes;
    let objects = root.join("native-objects");
    let io = Arc::new(NativeObjectReadIo::new(Arc::clone(&candidate.physical)));
    let lock = io.open(&objects.join(".object-namespace.lease"), budget)?;
    let reader = FileObjectReader::open_existing(&objects, lock, io, budget)?;
    if observed.namespace != Some(reader.namespace_binding()?) {
        return Err(IndexError::DigestConflict.into());
    }
    if retire_repomap {
        if old_manifest.protected_source_routes.is_none() {
            // A route-less old Native BOOT has no authenticated RepoMap census.
            return Err(IndexError::SnapshotUnavailable.into());
        }
        if joined.imported_legacy_source_count_v1() != 0 {
            return Err(IndexError::DigestConflict.into());
        }
        let candidate_current = if let Some(route) = route {
            let route_object = reader.get(route.object, budget)?;
            with_native_protected_source_routes_v1(
                route.object,
                Some(route.count),
                route_object,
                budget,
                |routes, budget| {
                    quanta_index_catalog::verify_native_source_root_quiescence_with_routes_v1(
                        root,
                        candidate.io.as_ref(),
                        manifest.operation.key.workspace,
                        routes,
                        budget,
                    )
                },
            )?
        } else {
            quanta_index_catalog::verify_native_source_root_quiescence_v1(
                root,
                candidate.io.as_ref(),
                manifest.operation.key.workspace,
                budget,
            )?
        };
        if candidate_current.snapshot.root_nonce != observed.root_nonce
            || candidate_current.snapshot.selected.as_ref() != Some(manifest)
            || candidate_current.snapshot.namespace != observed.namespace
        {
            return Err(IndexError::DigestConflict.into());
        }
        previous.with_current_source_reader_v1(budget, |old_current, old_reader, budget| {
            let _verified_census = verify_source_files_v1(
                old_reader,
                &reader,
                &old_current.source,
                &candidate_current.current_head.source,
                budget,
            )?;
            Ok(())
        })?;
        if let Some(old_route) = old_manifest.protected_source_routes {
            previous.with_protected_routes_v1(budget, |old_routes, old_reader, budget| {
                let policy = manifest
                    .retired_projection_policy
                    .ok_or(IndexError::SnapshotUnavailable)?;
                let policy_object = reader.get(policy.object, budget)?;
                if !policy_object.registration.children.is_empty() {
                    return Err(IndexError::DigestConflict.into());
                }
                let policy_count = preflight_retired_projection_policy_v1(&policy_object.payload)
                    .map_err(|_| IndexError::DigestConflict)?;
                if policy_count > old_routes.repomap_heads.len()
                    || policy_count != policy.identity_count as usize
                {
                    return Err(IndexError::DigestConflict.into());
                }
                let scratch_bytes = policy_count
                    .checked_mul(std::mem::size_of::<quanta_index_core::index::IndexInstanceId>())
                    .ok_or(IndexError::ResourceLimited)?;
                let derived_bytes = old_routes
                    .repomap_heads
                    .len()
                    .checked_mul(std::mem::size_of::<RetiredProjectionIdentityV1>())
                    .and_then(|size| {
                        size.checked_add(old_routes.repomap_heads.len().checked_mul(16)?)
                    })
                    .ok_or(IndexError::ResourceLimited)?;
                let _policy_memory = budget
                    .reserve_memory(
                        u64::try_from(
                            scratch_bytes
                                .checked_add(derived_bytes)
                                .ok_or(IndexError::ResourceLimited)?,
                        )?
                        .max(1),
                        MemoryLifetime::Temporary,
                    )?
                    .ok_or(IndexError::ResourceLimited)?;
                let mut scratch =
                    vec![quanta_index_core::index::IndexInstanceId([0; 16]); policy_count];
                let policy_view = decode_retired_projection_policy_v1(
                    &policy_object.payload,
                    &mut scratch,
                    &mut || budget.checkpoint(),
                )
                .map_err(|error| match error {
                    RetiredProjectionPolicyErrorV1::Admission(error) => error,
                    _ => IndexError::DigestConflict,
                })?;
                let header = policy_view.header_v1();
                if Digest(Sha256::digest(&policy_object.payload).into()) != policy.sha256
                    || header.old_selection_sha256 != previous.selection_sha256_v1()
                    || header.old_root_nonce != Digest(previous.observed_v1().snapshot.root_nonce)
                    || header.old_routes_ref != Some(old_route.object)
                    || header.old_source_head_count != u32::try_from(old_routes.heads.len())?
                    || header.candidate_root_nonce != Digest(observed.root_nonce)
                    || Some(header.candidate_config_sha256) != command.config_digest
                    || header.candidate_profile_ref != manifest.publication.profile
                {
                    return Err(IndexError::DigestConflict.into());
                }
                let mut derived = Vec::new();
                derived
                    .try_reserve_exact(old_routes.repomap_heads.len())
                    .map_err(|_| IndexError::ResourceLimited)?;
                if derived.capacity() > old_routes.repomap_heads.len() {
                    return Err(IndexError::ResourceLimited.into());
                }
                for old in &old_routes.repomap_heads {
                    budget.checkpoint()?;
                    derived.push(old_repomap_identity_v1(old, old_reader, budget)?);
                }
                budget.checkpoint()?;
                derived.sort_unstable_by_key(|identity| identity.projection.0);
                if derived.windows(2).any(|pair| {
                    pair[0].projection == pair[1].projection && pair[0].instance != pair[1].instance
                }) {
                    return Err(IndexError::DigestConflict.into());
                }
                derived.dedup();
                let mut instances = Vec::new();
                instances
                    .try_reserve_exact(derived.len())
                    .map_err(|_| IndexError::ResourceLimited)?;
                if instances.capacity() > derived.len() {
                    return Err(IndexError::ResourceLimited.into());
                }
                instances.extend(derived.iter().map(|identity| identity.instance));
                instances.sort_unstable_by_key(|instance| instance.0);
                if instances.windows(2).any(|pair| pair[0] == pair[1])
                    || derived.len() != policy_count
                {
                    return Err(IndexError::DigestConflict.into());
                }
                for (index, expected) in derived.iter().enumerate() {
                    budget.checkpoint()?;
                    if policy_view.identity_v1(index) != Some(*expected) {
                        return Err(IndexError::DigestConflict.into());
                    }
                }
                let witness = quanta_index_catalog::read_native_protected_snapshot_witness_v1(
                    &previous.selected_config_v1().state_root,
                    previous.catalog_io_v1(),
                    old_manifest,
                    old_routes,
                    budget,
                )?;
                for reference in witness.object_references_v1() {
                    old_reader.verify_object(reference, budget)?;
                }
                if joined.imported_native_source_count_v1() != old_routes.heads.len() {
                    return Err(IndexError::DigestConflict.into());
                }
                if old_routes.heads.is_empty() && route.is_some() {
                    return Err(IndexError::DigestConflict.into());
                }
                let Some(route) = route else {
                    if old_routes.heads.is_empty() {
                        reject_retired_candidate_publications_v1(
                            candidate,
                            &reader,
                            &candidate_current,
                            None,
                            &policy_view,
                            &scratch,
                            budget,
                        )?;
                        return Ok(());
                    }
                    return Err(IndexError::SnapshotUnavailable.into());
                };
                let route_object = reader.get(route.object, budget)?;
                with_native_protected_source_routes_v1(
                    route.object,
                    Some(route.count),
                    route_object,
                    budget,
                    |new_routes, budget| {
                        if new_routes.native_origin.is_some()
                            || !new_routes.repomap_heads.is_empty()
                            || new_routes.heads.len() != old_routes.heads.len()
                        {
                            return Err(IndexError::DigestConflict);
                        }
                        for (old, new) in old_routes.heads.iter().zip(&new_routes.heads) {
                            if old.repo_id != new.repo_id || old.revision_id != new.revision_id {
                                return Err(IndexError::DigestConflict);
                            }
                            let (imported, target, request) = joined
                                .imported_native_source_inputs_for_pair_v1(
                                    &old.repo_id,
                                    &old.revision_id,
                                )
                                .ok_or(IndexError::SnapshotUnavailable)?;
                            let _binding_memory = budget
                                .reserve_memory(16 * 1024, MemoryLifetime::Temporary)?
                                .ok_or(IndexError::ResourceLimited)?;
                            let full = NativeImportedNativeSourceHeadV1 {
                                origin: NativeSourceOriginV1 {
                                    old_root_nonce: imported.origin.old_root_nonce,
                                    old_routes_ref: imported.origin.old_routes_ref,
                                    repo_id: imported.origin.repo_id.clone(),
                                    revision_id: imported.origin.revision_id.clone(),
                                    old_source_head: old.clone(),
                                    file_count: imported.origin.file_count,
                                    file_census_sha256: imported.origin.file_census_sha256,
                                    accepted_transcript_sha256: imported
                                        .origin
                                        .accepted_transcript_sha256,
                                },
                                candidate_root_nonce: imported.candidate_root_nonce,
                                source_operation: new.source_operation,
                                source_publication: new.source_publication.clone(),
                                source_binding: imported.source_binding.clone(),
                                original_snapshot_id: imported.original_snapshot_id,
                                original_request_sha256: imported.original_request_sha256,
                                new_source_manifest_sha256: imported.new_source_manifest_sha256,
                                completed_journal_digest: imported.completed_journal_digest,
                            };
                            if imported.origin.old_root_nonce
                                != Digest(previous.observed_v1().snapshot.root_nonce)
                                || imported.origin.old_routes_ref != old_route.object
                                || imported.candidate_root_nonce != Digest(observed.root_nonce)
                                || imported.source_operation != new.source_operation
                                || !imported.matches_completed_v1(&full, target, request)
                            {
                                return Err(IndexError::DigestConflict);
                            }
                            let (files, census) = verify_source_files_v1(
                                old_reader,
                                &reader,
                                &old.source_publication.source,
                                &new.source_publication.source,
                                budget,
                            )?;
                            if files != imported.origin.file_count
                                || census != imported.origin.file_census_sha256
                            {
                                return Err(IndexError::DigestConflict);
                            }
                        }
                        reject_retired_candidate_publications_v1(
                            candidate,
                            &reader,
                            &candidate_current,
                            Some(new_routes),
                            &policy_view,
                            &scratch,
                            budget,
                        )?;
                        Ok(())
                    },
                )?;
                Ok(())
            })?;
        } else if route.is_some() || joined.imported_native_source_count_v1() != 0 {
            return Err(IndexError::DigestConflict.into());
        }
        drop(reader);
        candidate.physical.sweep_retired()?;
        require_retired(candidate)?;
        return previous.require_current_and_retired_v1(budget);
    }
    let route = route.ok_or(IndexError::SnapshotUnavailable)?;
    let route_object = reader.get(route.object, budget)?;
    with_native_protected_source_routes_v1(
        route.object,
        Some(route.count),
        route_object,
        budget,
        |routes, budget| {
            let origin = routes
                .native_origin
                .as_ref()
                .ok_or(IndexError::SnapshotUnavailable)?;
            let witness = previous
                .with_protected_routes_v1(budget, |old_routes, _old_reader, budget| {
                    if routes.heads != old_routes.heads
                        || routes.repomap_heads != old_routes.repomap_heads
                    {
                        return Err(IndexError::DigestConflict.into());
                    }
                    Ok(
                        quanta_index_catalog::read_native_protected_snapshot_witness_v1(
                            &previous.selected_config_v1().state_root,
                            previous.catalog_io_v1(),
                            old_manifest,
                            old_routes,
                            budget,
                        )?,
                    )
                })
                .map_err(|cause| {
                    cause.downcast::<IndexError>().unwrap_or_else(|cause| {
                        IndexError::Storage(format!("protected Native predecessor: {cause}"))
                    })
                })?;
            if witness.origin_v1() != origin {
                return Err(IndexError::DigestConflict);
            }
            // The predecessor digest fixes the object bodies and edges.
            // Reauthenticate each file without materializing its payload.
            for reference in witness.object_references_v1() {
                reader.verify_object(reference, budget)?;
            }
            quanta_index_catalog::verify_native_protected_snapshot_install_v1(
                root,
                candidate.io.as_ref(),
                manifest,
                routes,
                &witness,
                budget,
            )
        },
    )?;
    drop(reader);
    candidate.physical.sweep_retired()?;
    require_retired(candidate)?;
    previous.require_current_and_retired_v1(budget)
}
