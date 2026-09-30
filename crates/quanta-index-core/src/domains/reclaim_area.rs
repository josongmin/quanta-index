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
//! was already admitted for deletion by its owner (retired, incomplete, or
//! quarantined) and moved out of the serving namespace, so removing it is
//! only the rest of that delete.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, Stat, fstat, mkdirat, open, openat, renameat, statat,
    unlinkat,
};
use sha2::{Digest as _, Sha256};

use super::generation::FinishedReclaims;
use crate::CoreError;

/// The reserved directory under a track root that holds admitted deletions
/// in progress. Inventories skip it: it is neither a generation family nor
/// a quarantine finding.
pub const RECLAIM_AREA_DIR_NAME: &str = ".reclaim";

/// The reclaim area of `track_root`.
#[must_use]
pub fn reclaim_area(track_root: &Path) -> PathBuf {
    track_root.join(RECLAIM_AREA_DIR_NAME)
}

fn storage(action: &str, path: &Path, error: &std::io::Error) -> CoreError {
    CoreError::Storage(format!("reclaim: {action} {}: {error}", path.display()))
}

fn io_error(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::CLOEXEC)
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW);
const REMOVE_BATCH_ENTRIES: usize = 128;

fn open_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    openat(parent, Path::new(name), DIRECTORY_FLAGS, Mode::empty())
        .map(File::from)
        .map_err(io_error)
}

fn entry_names_at_limit(directory: &File, limit: usize) -> io::Result<Vec<OsString>> {
    let mut entries = Dir::read_from(directory).map_err(io_error)?;
    let mut names = Vec::new();
    while names.len() < limit {
        let Some(name) = next_entry_name(&mut entries)? else {
            break;
        };
        names.push(name);
    }
    Ok(names)
}

fn next_entry_name(entries: &mut Dir) -> io::Result<Option<OsString>> {
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(io_error)?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            return Ok(Some(OsString::from_vec(bytes.to_vec())));
        }
    }
    Ok(None)
}

struct DirectoryFrame {
    components: Vec<OsString>,
    identity: (u64, u64),
}

fn inode_key(metadata: &Stat) -> io::Result<(u64, u64)> {
    let device = u64::try_from(metadata.st_dev)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok((device, metadata.st_ino))
}

fn open_frame_directory(parent: &File, frame: &DirectoryFrame) -> io::Result<File> {
    let mut opened = parent.try_clone()?;
    for component in &frame.components {
        opened = open_directory_at(&opened, component)?;
    }
    if inode_key(&fstat(&opened).map_err(io_error)?)? != frame.identity {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "reclaim directory changed during traversal",
        ));
    }
    Ok(opened)
}

struct RemoveFrame {
    directory: DirectoryFrame,
    pending: Vec<OsString>,
}

impl RemoveFrame {
    fn new(directory: DirectoryFrame) -> Self {
        Self {
            directory,
            pending: Vec::new(),
        }
    }
}

