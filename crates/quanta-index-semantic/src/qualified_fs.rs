//! Filesystem mutations around the Lance connection use its same authority.
//! Legacy callers without admission retain their existing syscall behavior.

use std::fs;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::sync::Arc;

use lance::session::Session;
use lance_io::object_store::file_admission::{
    AdmittedFileStoreProvider, FileAdmission, FileMoveMode, copy_file_admitted,
    remove_dir_all_admitted,
};
use quanta_index_core::CoreError;
use quanta_index_core::{DiagnosticContextV1, FinishedReclaims};
use sha2::{Digest as _, Sha256};

use crate::SemanticMaintenanceFileFactory;
use crate::errors::{admitted_fs_err, fs_err};

pub(crate) fn lance_session(admission: &Arc<dyn FileAdmission>) -> Arc<Session> {
    let session = Arc::new(Session::default());
    session.store_registry().insert(
        "file",
        Arc::new(AdmittedFileStoreProvider::new(Arc::clone(admission))),
    );
    session
}

pub(crate) fn copy_file(
    from: &Path,
    to: &Path,
    authority: Option<&dyn FileAdmission>,
) -> Result<(), CoreError> {
    authority.map_or_else(
        || {
            fs::copy(from, to)
                .map(|_| ())
                .map_err(|error| fs_err("copy generation-local entry", from, &error))
        },
        |authority| {
            copy_file_admitted(from, to, authority)
                .map_err(|error| admitted_fs_err("copy generation-local entry", error))
        },
    )
}

pub(crate) fn hard_link(
    from: &Path,
    to: &Path,
    authority: Option<&dyn FileAdmission>,
) -> Result<(), CoreError> {
    let Some(authority) = authority else {
        return fs::hard_link(from, to)
            .map_err(|error| fs_err("link inherited dataset entry", from, &error));
    };
    authority
        .prepare_parent_dirs(to)
        .map_err(|error| admitted_fs_err("prepare inherited link parent", error))?;
    let mut ticket = authority
        .begin_link(from, to)
        .map_err(|error| admitted_fs_err("reserve inherited link", error))?;
    ticket
        .perform_and_sync()
        .map_err(|error| admitted_fs_err("settle inherited link", error))
}

pub(crate) fn rename(
    from: &Path,
    to: &Path,
    authority: Option<&dyn FileAdmission>,
) -> Result<(), CoreError> {
    let Some(authority) = authority else {
        return fs::rename(from, to)
            .map_err(|error| fs_err("rename dataset promotion entry", from, &error));
    };
    authority
        .prepare_parent_dirs(to)
        .map_err(|error| admitted_fs_err("prepare dataset move parent", error))?;
    let mut ticket = authority
        .begin_move(from, to, FileMoveMode::Overwrite)
        .map_err(|error| admitted_fs_err("reserve dataset move", error))?;
    ticket
        .perform_and_sync()
        .map_err(|error| admitted_fs_err("settle dataset move", error))
}

pub(crate) fn remove_file(
    path: &Path,
    authority: Option<&dyn FileAdmission>,
) -> Result<(), CoreError> {
    let Some(authority) = authority else {
        return fs::remove_file(path)
            .map_err(|error| fs_err("remove dataset promotion file", path, &error));
    };
    authority
        .delete_file_and_sync(path)
        .map_err(|error| admitted_fs_err("remove dataset promotion file", error))
}

pub(crate) fn remove_dir_all(
    path: &Path,
    authority: Option<&dyn FileAdmission>,
) -> Result<(), CoreError> {
    authority.map_or_else(
        || {
            fs::remove_dir_all(path)
                .map_err(|error| fs_err("remove dataset promotion directory", path, &error))
        },
        |authority| {
            remove_dir_all_admitted(path, authority)
                .map_err(|error| admitted_fs_err("remove dataset tree", error))
        },
    )
}

