//! The boot inventory of the adapter's generations and the quarantine discard.

#![expect(
    clippy::redundant_pub_crate,
    reason = "the module is private to the crate; `pub(crate)` is the visibility its items need across the crate's modules, and the workspace's `unreachable_pub = deny` forbids the bare `pub`"
)]

use crate::generation_dir::generation_tree_bytes;
use crate::index_store::{lexical_sealed_identity_path, read_lexical_sealed_identity};
use crate::sealed_generation::quarantined_by_scrub;
use quanta_index_contract::{GenerationSnapshot, SearchPlaneTrackKind};
use quanta_index_core::domains::generation::{
    GenerationQuarantineReasonV1, GenerationStorageKeyV1, InventoriedSealedGenerationV1,
    QuarantinedGenerationV1, SealedGenerationInventoryV1,
};
use quanta_index_core::{
    CoreError, QUARANTINE_TARGET_NOT_QUARANTINED_CODE, QuarantineDiscardOutcomeV1,
    RECLAIM_AREA_DIR_NAME,
};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Inventory the sealed generations under a lexical state root (QI-BB-026).
///
/// Reads each generation's sealed identity and nothing else: no sidecar is
/// hashed and no index is opened, so the cost is one small file per sealed
/// generation. A directory that is not a canonical family or `g<N>`, an
/// identity that cannot be read or decoded, and an identity that does not
/// own its directory are each reported as quarantined with the path and the
/// reason, and boot continues without them. Generations without a sealed
/// identity are in-progress builds and are skipped silently, as before. Only
/// a directory listing that fails is an error.
pub fn inventory_sealed_generations(
    lexical_root: &Path,
) -> Result<SealedGenerationInventoryV1, CoreError> {
    let mut inventory = SealedGenerationInventoryV1::default();
    if !lexical_root.exists() {
        return Ok(inventory);
    }
    for family_entry in std::fs::read_dir(lexical_root).map_err(|error| {
        CoreError::Storage(format!("lexical: list {}: {error}", lexical_root.display()))
    })? {
        let family_entry = family_entry.map_err(|error| {
            CoreError::Storage(format!("lexical: read generation-family entry: {error}"))
        })?;
        if !family_entry
            .file_type()
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: inspect {}: {error}",
                    family_entry.path().display()
                ))
            })?
            .is_dir()
        {
            continue;
        }
        let family_name = family_entry.file_name();
        // Reclaims in progress, finished by the reclaim port (QI-BB-003).
        if family_name == RECLAIM_AREA_DIR_NAME {
            continue;
        }
        if !family_name
            .to_str()
            .is_some_and(GenerationStorageKeyV1::is_canonical_name)
        {
            inventory.quarantined.push(quarantine(
                family_entry.path(),
                GenerationQuarantineReasonV1::NonCanonicalLayout,
                "directory is not a canonical generation family; it needs explicit migration"
                    .to_string(),
            ));
            continue;
        }
        for generation_entry in std::fs::read_dir(family_entry.path()).map_err(|error| {
            CoreError::Storage(format!(
                "lexical: list {}: {error}",
                family_entry.path().display()
            ))
        })? {
            let generation_entry = generation_entry.map_err(|error| {
                CoreError::Storage(format!("lexical: read generation entry: {error}"))
            })?;
            if !generation_entry
                .file_type()
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "lexical: inspect {}: {error}",
                        generation_entry.path().display()
                    ))
                })?
                .is_dir()
            {
                continue;
            }
            let generation_dir = generation_entry.path();
            match inventory_generation_dir(lexical_root, &generation_dir) {
                // A generation the scrub proved corrupt is quarantined by its
                // receipt, not served (QI-BB-017, QI-BB-026).
                Ok(Some(identity)) => match quarantined_by_scrub(&generation_dir)? {
                    Some(quarantined) => inventory.quarantined.push(quarantined),
                    None => inventory.sealed.push(InventoriedSealedGenerationV1 {
                        identity,
                        path: generation_dir,
                    }),
                },
                Ok(None) => {}
                Err(quarantined) => inventory.quarantined.push(quarantined),
            }
        }
    }
    Ok(inventory)
}

