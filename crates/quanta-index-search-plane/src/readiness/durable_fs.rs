//! Durable filesystem primitives: parent-directory sync, atomic replace from
//! staging, and staging / legacy temporary reconciliation.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt, fs};

use quanta_index_core::CoreError;

#[cfg(unix)]
pub(super) fn read_regular_file_nofollow_v1(path: &Path) -> std::io::Result<Vec<u8>> {
    use rustix::fs::{Mode, OFlags, open};

    let descriptor = open(path, OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW, Mode::empty())
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    let mut file = File::from(descriptor);
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(format!(
            "authority path is not a regular file: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    let _bytes_read = file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(not(unix))]
pub(super) fn read_regular_file_nofollow_v1(path: &Path) -> std::io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::other(format!(
            "authority path is not a regular non-symlink file: {}",
            path.display()
        )));
    }
    fs::read(path)
}

pub(super) trait ParentDirectorySyncPort: fmt::Debug + Send + Sync {
    fn sync_parent(&self, parent: &Path) -> std::io::Result<()>;
}

pub(super) fn ensure_durable_directory_v1(
    path: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    let mut boundary = path.parent().ok_or_else(|| {
        CoreError::Storage(format!("{owner}: durable directory has no parent: {}", path.display()))
    })?;
    while !boundary.is_dir() {
        boundary = boundary.parent().ok_or_else(|| {
            CoreError::Storage(format!(
                "{owner}: durable directory has no existing ancestor: {}",
                path.display()
            ))
        })?;
    }
    ensure_durable_directory_from_boundary_v1(path, boundary, owner, parent_sync)
}

pub(super) fn ensure_durable_directory_from_boundary_v1(
    path: &Path,
    boundary: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    if path == boundary {
        return Ok(());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(CoreError::Storage(format!(
                    "{owner}: durable directory path is a symlink: {}",
                    path.display()
                )));
            }
            if !metadata.is_dir() {
                return Err(CoreError::Storage(format!(
                    "{owner}: durable directory path is not a directory: {}",
                    path.display()
                )));
            }
            let parent = path.parent().ok_or_else(|| {
                CoreError::Storage(format!(
                    "{owner}: durable directory has no parent: {}",
                    path.display()
                ))
            })?;
            ensure_durable_directory_from_boundary_v1(parent, boundary, owner, parent_sync)?;
            return parent_sync.sync_parent(parent).map_err(|err| {
                CoreError::Storage(format!(
                    "{owner}: revalidate durable-directory parent {}: {err}",
                    parent.display()
                ))
            });
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(CoreError::Storage(format!(
                "{owner}: inspect durable directory {}: {err}",
                path.display()
            )));
        }
    }
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!("{owner}: durable directory has no parent: {}", path.display()))
    })?;
    ensure_durable_directory_from_boundary_v1(parent, boundary, owner, parent_sync)?;
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(err) => {
            return Err(CoreError::Storage(format!(
                "{owner}: create durable directory {}: {err}",
                path.display()
            )));
        }
    }
    parent_sync.sync_parent(parent).map_err(|err| {
        CoreError::Storage(format!(
            "{owner}: fsync durable-directory parent {}: {err}",
            parent.display()
        ))
    })
}

pub(super) fn sync_existing_file_parent_v1(
    path: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!("{owner}: durable file has no parent: {}", path.display()))
    })?;
    parent_sync.sync_parent(parent).map_err(|err| {
        CoreError::Storage(format!(
            "{owner}: revalidate parent durability {}: {err}",
            parent.display()
        ))
    })
}

#[derive(Debug)]
pub(super) struct FsParentDirectorySyncPort;

impl ParentDirectorySyncPort for FsParentDirectorySyncPort {
    fn sync_parent(&self, parent: &Path) -> std::io::Result<()> {
        File::open(parent)?.sync_all()
    }
}

pub(super) enum AtomicFileWriteOutcomeV1 {
    Durable,
    RenamedButParentSyncFailed(CoreError),
}

