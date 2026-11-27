//! Bounded inode exhaustion on a verified disposable XFS project.

use std::{
    error::Error,
    fs::{self, OpenOptions},
    os::fd::AsFd,
    path::Path,
};

use quanta_state_root_quota::read_xfs_project_quota_via;
use rustix::{fs::fstatvfs, io::Errno};

use super::xfs_project_pressure::is_space_exhaustion;

pub(super) const MAX_INODE_PRESSURE_LIMIT: u64 = 4096;
const MIN_BLOCK_HEADROOM_BYTES: u64 = 1024 * 1024;

/// Create only empty, closed files in a private sibling of the product root.
/// The caller removes names `inode-pressure-{index:04}` before retrying work.
pub(super) fn fill_inodes_until_quota(
    directory: &Path,
    project: &impl AsFd,
    verifier: &Path,
    hard_bytes: u64,
    hard_inodes: u64,
) -> Result<u64, Box<dyn Error>> {
    if hard_inodes == 0 || hard_inodes > MAX_INODE_PRESSURE_LIMIT {
        return Err("XFS inode pressure fixture exceeds its fixed 4096-inode ceiling".into());
    }
    let before = read_xfs_project_quota_via(verifier, project)?;
    if before.inode_hard_limit != hard_inodes
        || before.block_hard_limit_bytes != hard_bytes
        || !before.inherits_project
        || !before.accounting_enabled
        || !before.enforcement_enabled
        || before.used_inodes >= hard_inodes
    {
        return Err(
            format!("XFS inode pressure starts outside the verified project: {before:?}").into(),
        );
    }
    let needed = hard_inodes - before.used_inodes;
    for index in 0..needed {
        let path = directory.join(format!("inode-pressure-{index:04}"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        drop(file);
    }
    let observed = read_xfs_project_quota_via(verifier, project)?;
    // XFS reports project-limited statfs values on project-inheriting
    // directories. A regular filler file witnesses whole-volume headroom.
    let filesystem = fstatvfs(fs::File::open(directory.join("inode-pressure-0000"))?)?;
    let free_bytes = filesystem
        .f_bavail
        .checked_mul(filesystem.f_frsize)
        .ok_or("XFS filesystem free-byte calculation overflow")?;
    if observed.project_id != before.project_id
        || !observed.inherits_project
        || observed.inode_hard_limit != hard_inodes
        || observed.block_hard_limit_bytes != hard_bytes
        || observed.used_inodes != hard_inodes
        || !observed.accounting_enabled
        || !observed.enforcement_enabled
        || observed.used_bytes >= hard_bytes.saturating_sub(MIN_BLOCK_HEADROOM_BYTES)
        || free_bytes < MIN_BLOCK_HEADROOM_BYTES
        || filesystem.f_favail <= 1
    {
        return Err(format!(
            "XFS inode pressure lacks exact project saturation and independent disk headroom: {observed:?}, free bytes={free_bytes}, free inodes={}",
            filesystem.f_favail
        )
        .into());
    }
    let probe = directory.join("inode-boundary-probe");
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Err(error)
            if error
                .raw_os_error()
                .is_some_and(|errno| is_space_exhaustion(Errno::from_raw_os_error(errno))) =>
        {
            if read_xfs_project_quota_via(verifier, project)?.used_inodes != hard_inodes {
                return Err("XFS refused inode creation without a saturated project".into());
            }
            eprintln!(
                "XFS_INODE_QUOTA_REFUSAL errno={} project={} used_inodes={} hard_inodes={} used_bytes={} hard_bytes={} free_bytes={} free_inodes={}",
                error
                    .raw_os_error()
                    .ok_or("XFS inode refusal lacked errno")?,
                observed.project_id,
                observed.used_inodes,
                hard_inodes,
                observed.used_bytes,
                hard_bytes,
                free_bytes,
                filesystem.f_favail,
            );
            Ok(needed)
        }
        Ok(file) => {
            drop(file);
            fs::remove_file(probe)?;
            Err("kernel created an inode beyond the verified XFS project limit".into())
        }
        Err(error) => Err(format!("XFS boundary create failed for another reason: {error}").into()),
    }
}
