//! Test-only admission for the external disposable bounded ENOSPC mount.

use std::{fs, os::unix::fs::MetadataExt as _, path::PathBuf};

pub(super) fn bounded_volume_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let supplied = std::env::var_os("QUANTA_INDEX_PHYSICAL_ENOSPC_ROOT")
        .ok_or("bounded ENOSPC mount path absent")?;
    let root = PathBuf::from(supplied);
    if !root.is_absolute() || !fs::symlink_metadata(&root)?.file_type().is_dir() {
        return Err("bounded ENOSPC root is not an absolute physical directory".into());
    }
    if fs::canonicalize(&root)? != root {
        return Err("bounded ENOSPC root must be its canonical mountpoint".into());
    }
    let parent = root.parent().ok_or("bounded ENOSPC root has no parent")?;
    let device = fs::metadata(&root)?.dev();
    if device == fs::metadata(parent)?.dev() || device == fs::metadata(std::env::temp_dir())?.dev()
    {
        return Err("bounded ENOSPC root is not a separate mounted filesystem".into());
    }
    let stat = rustix::fs::statvfs(&root)?;
    let total = stat
        .f_blocks
        .checked_mul(stat.f_frsize)
        .ok_or("bounded ENOSPC filesystem capacity overflow")?;
    if !(32 * 1024 * 1024..=128 * 1024 * 1024).contains(&total) {
        return Err(format!(
            "bounded ENOSPC filesystem capacity must be 32..=128 MiB, got {total} bytes"
        )
        .into());
    }
    Ok(root)
}
