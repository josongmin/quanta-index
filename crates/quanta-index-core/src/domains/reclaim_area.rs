//! The reclaim area of a track root: where a retired generation goes before
//! it is removed, so that removing it is crash-atomic (QI-BB-003 보완 #3,
//! #4).
//!
//! A reclaim first renames the generation directory out of the generation
//! namespace into `<track root>/.reclaim/` — one rename on one filesystem,
//! made durable by syncing both directories it changed — and only then
//! removes it. A crash, or a removal that fails partway, can leave an entry
//! in the reclaim area; it can never leave a half-removed directory in the
//! generation namespace, where a partial tree without its sealed identity
//! would look like an unsealed build and never be reclaimed. The generation
//! namespace holds the whole sealed generation or nothing.
//!
//! [`finish_interrupted_reclaims`] removes what the area holds. Every entry
//! was a sealed generation its owner had retired, fenced and committed to
//! deleting, so removing it is only the rest of that delete.

use std::fs::File;
use std::path::{Path, PathBuf};

use super::generation::{FinishedReclaims, unique_inode_tree_bytes};
use crate::CoreError;

/// The reserved directory under a track root that holds reclaims in
/// progress. Inventories of the track skip it: it is neither a generation
/// family nor a quarantine finding.
pub const RECLAIM_AREA_DIR_NAME: &str = ".reclaim";

/// The reclaim area of `track_root`.
#[must_use]
pub fn reclaim_area(track_root: &Path) -> PathBuf {
    track_root.join(RECLAIM_AREA_DIR_NAME)
}

fn storage(action: &str, path: &Path, error: &std::io::Error) -> CoreError {
    CoreError::Storage(format!("reclaim: {action} {}: {error}", path.display()))
}

/// Make the entries of `directory` durable.
fn sync_directory(directory: &Path) -> Result<(), CoreError> {
    File::open(directory)
        .and_then(|handle| handle.sync_all())
        .map_err(|error| storage("sync directory", directory, &error))
}

/// Remove `generation_dir` crash-atomically: move it into the reclaim area
/// of `track_root` as `entry_name`, durably, then remove it.
///
/// `entry_name` is unique per generation (see
/// [`super::generation::GenerationStorageKeyV1::reclaim_entry_name`]); an entry already
/// under that name is the leftover of an earlier interrupted reclaim of the
/// same generation and is removed first. A removal that fails leaves the
/// entry in the area, where [`finish_interrupted_reclaims`] finds it; the
/// generation is already out of its namespace, so nothing can list, open or
/// serve it.
pub fn reclaim_directory(
    track_root: &Path,
    generation_dir: &Path,
    entry_name: &str,
) -> Result<(), CoreError> {
    reclaim_directory_removing_with(track_root, generation_dir, entry_name, |entry| {
        std::fs::remove_dir_all(entry)
    })
}

/// [`reclaim_directory`] with the final removal supplied, so a test can
/// fail it after the move.
fn reclaim_directory_removing_with(
    track_root: &Path,
    generation_dir: &Path,
    entry_name: &str,
    remove: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), CoreError> {
    let area = reclaim_area(track_root);
    if !area.is_dir() {
        std::fs::create_dir_all(&area)
            .map_err(|error| storage("create reclaim area", &area, &error))?;
        sync_directory(track_root)?;
    }
    let entry = area.join(entry_name);
    if entry.exists() {
        std::fs::remove_dir_all(&entry)
            .map_err(|error| storage("remove earlier interrupted reclaim", &entry, &error))?;
    }
    std::fs::rename(generation_dir, &entry).map_err(|error| {
        storage("move generation into the reclaim area", generation_dir, &error)
    })?;
    if let Some(parent) = generation_dir.parent() {
        sync_directory(parent)?;
    }
    sync_directory(&area)?;
    remove(&entry).map_err(|error| {
        storage(
            "remove reclaimed generation (left in the reclaim area for the next pass)",
            &entry,
            &error,
        )
    })?;
    sync_directory(&area)
}