/// Remove `entry.path` if `quarantined_now` — the track's inventory taken
/// this instant — names it under the same reason (QI-BB-026).
///
/// The inventory built every quarantined path from a directory walk under
/// `track_root`, so a path it names is under the root by construction; the
/// containment check below is the belt to that brace. Anything the
/// inventory does not name now is refused typed, never removed: a
/// directory repaired or sealed since the caller listed it stays.
pub(crate) fn discard_quarantined_directory(
    track_root: &Path,
    quarantined_now: &[QuarantinedGenerationV1],
    entry: &QuarantinedGenerationV1,
) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
    let not_quarantined = |why: String| CoreError::Typed {
        code: QUARANTINE_TARGET_NOT_QUARANTINED_CODE.to_string(),
        message: format!(
            "lexical: refusing to discard {}: {why}",
            entry.path.display()
        ),
    };
    let Some(current) = quarantined_now
        .iter()
        .find(|quarantined| quarantined.path == entry.path)
    else {
        if std::fs::symlink_metadata(&entry.path).is_ok() {
            return Err(not_quarantined(
                "the path is not quarantined now; a sealed, in-progress or repaired directory is not this port's to remove"
                    .to_string(),
            ));
        }
        return Ok(QuarantineDiscardOutcomeV1::Absent);
    };
    if current.reason != entry.reason {
        return Err(not_quarantined(format!(
            "it is quarantined as {} now, not {} as listed; list again",
            current.reason.as_code_str(),
            entry.reason.as_code_str()
        )));
    }
    if !entry.path.starts_with(track_root) {
        return Err(not_quarantined(format!(
            "the path is outside the track root {}",
            track_root.display()
        )));
    }
    let metadata = std::fs::symlink_metadata(&entry.path).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: inspect quarantined {}: {error}",
            entry.path.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Err(not_quarantined(
            "the path is not a directory; the inventory quarantines directories only".to_string(),
        ));
    }
    let bytes = generation_tree_bytes(&entry.path)?;
    std::fs::remove_dir_all(&entry.path).map_err(|error| {
        CoreError::Storage(format!(
            "lexical: discard quarantined {}: {error}",
            entry.path.display()
        ))
    })?;
    if let Some(parent) = entry.path.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: fsync {} after discarding quarantine: {error}",
                    parent.display()
                ))
            })?;
    }
    Ok(QuarantineDiscardOutcomeV1::Discarded { bytes })
}

/// One generation directory's inventory outcome: `Ok(Some)` for a sealed
/// identity that owns the directory, `Ok(None)` for an in-progress build,
/// `Err` for a quarantine.
pub(crate) fn inventory_generation_dir(
    lexical_root: &Path,
    generation_dir: &Path,
) -> Result<Option<GenerationSnapshot>, QuarantinedGenerationV1> {
    let Some(generation_name) = generation_dir.file_name().and_then(|name| name.to_str()) else {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory name is not UTF-8".to_string(),
        ));
    };
    if GenerationStorageKeyV1::generation_of_dir_name(generation_name).is_none() {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::NonCanonicalLayout,
            "generation directory is not `g<N>`; it needs explicit migration".to_string(),
        ));
    }
    if !lexical_sealed_identity_path(generation_dir).exists() {
        return Ok(None);
    }
    let identity = read_lexical_sealed_identity(generation_dir).map_err(|error| {
        quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::IdentityUnreadable,
            error.to_string(),
        )
    })?;
    if identity.track != SearchPlaneTrackKind::Lexical
        || GenerationStorageKeyV1::for_repo_revision(&identity.repo_id, &identity.revision_id)
            .generation_dir(lexical_root, identity.manifest_generation)
            != generation_dir
    {
        return Err(quarantine(
            generation_dir.to_path_buf(),
            GenerationQuarantineReasonV1::ScopeMismatch,
            format!(
                "sealed identity names {:?} repo={} revision={} generation={}, which does not own this directory",
                identity.track,
                identity.repo_id.as_str(),
                identity.revision_id.as_str(),
                identity.manifest_generation.get()
            ),
        ));
    }
    Ok(Some(identity))
}

pub(crate) fn quarantine(
    path: PathBuf,
    reason: GenerationQuarantineReasonV1,
    detail: String,
) -> QuarantinedGenerationV1 {
    QuarantinedGenerationV1 {
        track: SearchPlaneTrackKind::Lexical,
        path,
        reason,
        detail,
    }
}
