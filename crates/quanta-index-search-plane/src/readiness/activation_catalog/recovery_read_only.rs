//! Existing-only activation DATA under caller-supplied real read custody.
//! This does not verify sealed physical content or authorize a deployment.

use super::{
    ROOT_INCARNATION_BYTES_V1, ROOT_INCARNATION_FILE_V1, active_search_corpus_head_v1,
    is_search_corpus_root_path, repository_envelope,
};
pub use crate::readiness::recovery_read_io::RecoveryReadFailureV1 as ActivationReadFailureV1;
use crate::readiness::recovery_read_io::{directory, read_existing};
use quanta_index_contract::{RepoId, RevisionId, SearchCorpusActiveHeadV1};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectReadIoPort, WorkBudget,
};
use quanta_index_core::{ExistingTreeEntryKindV1, ExistingTreeMetadataIoPortV1};
use std::{ffi::{OsStr, OsString}, path::Path};

/// Observed DATA with the same Original's retained allocation. Neither this
/// value nor a root lease substitutes for sealed-content/receipt verification.
pub struct ReadOnlyActivationObservationV1 {
    head: Option<SearchCorpusActiveHeadV1>,
    _memory: MemoryLease,
}

impl ReadOnlyActivationObservationV1 {
    #[must_use]
    pub fn head_v1(&self) -> Option<&SearchCorpusActiveHeadV1> {
        self.head.as_ref()
    }
}