/// Remove only entries reachable through a pinned parent directory. Neither
/// a symlink at the target nor one nested in its tree is followed.
fn remove_tree_at(parent: &File, name: &OsStr) -> io::Result<()> {
    let path = Path::new(name);
    let metadata = match statat(parent, path, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => metadata,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    if !FileType::from_raw_mode(metadata.st_mode).is_dir() {
        return unlinkat(parent, path, AtFlags::empty()).map_err(io_error);
    }
    let mut pending = vec![RemoveFrame::new(DirectoryFrame {
        components: vec![name.to_os_string()],
        identity: inode_key(&metadata)?,
    })];
    while let Some(frame) = pending.last_mut() {
        let directory = open_frame_directory(parent, &frame.directory)?;
        let mut descend = None;
        loop {
            if frame.pending.is_empty() {
                frame.pending = entry_names_at_limit(&directory, REMOVE_BATCH_ENTRIES)?;
            }
            let Some(child) = frame.pending.pop() else {
                break;
            };
            let child_path = Path::new(&child);
            let child_stat = match statat(&directory, child_path, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => stat,
                Err(error) if error == rustix::io::Errno::NOENT => continue,
                Err(error) => return Err(io_error(error)),
            };
            if FileType::from_raw_mode(child_stat.st_mode).is_dir() {
                let mut components = frame.directory.components.clone();
                components.push(child);
                descend = Some(RemoveFrame::new(DirectoryFrame {
                    components,
                    identity: inode_key(&child_stat)?,
                }));
                break;
            }
            unlinkat(&directory, child_path, AtFlags::empty()).map_err(io_error)?;
        }
        if let Some(child) = descend {
            pending.push(child);
            continue;
        }
        let completed = pending.pop().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "reclaim stack became empty")
        })?;
        let name = completed.directory.components.last().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "reclaim frame has no name")
        })?;
        let parent_directory = match pending.last() {
            Some(frame) => open_frame_directory(parent, &frame.directory)?,
            None => parent.try_clone()?,
        };
        let current = statat(
            &parent_directory,
            Path::new(name),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(io_error)?;
        if inode_key(&current)? != completed.directory.identity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "reclaim directory changed before removal",
            ));
        }
        unlinkat(&parent_directory, Path::new(name), AtFlags::REMOVEDIR).map_err(io_error)?;
    }
    Ok(())
}

fn tree_bytes_at(
    parent: &File,
    name: &OsStr,
    seen: &mut BTreeSet<(u64, u64)>,
    skip: &dyn Fn(&str) -> bool,
) -> io::Result<u64> {
    let path = Path::new(name);
    let metadata = statat(parent, path, AtFlags::SYMLINK_NOFOLLOW).map_err(io_error)?;
    let file_type = FileType::from_raw_mode(metadata.st_mode);
    if file_type.is_file() {
        return Ok(if seen.insert(inode_key(&metadata)?) {
            u64::try_from(metadata.st_size).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid file size {}: {error}", metadata.st_size),
                )
            })?
        } else {
            0
        });
    }
    if !file_type.is_dir() {
        return Ok(0);
    }
    let mut total = 0_u64;
    let mut pending = vec![DirectoryFrame {
        components: vec![name.to_os_string()],
        identity: inode_key(&metadata)?,
    }];
    while let Some(frame) = pending.pop() {
        let directory = open_frame_directory(parent, &frame)?;
        let mut entries = Dir::read_from(&directory).map_err(io_error)?;
        while let Some(child) = next_entry_name(&mut entries)? {
            if child.to_str().is_some_and(skip) {
                continue;
            }
            let child_stat = match statat(&directory, Path::new(&child), AtFlags::SYMLINK_NOFOLLOW)
            {
                Ok(stat) => stat,
                Err(error) if error == rustix::io::Errno::NOENT => continue,
                Err(error) => return Err(io_error(error)),
            };
            let child_type = FileType::from_raw_mode(child_stat.st_mode);
            if child_type.is_dir() {
                let mut components = frame.components.clone();
                components.push(child);
                pending.push(DirectoryFrame {
                    components,
                    identity: inode_key(&child_stat)?,
                });
            } else if child_type.is_file() && seen.insert(inode_key(&child_stat)?) {
                let bytes = u64::try_from(child_stat.st_size).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid file size {}: {error}", child_stat.st_size),
                    )
                })?;
                total = total.saturating_add(bytes);
            }
        }
    }
    Ok(total)
}

/// Measure a family or generation below a track root without following a
/// substituted directory or nested symlink.
///
/// Each regular inode is counted once; `skip` applies inside the tree.
pub fn unique_inode_tree_bytes_below_track(
    track_root: &Path,
    path: &Path,
    skip: &dyn Fn(&str) -> bool,
) -> Result<u64, CoreError> {
    let track = open(track_root, DIRECTORY_FLAGS, Mode::empty())
        .map(File::from)
        .map_err(|error| storage("open track root", track_root, &io_error(error)))?;
    let (parent, name) = source_parent_at(track_root, path, &track)?;
    let metadata = statat(&parent, Path::new(&name), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| storage("inspect tree", path, &io_error(error)))?;
    if !FileType::from_raw_mode(metadata.st_mode).is_dir() {
        return Err(CoreError::Storage(format!(
            "reclaim: tree {} is not a directory",
            path.display()
        )));
    }
    tree_bytes_at(&parent, &name, &mut BTreeSet::new(), skip)
        .map_err(|error| storage("measure tree", path, &error))
}