/// Remove every entry the reclaim area of `track_root` holds: the rest of
/// reclaims a crash or a failed removal interrupted. Idempotent; an absent
/// area has nothing to finish.
pub fn finish_interrupted_reclaims(track_root: &Path) -> Result<FinishedReclaims, CoreError> {
    let area = reclaim_area(track_root);
    if !area.exists() {
        return Ok(FinishedReclaims::default());
    }
    let mut finished = FinishedReclaims::default();
    for entry in std::fs::read_dir(&area).map_err(|error| storage("list", &area, &error))? {
        let entry = entry.map_err(|error| storage("read entry of", &area, &error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| storage("inspect", &path, &error))?;
        if file_type.is_dir() {
            let bytes = unique_inode_tree_bytes(std::slice::from_ref(&path), &|_name| false)
                .map_err(|error| storage("measure", &path, &error))?;
            std::fs::remove_dir_all(&path).map_err(|error| storage("remove", &path, &error))?;
            finished.bytes = finished.bytes.saturating_add(bytes);
        } else {
            let bytes = entry
                .metadata()
                .map_err(|error| storage("inspect", &path, &error))?
                .len();
            std::fs::remove_file(&path).map_err(|error| storage("remove", &path, &error))?;
            finished.bytes = finished.bytes.saturating_add(bytes);
        }
        finished.entries = finished.entries.saturating_add(1);
    }
    if finished.entries > 0 {
        sync_directory(&area)?;
    }
    Ok(finished)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;

    use super::{
        FinishedReclaims, finish_interrupted_reclaims, reclaim_area, reclaim_directory,
        reclaim_directory_removing_with,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn generation(
        root: &Path,
        family: &str,
        name: &str,
        bytes: &[u8],
    ) -> io::Result<std::path::PathBuf> {
        let dir = root.join(family).join(name);
        std::fs::create_dir_all(dir.join("payload"))?;
        std::fs::write(dir.join("sealed-identity"), b"id")?;
        std::fs::write(dir.join("payload").join("data"), bytes)?;
        Ok(dir)
    }

    /// A reclaim leaves the generation namespace without the directory and
    /// the reclaim area empty.
    #[test]
    fn a_reclaim_removes_the_generation_and_leaves_nothing_behind() -> TestResult {
        let root = tempfile::tempdir()?;
        let dir = generation(root.path(), "family", "g3", b"0123456789")?;
        reclaim_directory(root.path(), &dir, "family.g3")?;
        if dir.exists()
            || std::fs::read_dir(reclaim_area(root.path()))?
                .next()
                .is_some()
        {
            return Err("the generation is gone and the reclaim area is empty".into());
        }
        Ok(())
    }

    /// A removal that fails after the move leaves the whole tree in the
    /// reclaim area and nothing in the generation namespace; finishing
    /// removes it, reports its bytes, and a second finish has nothing.
    #[test]
    fn an_interrupted_reclaim_is_out_of_the_namespace_and_finished_later() -> TestResult {
        let root = tempfile::tempdir()?;
        let dir = generation(root.path(), "family", "g3", b"0123456789")?;
        let interrupted =
            reclaim_directory_removing_with(root.path(), &dir, "family.g3", |_path| {
                Err(io::Error::other("injected removal failure"))
            });
        if interrupted.is_ok() || dir.exists() {
            return Err(
                "the failed removal is an error and the namespace no longer holds g3".into()
            );
        }
        let left = reclaim_area(root.path()).join("family.g3");
        if !left.join("payload").join("data").is_file() {
            return Err("the interrupted reclaim leaves the whole tree in the area".into());
        }
        let finished = finish_interrupted_reclaims(root.path())?;
        if finished
            != (FinishedReclaims {
                entries: 1,
                bytes: 12,
            })
            || left.exists()
        {
            return Err(
                format!("finishing removes the entry and its 12 bytes: {finished:?}").into()
            );
        }
        if finish_interrupted_reclaims(root.path())? != FinishedReclaims::default() {
            return Err("a second finish has nothing to do".into());
        }
        Ok(())
    }

    /// A leftover under the same entry name — an earlier interrupted
    /// reclaim of the same generation — is replaced, not a refusal.
    #[test]
    fn a_leftover_of_the_same_generation_is_replaced() -> TestResult {
        let root = tempfile::tempdir()?;
        let leftover = reclaim_area(root.path()).join("family.g3");
        std::fs::create_dir_all(&leftover)?;
        std::fs::write(leftover.join("stale"), b"x")?;
        let dir = generation(root.path(), "family", "g3", b"abc")?;
        reclaim_directory(root.path(), &dir, "family.g3")?;
        if dir.exists() || leftover.exists() {
            return Err("the generation and the stale leftover are both gone".into());
        }
        Ok(())
    }

    /// Finishing a track that never reclaimed anything has nothing to do.
    #[test]
    fn an_absent_reclaim_area_has_nothing_to_finish() -> TestResult {
        let root = tempfile::tempdir()?;
        if finish_interrupted_reclaims(root.path())? != FinishedReclaims::default() {
            return Err("no area, nothing finished".into());
        }
        Ok(())
    }
}
