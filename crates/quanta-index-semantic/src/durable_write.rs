//! Crash-atomic, crash-durable file replacement for generation sidecars.
//!
//! The lowest layer the build and the integrity receipts both write
//! through: a unique temporary file, synced, renamed over the target, and
//! the parent directory synced.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

use quanta_index_core::CoreError;

use crate::errors::fs_err;

#[cfg(test)]
static ATOMIC_WRITE_FAIL_BEFORE_RENAME_ACTION: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[cfg(test)]
pub(crate) fn set_atomic_write_fail_before_rename_action(action: Option<&str>) {
    let slot = ATOMIC_WRITE_FAIL_BEFORE_RENAME_ACTION.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(err) => err.into_inner(),
    };
    *guard = action.map(str::to_owned);
}

#[cfg(test)]
fn should_fail_atomic_write_before_rename(action: &str) -> bool {
    let slot = ATOMIC_WRITE_FAIL_BEFORE_RENAME_ACTION.get_or_init(|| Mutex::new(None));
    let guard = match slot.lock() {
        Ok(guard) => guard,
        Err(err) => err.into_inner(),
    };
    guard.as_deref() == Some(action)
}

static ATOMIC_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Crash-atomic and crash-durable file replacement for generation sidecars.
///
/// The file payload is synced before rename and the parent directory is synced
/// after rename. Pre-rename failures remove the unique temporary file so a
/// failed build cannot accumulate or later promote stale staging artifacts.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8], action: &str) -> Result<(), CoreError> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: {action} target has no parent: {}",
            path.display()
        ))
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: {action} target has no file name: {}",
            path.display()
        ))
    })?;
    let sequence = ATOMIC_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut staging_name = OsString::from(".");
    staging_name.push(file_name);
    staging_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    let staging = parent.join(staging_name);

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .map_err(|err| fs_err(action, &staging, &err))?;
    if let Err(err) = file.write_all(bytes) {
        drop(file);
        return Err(cleanup_atomic_temporary(
            &staging,
            fs_err(action, &staging, &err),
        ));
    }
    if let Err(err) = file.sync_all() {
        drop(file);
        return Err(cleanup_atomic_temporary(
            &staging,
            fs_err(action, &staging, &err),
        ));
    }
    drop(file);

    #[cfg(test)]
    if should_fail_atomic_write_before_rename(action) {
        return Err(cleanup_atomic_temporary(
            &staging,
            CoreError::Storage(format!("semantic: injected {action} failure before rename")),
        ));
    }

    if let Err(err) = fs::rename(&staging, path) {
        return Err(cleanup_atomic_temporary(
            &staging,
            fs_err(action, path, &err),
        ));
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| fs_err(action, parent, &err))
}

fn cleanup_atomic_temporary(staging: &Path, primary: CoreError) -> CoreError {
    match fs::remove_file(staging) {
        Ok(()) => primary,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => primary,
        Err(cleanup) => CoreError::Storage(format!(
            "semantic: {primary}; additionally failed to remove atomic temporary {}: {cleanup}",
            staging.display()
        )),
    }
}