/// Measure several generation roots with one inode set. Missing families or
/// generations are reported by position; an existing non-directory is an
/// error, never an absent generation.
pub fn unique_inode_tree_bytes_for_roots_below_track(
    track_root: &Path,
    roots: &[PathBuf],
    skip: &dyn Fn(&str) -> bool,
) -> Result<(u64, Vec<bool>), CoreError> {
    for root in roots {
        let _names = source_names(track_root, root)?;
    }
    if roots.is_empty() {
        return Ok((0, Vec::new()));
    }
    let track = match open(track_root, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(track) => File::from(track),
        Err(error) if error == rustix::io::Errno::NOENT => {
            return Ok((0, vec![false; roots.len()]));
        }
        Err(error) => return Err(storage("open track root", track_root, &io_error(error))),
    };
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    let mut present = Vec::with_capacity(roots.len());
    for root in roots {
        let Some((parent, name)) = source_parent_at_if_present(track_root, root, &track)? else {
            present.push(false);
            continue;
        };
        let metadata = match statat(&parent, Path::new(&name), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => metadata,
            Err(error) if error == rustix::io::Errno::NOENT => {
                present.push(false);
                continue;
            }
            Err(error) => return Err(storage("inspect generation", root, &io_error(error))),
        };
        if !FileType::from_raw_mode(metadata.st_mode).is_dir() {
            return Err(CoreError::Storage(format!(
                "reclaim: generation {} is not a directory",
                root.display()
            )));
        }
        let bytes = tree_bytes_at(&parent, &name, &mut seen, skip)
            .map_err(|error| storage("measure tree", root, &error))?;
        total = total.saturating_add(bytes);
        present.push(true);
    }
    Ok((total, present))
}

/// Measure the entire pinned track root, including its reclaim area.
pub fn unique_inode_tree_bytes_in_track(
    track_root: &Path,
    skip: &dyn Fn(&str) -> bool,
) -> Result<u64, CoreError> {
    let track = open(track_root, DIRECTORY_FLAGS, Mode::empty())
        .map(File::from)
        .map_err(|error| storage("open track root", track_root, &io_error(error)))?;
    tree_bytes_at(&track, OsStr::new("."), &mut BTreeSet::new(), skip)
        .map_err(|error| storage("measure track root", track_root, &error))
}

fn open_reclaim_area(track_root: &Path) -> Result<(File, File), CoreError> {
    let track = open(track_root, DIRECTORY_FLAGS, Mode::empty())
        .map(File::from)
        .map_err(|error| storage("open track root", track_root, &io_error(error)))?;
    let area = reclaim_area(track_root);
    match mkdirat(
        &track,
        RECLAIM_AREA_DIR_NAME,
        Mode::from_bits_truncate(0o700),
    ) {
        Ok(()) => track
            .sync_all()
            .map_err(|error| storage("sync track root", track_root, &error))?,
        Err(error) if error == rustix::io::Errno::EXIST => {}
        Err(error) => return Err(storage("create reclaim area", &area, &io_error(error))),
    }
    let opened = open_directory_at(&track, OsStr::new(RECLAIM_AREA_DIR_NAME))
        .map_err(|error| storage("open reclaim area", &area, &error))?;
    Ok((track, opened))
}

fn source_parent_at(
    track_root: &Path,
    source: &Path,
    track: &File,
) -> Result<(File, OsString), CoreError> {
    source_parent_at_if_present(track_root, source, track)?.ok_or_else(|| {
        CoreError::Storage(format!(
            "reclaim: source family for {} is absent",
            source.display()
        ))
    })
}

