//! Shared test-only physical object path and nofollow absence oracle.

use quanta_index_core::index::ObjectRef;
use std::{
    fmt::Write as _,
    io,
    path::{Path, PathBuf},
};

// Moved from native_index_uds: callers supply their original managed store
// root, so this remains one test path oracle across native physical fixtures.
pub(super) fn object_file(
    store_root: &Path,
    reference: ObjectRef,
) -> Result<PathBuf, std::fmt::Error> {
    let bytes = (reference.0).0;
    let mut name = String::with_capacity(64);
    for byte in bytes {
        write!(&mut name, "{byte:02x}")?;
    }
    Ok(store_root
        .join("objects")
        .join(format!(
            "{:02x}",
            bytes.first().copied().unwrap_or_default()
        ))
        .join(name))
}

pub(super) fn absent_nofollow(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(error),
    }
}
