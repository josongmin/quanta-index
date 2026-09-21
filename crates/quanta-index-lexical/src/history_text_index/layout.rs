//! Where an epoch index lives on disk.
//!
//! ```text
//! {root}/generation-v1-{sha256(repo, revision)}/g{generation}/e{epoch}/
//!     history-text-manifest.cbor
//!     commits/   one inverted index over commit messages
//!     diffs/     one inverted index over diff-hunk text
//! ```
//!
//! An epoch directory is immutable once it exists: it is built under the
//! sibling name `e{epoch}.staging` and renamed into place after its
//! manifest is durable, so a directory named `e{epoch}` is a complete
//! index or a corruption, never a half-built one.

use std::path::{Path, PathBuf};

use quanta_index_contract::{AuxEpochV1, ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, CoreError, GenerationStorageKeyV1, HistoryTextKindV1,
};

const EPOCH_DIR_PREFIX: &str = "e";
const STAGING_DIR_SUFFIX: &str = ".staging";
const COMMITS_DIR_NAME: &str = "commits";
const DIFFS_DIR_NAME: &str = "diffs";

/// The directory holding every epoch index of one generation.
#[must_use]
pub(super) fn generation_dir(root: &Path, generation: &AuxiliaryGenerationKeyV1) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(&generation.repo_id, &generation.revision_id)
        .generation_dir(root, generation.generation)
}

/// The directory of one published epoch index.
#[must_use]
pub(super) fn epoch_dir(
    root: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
) -> PathBuf {
    generation_dir(root, generation).join(format!("{EPOCH_DIR_PREFIX}{}", epoch.get()))
}

/// Where one epoch index is built before it is renamed into place.
#[must_use]
pub(super) fn staging_dir(
    root: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
) -> PathBuf {
    generation_dir(root, generation).join(format!(
        "{EPOCH_DIR_PREFIX}{}{STAGING_DIR_SUFFIX}",
        epoch.get()
    ))
}

/// The sub-directory of one kind's index inside an epoch directory.
#[must_use]
pub(super) fn kind_dir(epoch_dir: &Path, kind: HistoryTextKindV1) -> PathBuf {
    epoch_dir.join(match kind {
        HistoryTextKindV1::Commit => COMMITS_DIR_NAME,
        HistoryTextKindV1::Diff => DIFFS_DIR_NAME,
    })
}

/// The epoch a published epoch directory name denotes, if it is one.
///
/// Staging leftovers (`e{n}.staging`) are not published epochs; any other
/// name inside a generation directory is foreign and is reported as
/// such rather than skipped.
fn parse_epoch_dir_name(name: &str) -> Result<Option<AuxEpochV1>, CoreError> {
    if name.ends_with(STAGING_DIR_SUFFIX) {
        return Ok(None);
    }
    let digits = name
        .strip_prefix(EPOCH_DIR_PREFIX)
        .ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexCorrupt,
            message: format!(
                "history text index: foreign entry `{name}` in a generation directory"
            ),
        })?;
    let epoch = digits.parse::<u64>().map_err(|err| CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexCorrupt,
        message: format!("history text index: entry `{name}` is not an epoch directory: {err}"),
    })?;
    Ok(Some(AuxEpochV1::new(epoch)))
}

/// Every generation of the pair with a directory, ascending; none when the
/// pair has no directory. Any other entry is foreign and is reported as
/// such rather than skipped.
pub(super) fn list_generations(
    root: &Path,
    repo_id: &RepoId,
    revision_id: &RevisionId,
) -> Result<Vec<ManifestGeneration>, CoreError> {
    let dir = root.join(GenerationStorageKeyV1::for_repo_revision(repo_id, revision_id).as_str());
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&dir).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: list pair directory {}: {err}",
            dir.display()
        ))
    })?;
    let mut generations = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            CoreError::Storage(format!(
                "history text index: read pair directory entry {}: {err}",
                dir.display()
            ))
        })?;
        let name = entry.file_name();
        let generation = name
            .to_str()
            .and_then(GenerationStorageKeyV1::generation_of_dir_name)
            .ok_or_else(|| CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexCorrupt,
                message: format!(
                    "history text index: foreign entry {} in the pair directory {}",
                    name.to_string_lossy(),
                    dir.display()
                ),
            })?;
        generations.push(generation);
    }
    generations.sort_unstable();
    Ok(generations)
}

/// Every published epoch of one generation, ascending; none when the
/// generation has no directory.
pub(super) fn list_epochs(
    root: &Path,
    generation: &AuxiliaryGenerationKeyV1,
) -> Result<Vec<AuxEpochV1>, CoreError> {
    let dir = generation_dir(root, generation);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&dir).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: list generation directory {}: {err}",
            dir.display()
        ))
    })?;
    let mut epochs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            CoreError::Storage(format!(
                "history text index: read generation directory entry {}: {err}",
                dir.display()
            ))
        })?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::HistoryTextIndexCorrupt,
            message: format!(
                "history text index: entry with a non-UTF-8 name in {}",
                dir.display()
            ),
        })?;
        if let Some(epoch) = parse_epoch_dir_name(name)? {
            epochs.push(epoch);
        }
    }
    epochs.sort_unstable();
    Ok(epochs)
}

/// Recursive byte size of a directory tree (what a discard reclaims).
pub(super) fn tree_bytes(path: &Path) -> Result<u64, CoreError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: inspect {}: {err}",
            path.display()
        ))
    })?;
    if !metadata.is_dir() {
        return Ok(metadata.len());
    }
    let mut total = 0_u64;
    let entries = std::fs::read_dir(path).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: list {}: {err}",
            path.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            CoreError::Storage(format!(
                "history text index: read entry of {}: {err}",
                path.display()
            ))
        })?;
        total = total.saturating_add(tree_bytes(&entry.path())?);
    }
    Ok(total)
}

/// Make the directory entry `path` durable in its parent.
pub(super) fn fsync_parent(path: &Path) -> Result<(), CoreError> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "history text index: {} has no parent directory",
            path.display()
        ))
    })?;
    let dir = std::fs::File::open(parent).map_err(|err| {
        CoreError::Storage(format!(
            "history text index: open directory {}: {err}",
            parent.display()
        ))
    })?;
    dir.sync_all().map_err(|err| {
        CoreError::Storage(format!(
            "history text index: fsync directory {}: {err}",
            parent.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::AuxEpochV1;

    use super::parse_epoch_dir_name;

    #[test]
    fn epoch_directory_names_parse_and_foreign_names_are_refused() {
        assert_eq!(
            parse_epoch_dir_name("e7").expect("parses"),
            Some(AuxEpochV1::new(7))
        );
        assert_eq!(parse_epoch_dir_name("e7.staging").expect("parses"), None);
        assert!(parse_epoch_dir_name("g7").is_err());
        assert!(parse_epoch_dir_name("e").is_err());
        assert!(parse_epoch_dir_name("e-1").is_err());
    }
}