fn source_parent_at_if_present(
    track_root: &Path,
    source: &Path,
    track: &File,
) -> Result<Option<(File, OsString)>, CoreError> {
    let (family, name) = source_names(track_root, source)?;
    match family {
        None => Ok(Some((
            track
                .try_clone()
                .map_err(|error| storage("clone track root", track_root, &error))?,
            name,
        ))),
        Some(family) => {
            let parent = match open_directory_at(track, &family) {
                Ok(parent) => parent,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => {
                    return Err(storage(
                        "open generation family",
                        &track_root.join(&family),
                        &error,
                    ));
                }
            };
            Ok(Some((parent, name)))
        }
    }
}

fn source_names(
    track_root: &Path,
    source: &Path,
) -> Result<(Option<OsString>, OsString), CoreError> {
    let relative = source.strip_prefix(track_root).map_err(|error| {
        CoreError::InvalidContract(format!("reclaim source is outside track root: {error}"))
    })?;
    let components: Vec<_> = relative.components().collect();
    match components.as_slice() {
        [Component::Normal(name)] => Ok((None, (*name).to_os_string())),
        [Component::Normal(family), Component::Normal(name)] => {
            Ok((Some((*family).to_os_string()), (*name).to_os_string()))
        }
        _ => Err(CoreError::InvalidContract(format!(
            "reclaim source must be a family or generation under {}",
            track_root.display()
        ))),
    }
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
    reclaim_directory_removing_with(track_root, generation_dir, entry_name, remove_tree_at)
}

/// Dispose of one freshly inventoried quarantined family or generation via
/// the same crash-atomic, descriptor-anchored reclaim area. Its opaque entry
/// name cannot overlap a normal generation reclaim.
pub fn reclaim_quarantined_directory(track_root: &Path, path: &Path) -> Result<(), CoreError> {
    let _names = source_names(track_root, path)?;
    let relative = path.strip_prefix(track_root).map_err(|error| {
        CoreError::InvalidContract(format!("quarantine path is outside track root: {error}"))
    })?;
    let digest = Sha256::digest(relative.as_os_str().as_bytes());
    reclaim_directory(track_root, path, &format!("quarantine-{digest:x}"))
}

/// [`reclaim_directory`] with the final removal supplied, so a test can
/// fail it after the move.
fn reclaim_directory_removing_with(
    track_root: &Path,
    generation_dir: &Path,
    entry_name: &str,
    remove: impl FnOnce(&File, &OsStr) -> std::io::Result<()>,
) -> Result<(), CoreError> {
    let _names = source_names(track_root, generation_dir)?;
    if !matches!(
        Path::new(entry_name)
            .components()
            .collect::<Vec<_>>()
            .as_slice(),
        [Component::Normal(_)]
    ) {
        return Err(CoreError::InvalidContract(
            "reclaim entry name is not one path component".into(),
        ));
    }
    let area = reclaim_area(track_root);
    let (track, area_directory) = open_reclaim_area(track_root)?;
    let (parent, name) = source_parent_at(track_root, generation_dir, &track)?;
    let entry = area.join(entry_name);
    remove_tree_at(&area_directory, OsStr::new(entry_name))
        .map_err(|error| storage("remove earlier interrupted reclaim", &entry, &error))?;
    renameat(&parent, Path::new(&name), &area_directory, entry_name).map_err(|error| {
        storage(
            "move generation into the reclaim area",
            generation_dir,
            &io_error(error),
        )
    })?;
    parent
        .sync_all()
        .map_err(|error| storage("sync source parent", generation_dir, &error))?;
    area_directory
        .sync_all()
        .map_err(|error| storage("sync reclaim area", &area, &error))?;
    remove(&area_directory, OsStr::new(entry_name)).map_err(|error| {
        storage(
            "remove reclaimed generation (left in the reclaim area for the next pass)",
            &entry,
            &error,
        )
    })?;
    area_directory
        .sync_all()
        .map_err(|error| storage("sync reclaim area", &area, &error))
}