/// The caller holds the actual existing StateRootLease throughout inspection
/// and its later transaction. `io` and `metadata` must carry the real Legacy
/// read authority admitted from the same Original; a lease alone cannot
/// construct either port.
/// This method does not instantiate ActivationCatalog or clean/create state.
pub fn inspect_existing_activation_repository_v1(
    activations_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    io: &dyn ObjectReadIoPort,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    budget: &mut WorkBudget,
) -> Result<ReadOnlyActivationObservationV1, ActivationReadFailureV1> {
    budget.checkpoint()?;
    let path_bytes = u64::try_from(activations_root.as_os_str().len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(4)
        .and_then(|n| n.checked_add(8192))
        .ok_or(IndexError::ResourceLimited)?;
    let _paths = budget
        .reserve_memory(path_bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let before_directory = directory(activations_root, budget)?;
    let incarnation_path = activations_root.join(ROOT_INCARNATION_FILE_V1);
    let incarnation = read_existing(
        &incarnation_path,
        before_directory.owner,
        u64::try_from(ROOT_INCARNATION_BYTES_V1).map_err(|_error| IndexError::ResourceLimited)?,
        io,
        budget,
    )?;
    let root_incarnation: [u8; ROOT_INCARNATION_BYTES_V1] = incarnation
        .bytes_v1()
        .try_into()
        .map_err(|_error| IndexError::DigestConflict)?;
    if root_incarnation.iter().all(|byte| *byte == 0) {
        return Err(IndexError::DigestConflict.into());
    }
    drop(incarnation);
    // ActivationCatalog restores every repository envelope and every active
    // revision at startup. A one-pair projection cannot prove that replacing
    // this entire Legacy root preserves the other query-visible heads.
    let mut saw_selected = false;
    let mut head = None;
    let mut retained = None;
    metadata.visit_directory_v1(
        activations_root,
        budget,
        &mut |name: &OsStr, kind, _bytes, budget| {
            let basename = Path::new(name);
            if !is_search_corpus_root_path(basename) {
                if kind == ExistingTreeEntryKindV1::File
                    && basename.extension() == Some(OsStr::new("json"))
                {
                    return Err(IndexError::DigestConflict.into());
                }
                return Ok(());
            }
            if kind != ExistingTreeEntryKindV1::File {
                return Err(IndexError::DigestConflict.into());
            }
            let path = activations_root.join(name);
            let envelope = read_existing(
                &path,
                before_directory.owner,
                u64::try_from(repository_envelope::MAX_ENVELOPE_BYTES)
                    .map_err(|_| IndexError::ResourceLimited)?,
                io,
                budget,
            )?;
            let (observed_repo, observed) =
                repository_envelope::decode(&path, envelope.bytes_v1())?;
            require_selected_active_scope_v1(observed.roots.keys(), repo, revision)?;
            if &observed_repo == repo {
                if saw_selected {
                    return Err(IndexError::DigestConflict.into());
                }
                // The decoder's temporary grant covers the private projection.
                // Admit the returned DATA before cloning its contract strings.
                let retained_bytes = u64::try_from(envelope.bytes_v1().len())
                    .map_err(|_| IndexError::ResourceLimited)?
                    .checked_mul(4)
                    .and_then(|n| n.checked_add(4096))
                    .ok_or(IndexError::ResourceLimited)?;
                retained = Some(
                    budget
                        .reserve_memory(retained_bytes, MemoryLifetime::Retained)?
                        .ok_or(IndexError::ResourceLimited)?,
                );
                head = active_search_corpus_head_v1(&observed.roots, repo, revision)
                    .map(|selected| selected.to_contract_v1(root_incarnation))
                    .transpose()?;
                saw_selected = true;
            }
            Ok(())
        },
    )?;
    if !saw_selected || directory(activations_root, budget)? != before_directory {
        return Err(IndexError::DigestConflict.into());
    }
    budget.checkpoint()?;
    Ok(ReadOnlyActivationObservationV1 {
        head,
        _memory: retained.ok_or(IndexError::DigestConflict)?,
    })
}

/// Lend every query-visible head in deterministic physical-envelope and
/// revision order. The caller retains the real StateRootLease and the same
/// admitted read ports through the complete scan. Observations remain
/// provisional until the final directory recheck succeeds; this reader does
/// not verify sealed generation content or issue a Source receipt.
pub fn visit_existing_active_search_corpus_heads_v1(
    activations_root: &Path,
    io: &dyn ObjectReadIoPort,
    metadata: &dyn ExistingTreeMetadataIoPortV1,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(
        &RepoId,
        &RevisionId,
        &SearchCorpusActiveHeadV1,
        &mut WorkBudget,
    ) -> Result<(), ActivationReadFailureV1>,
) -> Result<u64, ActivationReadFailureV1> {
    budget.checkpoint()?;
    let path_bytes = u64::try_from(activations_root.as_os_str().len())
        .map_err(|_| IndexError::ResourceLimited)?
        .checked_mul(4)
        .and_then(|n| n.checked_add(8192))
        .ok_or(IndexError::ResourceLimited)?;
    let _paths = budget
        .reserve_memory(path_bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let before_directory = directory(activations_root, budget)?;
    let incarnation_path = activations_root.join(ROOT_INCARNATION_FILE_V1);
    let incarnation = read_existing(
        &incarnation_path,
        before_directory.owner,
        u64::try_from(ROOT_INCARNATION_BYTES_V1).map_err(|_| IndexError::ResourceLimited)?,
        io,
        budget,
    )?;
    let root_incarnation: [u8; ROOT_INCARNATION_BYTES_V1] = incarnation
        .bytes_v1()
        .try_into()
        .map_err(|_| IndexError::DigestConflict)?;
    if root_incarnation.iter().all(|byte| *byte == 0) {
        return Err(IndexError::DigestConflict.into());
    }
    drop(incarnation);

    // The metadata port lends names in filesystem order. Fund copies and sort
    // before opening any envelope, so a filesystem reorder cannot change the
    // census order or hide an uninspected entry.
    let mut names: Vec<(OsString, MemoryLease)> = Vec::new();
    metadata.visit_directory_v1(
        activations_root,
        budget,
        &mut |name, kind, _bytes, budget| {
            let basename = Path::new(name);
            if !is_search_corpus_root_path(basename) {
                if kind == ExistingTreeEntryKindV1::File
                    && basename.extension() == Some(OsStr::new("json"))
                {
                    return Err(IndexError::DigestConflict.into());
                }
                return Ok(());
            }
            if kind != ExistingTreeEntryKindV1::File {
                return Err(IndexError::DigestConflict.into());
            }
            let width = std::mem::size_of::<(OsString, MemoryLease)>()
                .checked_mul(4)
                .and_then(|n| n.checked_add(name.len().checked_mul(4)?))
                .ok_or(IndexError::ResourceLimited)?;
            let allocation = budget
                .reserve_memory(
                    u64::try_from(width).map_err(|_| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?
                .ok_or(IndexError::ResourceLimited)?;
            names.try_reserve(1).map_err(|_| IndexError::ResourceLimited)?;
            names.push((name.to_owned(), allocation));
            Ok(())
        },
    )?;
    names.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    if names.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(IndexError::DigestConflict.into());
    }

    let mut count = 0_u64;
    for (name, _allocation) in &names {
        budget.checkpoint()?;
        let path = activations_root.join(name);
        let envelope = read_existing(
            &path,
            before_directory.owner,
            u64::try_from(repository_envelope::MAX_ENVELOPE_BYTES)
                .map_err(|_| IndexError::ResourceLimited)?,
            io,
            budget,
        )?;
        let (repo, observed) = repository_envelope::decode(&path, envelope.bytes_v1())?;
        let projection_bytes = u64::try_from(envelope.bytes_v1().len())
            .map_err(|_| IndexError::ResourceLimited)?
            .checked_mul(4)
            .and_then(|n| n.checked_add(4096))
            .ok_or(IndexError::ResourceLimited)?;
        let _projection = budget
            .reserve_memory(projection_bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        for (key, active) in &observed.roots {
            if key.repo_id != repo {
                return Err(IndexError::DigestConflict.into());
            }
            let head = active.to_contract_v1(root_incarnation)?;
            visit(&repo, &key.revision_id, &head, budget)?;
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
    }
    if directory(activations_root, budget)? != before_directory {
        return Err(IndexError::DigestConflict.into());
    }
    budget.checkpoint()?;
    Ok(count)
}

fn require_selected_active_scope_v1<'a>(
    mut roots: impl Iterator<Item = &'a super::ActivationKey>,
    repo: &RepoId,
    revision: &RevisionId,
) -> Result<(), IndexError> {
    if roots.any(|key| &key.repo_id != repo || &key.revision_id != revision) {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readiness::activation_catalog::ActivationKey;

    #[test]
    fn whole_root_active_scope_rejects_sibling_and_foreign_heads()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = RepoId::new("selected-repo")?;
        let revision = RevisionId::new("selected-revision")?;
        let selected = ActivationKey::for_pair(&repo, &revision);
        let sibling = ActivationKey::for_pair(&repo, &RevisionId::new("sibling-revision")?);
        let foreign = ActivationKey::for_pair(&RepoId::new("foreign-repo")?, &revision);
        assert!(
            require_selected_active_scope_v1([&selected].into_iter(), &repo, &revision).is_ok()
        );
        assert!(
            require_selected_active_scope_v1([&selected, &sibling].into_iter(), &repo, &revision)
                .is_err()
        );
        assert!(
            require_selected_active_scope_v1([&foreign].into_iter(), &repo, &revision).is_err()
        );
        // An envelope with only historical receipts has no query-visible root.
        assert!(require_selected_active_scope_v1(std::iter::empty(), &repo, &revision).is_ok());
        Ok(())
    }
}