/// Crash-atomic qualified reclaim: every moved or removed directory passes
/// through the same physical binder as Lance's files. A previous interrupted
/// reclaim of this exact generation is completed first.
pub(crate) fn reclaim_directory(
    track_root: &Path,
    source: &Path,
    entry_name: &str,
    factory: Option<&dyn SemanticMaintenanceFileFactory>,
) -> Result<(), CoreError> {
    let Some(factory) = factory else {
        return quanta_index_core::reclaim_directory(track_root, source, entry_name);
    };
    let relative = source.strip_prefix(track_root).map_err(|error| {
        CoreError::InvalidContract(format!(
            "semantic reclaim source outside track root: {error}"
        ))
    })?;
    if !matches!(
        relative.components().collect::<Vec<_>>().as_slice(),
        [std::path::Component::Normal(_)]
            | [
                std::path::Component::Normal(_),
                std::path::Component::Normal(_)
            ]
    ) || !matches!(
        Path::new(entry_name)
            .components()
            .collect::<Vec<_>>()
            .as_slice(),
        [std::path::Component::Normal(_)]
    ) {
        return Err(CoreError::InvalidContract(
            "semantic reclaim requires a family/generation source and one-component entry".into(),
        ));
    }
    let entry = quanta_index_core::reclaim_area(track_root).join(entry_name);
    factory.prepare_retirement_area(&entry)?;
    let admission = factory.retirement_admission()?;
    admission
        .reconcile_pending_namespace()
        .map_err(|error| admitted_fs_err("reconcile earlier reclaim", error))?;
    match fs::symlink_metadata(&entry) {
        Ok(_) => remove_dir_all(&entry, Some(admission.as_ref()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(fs_err("inspect earlier reclaim", &entry, &error)),
    }
    let mut move_ticket = admission
        .begin_directory_move(source, &entry)
        .map_err(|error| admitted_fs_err("reserve reclaim directory move", error))?;
    move_ticket
        .perform_and_sync()
        .map_err(|error| admitted_fs_err("move reclaim directory", error))?;
    remove_dir_all(&entry, Some(admission.as_ref()))
}

pub(crate) fn reclaim_quarantined_directory(
    track_root: &Path,
    source: &Path,
    factory: Option<&dyn SemanticMaintenanceFileFactory>,
) -> Result<(), CoreError> {
    let Some(factory) = factory else {
        return quanta_index_core::reclaim_quarantined_directory(track_root, source);
    };
    let relative = source.strip_prefix(track_root).map_err(|error| {
        CoreError::InvalidContract(format!(
            "semantic quarantine source outside track root: {error}"
        ))
    })?;
    let digest = Sha256::digest(relative.as_os_str().as_bytes());
    reclaim_directory(
        track_root,
        source,
        &format!("quarantine-{digest:x}"),
        Some(factory),
    )
}

pub(crate) fn finish_interrupted_reclaims(
    track_root: &Path,
    diagnostic: Option<&DiagnosticContextV1>,
    factory: Option<&dyn SemanticMaintenanceFileFactory>,
) -> Result<FinishedReclaims, CoreError> {
    let Some(factory) = factory else {
        return quanta_index_core::finish_interrupted_reclaims(track_root, diagnostic);
    };
    let admission = factory.retirement_admission()?;
    admission
        .reconcile_pending_namespace()
        .map_err(|error| admitted_fs_err("reconcile interrupted reclaim", error))?;
    let area = quanta_index_core::reclaim_area(track_root);
    match fs::symlink_metadata(&area) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FinishedReclaims::default());
        }
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(CoreError::Storage(format!(
                "semantic reclaim area is not a directory: {}",
                area.display()
            )));
        }
        Err(error) => return Err(fs_err("inspect reclaim area", &area, &error)),
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&area).map_err(|error| fs_err("list reclaim area", &area, &error))? {
        entries.push(
            entry
                .map_err(|error| fs_err("read reclaim entry", &area, &error))?
                .path(),
        );
    }
    let (bytes, present) = quanta_index_core::unique_inode_tree_bytes_for_roots_below_track(
        track_root,
        &entries,
        &|_name| false,
        diagnostic,
    )?;
    let mut removed = 0_u64;
    for (entry, present) in entries.iter().zip(present) {
        if !present {
            continue;
        }
        remove_dir_all(entry, Some(admission.as_ref()))?;
        removed = removed.checked_add(1).ok_or_else(|| {
            CoreError::InvalidContract("semantic reclaim entry count overflow".into())
        })?;
    }
    Ok(FinishedReclaims {
        entries: removed,
        bytes,
    })
}