/// Remove every entry the reclaim area of `track_root` holds: the rest of
/// reclaims a crash or a failed removal interrupted. Idempotent; an absent
/// area has nothing to finish.
pub fn finish_interrupted_reclaims(track_root: &Path) -> Result<FinishedReclaims, CoreError> {
    let area = reclaim_area(track_root);
    match std::fs::symlink_metadata(&area) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(FinishedReclaims::default());
        }
        Ok(_) => {}
        Err(error) => return Err(storage("inspect reclaim area", &area, &error)),
    }
    let (_track, area_directory) = open_reclaim_area(track_root)?;
    let mut finished = FinishedReclaims::default();
    let mut seen = BTreeSet::new();
    loop {
        let names = entry_names_at_limit(&area_directory, REMOVE_BATCH_ENTRIES)
            .map_err(|error| storage("list", &area, &error))?;
        if names.is_empty() {
            break;
        }
        for name in names {
            let path = area.join(&name);
            let bytes = tree_bytes_at(&area_directory, &name, &mut seen, &|_name| false)
                .map_err(|error| storage("measure", &path, &error))?;
            remove_tree_at(&area_directory, &name)
                .map_err(|error| storage("remove", &path, &error))?;
            finished.bytes = finished.bytes.saturating_add(bytes);
            finished.entries = finished.entries.saturating_add(1);
        }
    }
    if finished.entries > 0 {
        area_directory
            .sync_all()
            .map_err(|error| storage("sync reclaim area", &area, &error))?;
    }
    Ok(finished)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use super::{
        FinishedReclaims, finish_interrupted_reclaims, reclaim_area, reclaim_directory,
        reclaim_directory_removing_with, reclaim_quarantined_directory,
        unique_inode_tree_bytes_below_track, unique_inode_tree_bytes_for_roots_below_track,
        unique_inode_tree_bytes_in_track,
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
            reclaim_directory_removing_with(root.path(), &dir, "family.g3", |_area, _name| {
                Err(io::Error::other("injected removal failure"))
            });
        if interrupted.is_ok() || dir.exists() {
            return Err(
                "the failed removal is an error and the namespace no longer holds g3".into(),
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
                format!("finishing removes the entry and its 12 bytes: {finished:?}").into(),
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

    #[test]
    fn a_family_symlink_cannot_redirect_a_generation_reclaim() -> TestResult {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let external = generation(outside.path(), "family", "g3", b"keep")?;
        std::os::unix::fs::symlink(
            external.parent().ok_or("missing family")?,
            root.path().join("family"),
        )?;
        if reclaim_directory(root.path(), &root.path().join("family/g3"), "family.g3").is_ok()
            || !external.join("payload/data").is_file()
        {
            return Err("reclaim followed a family symlink".into());
        }
        if unique_inode_tree_bytes_for_roots_below_track(
            root.path(),
            &[root.path().join("family/g3")],
            &|_| false,
        )
        .is_ok()
            || unique_inode_tree_bytes_below_track(
                root.path(),
                &root.path().join("family"),
                &|_| false,
            )
            .is_ok()
            || unique_inode_tree_bytes_in_track(root.path(), &|_| false)? != 0
        {
            return Err("measurement followed a family symlink".into());
        }
        Ok(())
    }

    #[test]
    fn a_reclaim_area_symlink_cannot_redirect_deletion() -> TestResult {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let dir = generation(root.path(), "family", "g3", b"keep")?;
        std::fs::create_dir(outside.path().join("family.g3"))?;
        std::fs::write(outside.path().join("family.g3/keep"), b"keep")?;
        std::os::unix::fs::symlink(outside.path(), reclaim_area(root.path()))?;
        if reclaim_directory(root.path(), &dir, "family.g3").is_ok()
            || !dir.join("payload/data").is_file()
            || !outside.path().join("family.g3/keep").is_file()
            || finish_interrupted_reclaims(root.path()).is_ok()
        {
            return Err("reclaim followed a symlinked reclaim area".into());
        }
        Ok(())
    }

    #[test]
    fn quarantined_family_removal_does_not_follow_nested_symlinks() -> TestResult {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let family = root.path().join("noncanonical-family");
        std::fs::create_dir(&family)?;
        std::fs::write(outside.path().join("keep"), b"keep")?;
        std::os::unix::fs::symlink(outside.path(), family.join("outside"))?;
        reclaim_quarantined_directory(root.path(), &family)?;
        if family.exists() || !outside.path().join("keep").is_file() {
            return Err("quarantine removal escaped the track root".into());
        }
        Ok(())
    }

    #[test]
    fn unreadable_regular_file_can_be_measured_and_reclaimed() -> TestResult {
        let root = tempfile::tempdir()?;
        let family = root.path().join("noncanonical-family");
        std::fs::create_dir(&family)?;
        let file = family.join("unreadable");
        std::fs::write(&file, b"payload")?;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000))?;
        let bytes = unique_inode_tree_bytes_below_track(root.path(), &family, &|_| false)?;
        if bytes != 7 {
            return Err(format!("expected seven metadata bytes, observed {bytes}").into());
        }
        reclaim_quarantined_directory(root.path(), &family)?;
        if family.exists() {
            return Err("unreadable file prevented quarantine removal".into());
        }
        Ok(())
    }

    #[test]
    fn multiple_generation_measurement_preserves_absence_and_hardlink_dedup() -> TestResult {
        let root = tempfile::tempdir()?;
        let base = generation(root.path(), "family", "g1", b"abc")?;
        let delta = generation(root.path(), "family", "g2", b"abc")?;
        std::fs::remove_file(delta.join("payload/data"))?;
        std::fs::hard_link(base.join("payload/data"), delta.join("payload/data"))?;
        let roots = vec![base, root.path().join("family/g3"), delta];
        let (bytes, present) =
            unique_inode_tree_bytes_for_roots_below_track(root.path(), &roots, &|_| false)?;
        if bytes != 7 || present != [true, false, true] {
            return Err(format!(
                "expected one shared payload and two identities: {bytes}, {present:?}"
            )
            .into());
        }
        if unique_inode_tree_bytes_in_track(root.path(), &|_| false)? != 7 {
            return Err("track measurement double-counted the hard link".into());
        }
        Ok(())
    }

    #[test]
    fn measurement_reports_all_absent_before_a_track_is_created() -> TestResult {
        let root = tempfile::tempdir()?;
        let missing_track = root.path().join("not-created");
        let roots = vec![missing_track.join("family/g1")];
        let measured =
            unique_inode_tree_bytes_for_roots_below_track(&missing_track, &roots, &|_| false)?;
        if measured != (0, vec![false]) {
            return Err(format!("missing track was not absent: {measured:?}").into());
        }
        let empty = unique_inode_tree_bytes_for_roots_below_track(&missing_track, &[], &|_| false)?;
        if empty != (0, Vec::new()) {
            return Err(format!("empty root set was not empty: {empty:?}").into());
        }
        if unique_inode_tree_bytes_for_roots_below_track(
            &missing_track,
            &[root.path().join("outside")],
            &|_| false,
        )
        .is_ok()
        {
            return Err("missing track accepted an out-of-root measurement".into());
        }
        Ok(())
    }

    #[test]
    fn quarantined_nested_tree_removes_more_than_one_entry_batch() -> TestResult {
        let root = tempfile::tempdir()?;
        let family = root.path().join("noncanonical-family");
        std::fs::create_dir(&family)?;
        let mut leaf = family.clone();
        for _ in 0..64 {
            leaf.push("d");
            std::fs::create_dir(&leaf)?;
        }
        for index in 0..260 {
            std::fs::write(leaf.join(format!("file-{index}")), b"x")?;
        }
        if unique_inode_tree_bytes_below_track(root.path(), &family, &|_| false)? != 260 {
            return Err("nested quarantine measurement lost files".into());
        }
        reclaim_quarantined_directory(root.path(), &family)?;
        if family.exists() {
            return Err("batched nested quarantine removal left the family".into());
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
