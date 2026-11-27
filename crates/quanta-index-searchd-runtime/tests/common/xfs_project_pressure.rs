//! Fill a verified disposable XFS project without requiring one giant extent.

use std::{error::Error, fs::File, io, os::fd::AsFd, os::unix::fs::FileExt as _, path::Path};

use quanta_state_root_quota::read_xfs_project_quota_via;
use rustix::{
    fs::{FallocateFlags, fallocate, fstatvfs},
    io::Errno,
};

const MAX_FILL_CHUNK_BYTES: u64 = 64 * 1024 * 1024;

pub(super) fn is_space_exhaustion(errno: Errno) -> bool {
    matches!(errno, Errno::DQUOT | Errno::NOSPC)
}

pub(super) fn fill_until_quota(
    file: &File,
    project: &impl AsFd,
    verifier: &Path,
    hard_bytes: u64,
    block_bytes: u64,
) -> Result<(), Box<dyn Error>> {
    let mut blocks = hard_bytes
        .min(MAX_FILL_CHUNK_BYTES)
        .checked_div(block_bytes)
        .ok_or("XFS reported a zero allocation unit")?;
    let mut offset = 0_u64;
    while blocks > 0 {
        let width = blocks
            .checked_mul(block_bytes)
            .ok_or("XFS fill width overflow")?;
        if let Some(end) = offset.checked_add(width).filter(|end| *end <= hard_bytes) {
            match fallocate(file, FallocateFlags::empty(), offset, width) {
                Ok(()) => {
                    offset = end;
                    continue;
                }
                Err(errno) if is_space_exhaustion(errno) => {}
                Err(error) => {
                    return Err(format!(
                        "XFS pressure fallocate at {offset}, width {width}: {error}"
                    )
                    .into());
                }
            }
        }
        blocks >>= 1;
    }
    // Preallocation can fail while the project still has room for writes.
    // Finish with synchronous one-block writes, then verify the actual limit.
    let block = vec![0; usize::try_from(block_bytes)?];
    while offset <= hard_bytes {
        let result = file.write_at(&block, offset).and_then(|written| {
            if written != block.len() {
                return Err(io::Error::other("short XFS quota probe write"));
            }
            file.sync_all()
        });
        match result {
            Ok(()) => {
                offset = offset
                    .checked_add(block_bytes)
                    .ok_or("XFS probe offset overflow")?
            }
            Err(error)
                if error
                    .raw_os_error()
                    .is_some_and(|errno| is_space_exhaustion(Errno::from_raw_os_error(errno))) =>
            {
                let observed = read_xfs_project_quota_via(verifier, project)?;
                let filesystem = fstatvfs(file)?;
                if observed.block_hard_limit_bytes != hard_bytes
                    || observed.used_bytes != hard_bytes
                    || !observed.accounting_enabled
                    || !observed.enforcement_enabled
                    || filesystem.f_bavail <= 1
                {
                    return Err(format!(
                        "XFS {error} did not prove project saturation with volume headroom: {observed:?}, available blocks={}",
                        filesystem.f_bavail
                    )
                    .into());
                }
                return Ok(());
            }
            Err(error) => return Err(format!("XFS pressure write at {offset}: {error}").into()),
        }
    }
    Err("kernel accepted a block allocation beyond the verified XFS project limit".into())
}