pub(super) fn atomic_replace_file_from_staging_v1(
    path: &Path,
    bytes: &[u8],
    staging_dir: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<AtomicFileWriteOutcomeV1, CoreError> {
    static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let target_parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!("{owner}: durable file has no parent: {}", path.display()))
    })?;
    let target_parent_name = target_parent
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "{owner}: target parent has no UTF-8 name: {}",
                target_parent.display()
            ))
        })?;
    let target_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "{owner}: durable file has no UTF-8 name: {}",
                path.display()
            ))
        })?;
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = staging_dir.join(format!(
        "scv1-{target_parent_name}-{target_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: create staging file {}: {error}",
                temporary.display()
            ))
        })?;
    if let Err(error) = file.write_all(bytes) {
        return Err(cleanup_staging_temporary_v1(
            &temporary,
            staging_dir,
            parent_sync,
            CoreError::Storage(format!(
                "{owner}: write staging file {}: {error}",
                temporary.display()
            )),
        ));
    }
    if let Err(error) = file.sync_all() {
        return Err(cleanup_staging_temporary_v1(
            &temporary,
            staging_dir,
            parent_sync,
            CoreError::Storage(format!(
                "{owner}: fsync staging file {}: {error}",
                temporary.display()
            )),
        ));
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        return Err(cleanup_staging_temporary_v1(
            &temporary,
            staging_dir,
            parent_sync,
            CoreError::Storage(format!(
                "{owner}: rename staging file {} to {}: {error}",
                temporary.display(),
                path.display()
            )),
        ));
    }
    if let Err(error) = parent_sync.sync_parent(target_parent) {
        return Ok(AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(CoreError::Storage(
            format!("{owner}: fsync target parent {}: {error}", target_parent.display()),
        )));
    }
    match parent_sync.sync_parent(staging_dir) {
        Ok(()) => Ok(AtomicFileWriteOutcomeV1::Durable),
        Err(error) => Ok(AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(CoreError::Storage(
            format!("{owner}: fsync staging parent {}: {error}", staging_dir.display()),
        ))),
    }
}

pub(super) fn cleanup_staging_temporary_v1(
    temporary: &Path,
    staging_dir: &Path,
    parent_sync: &dyn ParentDirectorySyncPort,
    primary: CoreError,
) -> CoreError {
    match fs::remove_file(temporary) {
        Ok(()) => match parent_sync.sync_parent(staging_dir) {
            Ok(()) => primary,
            Err(error) => CoreError::Storage(format!(
                "{primary:?}; additionally failed to fsync staging cleanup {}: {error}",
                staging_dir.display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => primary,
        Err(error) => CoreError::Storage(format!(
            "{primary:?}; additionally failed to remove staging file {}: {error}",
            temporary.display()
        )),
    }
}

pub(super) fn is_owned_search_corpus_staging_name_v1(name: &str) -> bool {
    let Some((target, suffix)) = name.split_once(".tmp-") else {
        return false;
    };
    if !target.starts_with("scv1-")
        || !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return false;
    }
    let mut suffix = suffix.split('-');
    suffix
        .next()
        .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
        && suffix.next().is_some_and(|sequence| {
            !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
        })
        && suffix.next().is_none()
}

pub(super) fn reconcile_owned_staging_directory_v1(
    staging_dir: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    let mut removed = false;
    for entry in fs::read_dir(staging_dir).map_err(|error| {
        CoreError::Storage(format!(
            "{owner}: list staging directory {}: {error}",
            staging_dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: read staging directory entry in {}: {error}",
                staging_dir.display()
            ))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: inspect staging entry {}: {error}",
                path.display()
            ))
        })?;
        let owned_staging_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_owned_search_corpus_staging_name_v1);
        if !file_type.is_file() || !owned_staging_name {
            return Err(CoreError::Storage(format!(
                "{owner}: foreign staging entry {}",
                path.display()
            )));
        }
        fs::remove_file(&path).map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: remove abandoned staging file {}: {error}",
                path.display()
            ))
        })?;
        removed = true;
    }
    if removed {
        parent_sync.sync_parent(staging_dir).map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: fsync staging directory after reconciliation {}: {error}",
                staging_dir.display()
            ))
        })?;
    }
    Ok(())
}

pub(super) fn reconcile_legacy_atomic_temporaries_v1(
    directory: &Path,
    target_suffix: &str,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    let mut removed = false;
    for entry in fs::read_dir(directory).map_err(|error| {
        CoreError::Storage(format!(
            "{owner}: list legacy temporary directory {}: {error}",
            directory.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: read legacy temporary entry in {}: {error}",
                directory.display()
            ))
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_legacy_atomic_temporary_name_v1(name, target_suffix) {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: inspect legacy temporary {}: {error}",
                path.display()
            ))
        })?;
        if !file_type.is_file() {
            return Err(CoreError::Storage(format!(
                "{owner}: legacy temporary is not a regular file: {}",
                path.display()
            )));
        }
        fs::remove_file(&path).map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: remove abandoned legacy temporary {}: {error}",
                path.display()
            ))
        })?;
        removed = true;
    }
    if removed {
        parent_sync.sync_parent(directory).map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: fsync directory after legacy temporary reconciliation {}: {error}",
                directory.display()
            ))
        })?;
    }
    Ok(())
}

pub(super) fn is_legacy_atomic_temporary_name_v1(name: &str, target_suffix: &str) -> bool {
    let Some((target, suffix)) = name.split_once(".tmp-") else {
        return false;
    };
    if !target.starts_with('.') || !target.ends_with(target_suffix) {
        return false;
    }
    let mut suffix = suffix.split('-');
    suffix
        .next()
        .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
        && suffix.next().is_some_and(|sequence| {
            !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
        })
        && suffix.next().is_none()
}
