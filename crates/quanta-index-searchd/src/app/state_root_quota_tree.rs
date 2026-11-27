//! Bound the existing XFS project tree before admitting StateRoot writes.
//!
//! The caller owns the root directory descriptor and verifies its quota. Every
//! descendant is opened relative to a held directory with openat2 confinement;
//! directory entry types are never trusted as the membership witness.

use std::ffi::CStr;
use std::io;
use std::os::fd::{AsFd, OwnedFd};

use rustix::fs::{self, AtFlags, Dir, FileType, Mode, OFlags, ResolveFlags};

const MAX_TREE_DEPTH: u32 = 64;
pub(super) const RESOLVE_TREE: ResolveFlags = ResolveFlags::NO_XDEV
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::BENEATH);

fn invalid_tree(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

pub(super) fn require_member(fd: &impl AsFd, project_id: u32, directory: bool) -> io::Result<()> {
    let member = quanta_state_root_quota::read_xfs_project_membership(fd)?;
    if member.project_id != project_id || (directory && !member.inherits_project) {
        return Err(invalid_tree(
            "StateRoot inode is outside the inherited XFS project",
        ));
    }
    Ok(())
}

fn open_project_entry(
    directory: &impl AsFd,
    name: &CStr,
    enumerated_inode: u64,
) -> io::Result<(OwnedFd, FileType)> {
    // Reject special inodes before opening them. Then confine the actual
    // open and compare its inode with the directory entry just observed.
    let listed = fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
    if listed.st_ino != enumerated_inode {
        return Err(invalid_tree(
            "StateRoot directory entry changed during quota scan",
        ));
    }
    let kind = FileType::from_raw_mode(listed.st_mode);
    let type_flag = match kind {
        FileType::RegularFile => OFlags::empty(),
        FileType::Directory => OFlags::DIRECTORY,
        _ => {
            return Err(invalid_tree(
                "StateRoot project tree contains a special inode",
            ));
        }
    };
    let child = fs::openat2(
        directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOFOLLOW | type_flag,
        Mode::empty(),
        RESOLVE_TREE,
    )?;
    let stat = fs::fstat(&child)?;
    if stat.st_dev != listed.st_dev
        || stat.st_ino != listed.st_ino
        || FileType::from_raw_mode(stat.st_mode) != kind
    {
        return Err(invalid_tree(
            "StateRoot directory entry changed during quota scan",
        ));
    }
    Ok((child, kind))
}

fn scan_directory(
    directory: &impl AsFd,
    project_id: u32,
    max_entries: u64,
    entries: &mut u64,
    depth: u32,
    checkpoint: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<()> {
    checkpoint()?;
    if depth > MAX_TREE_DEPTH {
        return Err(invalid_tree("StateRoot project tree exceeds maximum depth"));
    }
    let mut iterator = Dir::read_from(directory)?;
    while let Some(entry) = iterator.read() {
        checkpoint()?;
        let entry = entry?;
        let name = entry.file_name();
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        *entries = entries
            .checked_add(1)
            .filter(|count| *count <= max_entries)
            .ok_or_else(|| invalid_tree("StateRoot project tree exceeds entry limit"))?;
        let (child, kind) = open_project_entry(directory, name, entry.ino())?;
        match kind {
            FileType::RegularFile => require_member(&child, project_id, false)?,
            FileType::Directory => {
                require_member(&child, project_id, true)?;
                scan_directory(
                    &child,
                    project_id,
                    max_entries,
                    entries,
                    depth.saturating_add(1),
                    checkpoint,
                )?;
            }
            _ => {
                return Err(invalid_tree(
                    "StateRoot project tree contains a special inode",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Dir, FileType, open_project_entry};
    use std::{fs, io, os::unix::fs::symlink};

    #[test]
    fn quota_scan_binds_the_inode_returned_by_directory_enumeration() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let leaf = temp.path().join("leaf");
        let retired = temp.path().join("retired");
        fs::write(&leaf, b"enumerated")?;
        let directory = fs::File::open(temp.path())?;
        let entry = Dir::read_from(&directory)?
            .find_map(|entry| match entry {
                Ok(entry) if entry.file_name() == c"leaf" => Some(Ok(entry)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .ok_or_else(|| io::Error::other("fixture entry missing"))??;
        let (_, kind) = open_project_entry(&directory, entry.file_name(), entry.ino())?;
        assert_eq!(kind, FileType::RegularFile);

        fs::rename(&leaf, &retired)?;
        fs::write(&leaf, b"replacement")?;
        let error = open_project_entry(&directory, entry.file_name(), entry.ino()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&leaf)?, b"replacement");
        assert_eq!(fs::read(&retired)?, b"enumerated");
        Ok(())
    }

    #[test]
    fn quota_scan_does_not_follow_an_enumerated_symlink() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let target = temp.path().join("target");
        fs::write(&target, b"must remain outside the entry")?;
        symlink(&target, temp.path().join("link"))?;
        let directory = fs::File::open(temp.path())?;
        let entry = Dir::read_from(&directory)?
            .find_map(|entry| match entry {
                Ok(entry) if entry.file_name() == c"link" => Some(Ok(entry)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .ok_or_else(|| io::Error::other("fixture symlink missing"))??;
        let error = open_project_entry(&directory, entry.file_name(), entry.ino()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(target)?, b"must remain outside the entry");
        Ok(())
    }
}

/// Require every existing regular file and directory to belong to one XFS
/// project. A directory must also pass the project ID to newly created files.
/// The root counts as one entry; depth and open descriptors have fixed bounds.
pub(super) fn require_xfs_project_tree(
    root: &impl AsFd,
    project_id: u32,
    max_entries: u64,
    checkpoint: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<()> {
    checkpoint()?;
    if project_id == 0 || max_entries == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "StateRoot project ID and entry bound must be nonzero",
        ));
    }
    if FileType::from_raw_mode(fs::fstat(root)?.st_mode) != FileType::Directory {
        return Err(invalid_tree("StateRoot project root is not a directory"));
    }
    require_member(root, project_id, true)?;
    let mut entries = 1;
    scan_directory(root, project_id, max_entries, &mut entries, 0, checkpoint)
}
