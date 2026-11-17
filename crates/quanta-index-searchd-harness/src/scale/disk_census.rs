//! Allocated-block census shared by the Legacy and Native benchmark observers.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use anyhow::{Context as _, Result};

/// Disk census policy for a timed phase. Both modes retain boundary snapshots.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiskObservationMode {
    #[default]
    BoundaryOnly,
    Periodic,
}

impl DiskObservationMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BoundaryOnly => "boundary-only",
            Self::Periodic => "periodic",
        }
    }
}

#[cfg(all(unix, feature = "legacy-bench"))]
pub(crate) fn allocated_root_bytes_with_cancel(
    root: &Path,
    cancelled: impl FnMut() -> bool,
) -> Result<u64> {
    let [total, _, _] = allocated_root_scopes(root, cancelled, |_path, _metadata| Ok(0))?;
    Ok(total)
}

#[cfg(unix)]
fn allocated_root_scopes(
    root: &Path,
    mut cancelled: impl FnMut() -> bool,
    mut subsets: impl FnMut(&Path, &fs::Metadata) -> Result<u8>,
) -> Result<[u64; 3]> {
    let mut pending = vec![root.to_path_buf()];
    // Each inode records the fixture, state and scratch memberships in order.
    #[expect(
        clippy::disallowed_types,
        reason = "Transient inode membership only; map iteration never selects observations or serialized order"
    )]
    let mut seen = std::collections::HashMap::<(u64, u64), u8>::new();
    let mut totals = [0_u64; 3];
    while let Some(path) = pending.pop() {
        anyhow::ensure!(!cancelled(), "disk_probe_cancelled");
        let metadata = fs::symlink_metadata(&path)?;
        let scopes = 1 | subsets(&path, &metadata)?;
        let prior = seen.entry((metadata.dev(), metadata.ino())).or_default();
        let new_scopes = scopes & !*prior;
        *prior |= scopes;
        // A hard link reached outside a subset must still count when the
        // same inode is later reached through a path inside that subset.
        if new_scopes == 0 {
            continue;
        }
        let bytes = metadata
            .blocks()
            .checked_mul(512)
            .context("scale: allocated-block byte count overflow")?;
        for (scope, total) in totals.iter_mut().enumerate() {
            if new_scopes & (1 << scope) != 0 {
                *total = total
                    .checked_add(bytes)
                    .context("scale: allocated root byte count overflow")?;
            }
        }
        if new_scopes & 1 != 0 && metadata.is_dir() {
            anyhow::ensure!(!cancelled(), "disk_probe_cancelled");
            for entry in fs::read_dir(path)? {
                anyhow::ensure!(!cancelled(), "disk_probe_cancelled");
                pending.push(entry?.path());
            }
        }
    }
    Ok(totals)
}

#[cfg(unix)]
pub(crate) fn allocated_native_fixture_with_cancel(
    state: &Path,
    cancelled: impl FnMut() -> bool,
) -> Result<[u64; 3]> {
    let fixture = state
        .parent()
        .context("native state root has no fixture parent")?;
    let scratch = state.join("native-scratch-v1");
    let mut state_seen = false;
    let totals = allocated_root_scopes(fixture, cancelled, |path, metadata| {
        if path == state {
            anyhow::ensure!(metadata.is_dir(), "native state root is not a directory");
            state_seen = true;
        }
        Ok((u8::from(path.starts_with(state)) << 1) | (u8::from(path.starts_with(&scratch)) << 2))
    })?;
    anyhow::ensure!(state_seen, "native state root absent from fixture census");
    Ok(totals)
}
