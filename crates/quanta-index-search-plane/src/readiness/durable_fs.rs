//! Durable filesystem primitives: parent-directory sync, atomic replace from
//! staging, and staging / legacy temporary reconciliation.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fmt, fs};

use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::{
    DurableControlFileAdmissionPort, PublicationOutputPermit,
};

#[derive(Clone)]
pub(super) struct DurableControlAdmissionV1(pub Arc<dyn DurableControlFileAdmissionPort>);

impl fmt::Debug for DurableControlAdmissionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DurableControlAdmissionV1")
    }
}

#[cfg(unix)]
pub(super) fn read_regular_file_nofollow_v1(path: &Path) -> std::io::Result<Vec<u8>> {
    use rustix::fs::{Mode, OFlags, open};

    let descriptor = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
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
        CoreError::Storage(format!(
            "{owner}: durable directory has no parent: {}",
            path.display()
        ))
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
        CoreError::Storage(format!(
            "{owner}: durable directory has no parent: {}",
            path.display()
        ))
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
        CoreError::Storage(format!(
            "{owner}: durable file has no parent: {}",
            path.display()
        ))
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
    RenamedButAdmissionCommitFailed(CoreError),
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
        CoreError::Storage(format!(
            "{owner}: durable file has no parent: {}",
            path.display()
        ))
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
        return Ok(AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(
            CoreError::Storage(format!(
                "{owner}: fsync target parent {}: {error}",
                target_parent.display()
            )),
        ));
    }
    match parent_sync.sync_parent(staging_dir) {
        Ok(()) => Ok(AtomicFileWriteOutcomeV1::Durable),
        Err(error) => Ok(AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(
            CoreError::Storage(format!(
                "{owner}: fsync staging parent {}: {error}",
                staging_dir.display()
            )),
        )),
    }
}

/// The qualified lifecycle writer. The encoded payload is the exact upper
/// bound, and a distinct Control file slot is held before the first create.
pub(super) fn atomic_replace_file_from_staging_admitted_v1(
    path: &Path,
    bytes: &[u8],
    staging_dir: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
    admission: &dyn DurableControlFileAdmissionPort,
) -> Result<AtomicFileWriteOutcomeV1, CoreError> {
    atomic_write_file_from_staging_admitted_v1(
        path,
        bytes,
        staging_dir,
        AdmittedPublishContext {
            owner,
            parent_sync,
            admission,
            mode: AdmittedPublishMode::Replace,
        },
    )
}

/// First-birth Control file publication. An exclusive rename preserves the
/// staging-to-target transition and refuses to replace an existing identity.
pub(super) fn atomic_create_file_from_staging_admitted_v1(
    path: &Path,
    bytes: &[u8],
    staging_dir: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
    admission: &dyn DurableControlFileAdmissionPort,
) -> Result<AtomicFileWriteOutcomeV1, CoreError> {
    atomic_write_file_from_staging_admitted_v1(
        path,
        bytes,
        staging_dir,
        AdmittedPublishContext {
            owner,
            parent_sync,
            admission,
            mode: AdmittedPublishMode::CreateNew,
        },
    )
}

/// Prepare only the cursor key's private namespace and reconcile its owned
/// temporary files before deciding whether the canonical key exists.
pub(crate) fn prepare_cursor_key_admitted_v1(
    state_root: &Path,
    admission: &dyn DurableControlFileAdmissionPort,
) -> Result<(), CoreError> {
    let owner = "cursor key";
    let keys_dir = state_root.join("keys");
    let staging_dir = keys_dir.join(".staging");
    prepare_cursor_key_directories_v1(state_root, &keys_dir, &staging_dir)?;
    reconcile_owned_staging_directory_admitted_v1(
        &staging_dir,
        owner,
        &FsParentDirectorySyncPort,
        admission,
    )
}

#[cfg(unix)]
fn prepare_cursor_key_directories_v1(
    state_root: &Path,
    keys_dir: &Path,
    staging_dir: &Path,
) -> Result<(), CoreError> {
    use rustix::fs::{Mode, OFlags, mkdirat, open, openat};
    use std::os::unix::fs::MetadataExt as _;

    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root = File::from(open(state_root, flags, Mode::empty()).map_err(|error| {
        CoreError::storage_io(
            "cursor key: open held state root",
            &std::io::Error::from(error),
        )
    })?);
    let root_metadata = root
        .metadata()
        .map_err(|error| CoreError::storage_io("cursor key: stat held state root", &error))?;
    let named_root = fs::symlink_metadata(state_root)
        .map_err(|error| CoreError::storage_io("cursor key: stat named state root", &error))?;
    if !named_root.is_dir()
        || named_root.dev() != root_metadata.dev()
        || named_root.ino() != root_metadata.ino()
    {
        return Err(CoreError::InvalidContract(
            "cursor key: state-root identity changed before directory creation".into(),
        ));
    }

    let child = |parent: &File, name: &str, path: &Path| -> Result<File, CoreError> {
        let opened = match openat(parent, name, flags, Mode::empty()) {
            Ok(file) => File::from(file),
            Err(rustix::io::Errno::NOENT) => {
                match mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => {
                        return Err(CoreError::storage_io(
                            "cursor key: mkdirat private parent",
                            &std::io::Error::from(error),
                        ));
                    }
                }
                let directory =
                    File::from(openat(parent, name, flags, Mode::empty()).map_err(|error| {
                        CoreError::storage_io(
                            "cursor key: open new private parent",
                            &std::io::Error::from(error),
                        )
                    })?);
                directory.sync_all().map_err(|error| {
                    CoreError::storage_io("cursor key: sync new private parent", &error)
                })?;
                parent.sync_all().map_err(|error| {
                    CoreError::storage_io("cursor key: sync parent of private directory", &error)
                })?;
                directory
            }
            Err(error) => {
                return Err(CoreError::storage_io(
                    "cursor key: open private parent",
                    &std::io::Error::from(error),
                ));
            }
        };
        let held = opened
            .metadata()
            .map_err(|error| CoreError::storage_io("cursor key: stat held parent", &error))?;
        let named = fs::symlink_metadata(path)
            .map_err(|error| CoreError::storage_io("cursor key: stat named parent", &error))?;
        let unsafe_mode = if name == ".staging" {
            held.mode() & 0o077 != 0
        } else {
            // Earlier unqualified roots may have created `keys` as 0755.
            // Its final key is 0600 and new staging remains private.
            held.mode() & 0o022 != 0
        };
        if !held.is_dir()
            || !named.is_dir()
            || held.dev() != named.dev()
            || held.ino() != named.ino()
            || held.uid() != rustix::process::geteuid().as_raw()
            || unsafe_mode
        {
            return Err(CoreError::InvalidContract(
                "cursor key: private directory identity or permissions changed".into(),
            ));
        }
        Ok(opened)
    };
    let keys = child(&root, "keys", keys_dir)?;
    let staging = child(&keys, ".staging", staging_dir)?;
    for (held, named) in [
        (&root, state_root),
        (&keys, keys_dir),
        (&staging, staging_dir),
    ] {
        let held_metadata = held
            .metadata()
            .map_err(|error| CoreError::storage_io("cursor key: restat held directory", &error))?;
        let named_metadata = fs::symlink_metadata(named)
            .map_err(|error| CoreError::storage_io("cursor key: restat named directory", &error))?;
        let private_mode_changed = if named == staging_dir {
            held_metadata.mode() & 0o077 != 0
        } else if named == keys_dir {
            held_metadata.mode() & 0o022 != 0
        } else {
            false
        };
        if !named_metadata.is_dir()
            || held_metadata.dev() != named_metadata.dev()
            || held_metadata.ino() != named_metadata.ino()
            || (named != state_root && held_metadata.uid() != rustix::process::geteuid().as_raw())
            || private_mode_changed
        {
            return Err(CoreError::InvalidContract(
                "cursor key: directory identity changed after mkdirat".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_cursor_key_directories_v1(
    _state_root: &Path,
    _keys_dir: &Path,
    _staging_dir: &Path,
) -> Result<(), CoreError> {
    Err(CoreError::InvalidContract(
        "cursor key: private directory admission requires Unix".into(),
    ))
}

/// Publish one 40-byte cursor key through the existing Control writer path.
pub(crate) fn create_cursor_key_admitted_v1(
    key_path: &Path,
    raw: &[u8; 40],
    admission: &dyn DurableControlFileAdmissionPort,
) -> Result<(), CoreError> {
    let keys_dir = key_path.parent().ok_or_else(|| {
        CoreError::InvalidContract("cursor key target lacks a parent directory".into())
    })?;
    let outcome = atomic_create_file_from_staging_admitted_v1(
        key_path,
        raw,
        &keys_dir.join(".staging"),
        "cursor key",
        &FsParentDirectorySyncPort,
        admission,
    )?;
    match outcome {
        AtomicFileWriteOutcomeV1::Durable => Ok(()),
        AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(error)
        | AtomicFileWriteOutcomeV1::RenamedButAdmissionCommitFailed(error) => Err(error),
    }
}

#[derive(Clone, Copy)]
enum AdmittedPublishMode {
    Replace,
    CreateNew,
}

#[derive(Clone, Copy)]
struct AdmittedPublishContext<'a> {
    owner: &'a str,
    parent_sync: &'a dyn ParentDirectorySyncPort,
    admission: &'a dyn DurableControlFileAdmissionPort,
    mode: AdmittedPublishMode,
}

#[cfg(unix)]
fn exclusive_rename_v1(from: &Path, to: &Path) -> std::io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)
}

#[cfg(not(unix))]
fn exclusive_rename_v1(_from: &Path, _to: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "exclusive Control-file rename requires Unix",
    ))
}

fn atomic_write_file_from_staging_admitted_v1(
    path: &Path,
    bytes: &[u8],
    staging_dir: &Path,
    context: AdmittedPublishContext<'_>,
) -> Result<AtomicFileWriteOutcomeV1, CoreError> {
    static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let AdmittedPublishContext {
        owner,
        parent_sync,
        admission,
        mode,
    } = context;

    let target_parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "{owner}: durable file has no parent: {}",
            path.display()
        ))
    })?;
    let target_parent_name = target_parent
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CoreError::Storage(format!("{owner}: target parent has no UTF-8 name")))?;
    let target_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CoreError::Storage(format!("{owner}: durable file has no UTF-8 name")))?;
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = staging_dir.join(format!(
        "scv1-{target_parent_name}-{target_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let exact = u64::try_from(bytes.len()).map_err(|error| {
        CoreError::InvalidContract(format!("encoded lifecycle file exceeds u64: {error}"))
    })?;
    let bound = exact.max(1);
    let mut permit = admission.reserve_output(bound)?;
    permit.bind_output(&temporary, path)?;
    permit.creation_started()?;
    let mut options = OpenOptions::new();
    let _configured = options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let _private_mode = options.mode(0o600);
    }
    let mut file = match options.open(&temporary) {
        Ok(file) => file,
        Err(error) => {
            let primary = CoreError::Storage(format!(
                "{owner}: create staging file {}: {error}",
                temporary.display()
            ));
            // A failed create yielded no writer FD. Only a proven absent name
            // and synced parent can release its provisional reservation.
            let absence = match fs::symlink_metadata(&temporary) {
                Err(absent) if absent.kind() == std::io::ErrorKind::NotFound => parent_sync
                    .sync_parent(staging_dir)
                    .map_err(|sync_error| {
                        CoreError::Storage(format!(
                            "{owner}: fsync failed-create parent {}: {sync_error}",
                            staging_dir.display()
                        ))
                    })
                    .and_then(|()| permit.confirmed_uncreated_absent_and_synced()),
                Ok(_) => Err(CoreError::Storage(format!(
                    "{owner}: failed-create staging name is occupied: {}",
                    temporary.display()
                ))),
                Err(inspect_error) => Err(CoreError::Storage(format!(
                    "{owner}: inspect failed-create staging name {}: {inspect_error}",
                    temporary.display()
                ))),
            };
            return Err(match absence {
                Ok(()) => primary,
                Err(proof_error) => {
                    append_custody_error(primary, "failed-create absence", &proof_error)
                }
            });
        }
    };
    if let Err(error) = permit.created(&file, &temporary) {
        return Err(cleanup_admitted_staging_v1(
            &temporary,
            staging_dir,
            parent_sync,
            &mut *permit,
            AdmittedWriterCustody::Unconfirmed(file),
            error,
        ));
    }
    if let Err(error) = file.write_all(bytes) {
        return Err(cleanup_admitted_staging_v1(
            &temporary,
            staging_dir,
            parent_sync,
            &mut *permit,
            AdmittedWriterCustody::Confirmed(file),
            CoreError::Storage(format!(
                "{owner}: write staging file {}: {error}",
                temporary.display()
            )),
        ));
    }
    if let Err(error) = file.sync_all() {
        return Err(cleanup_admitted_staging_v1(
            &temporary,
            staging_dir,
            parent_sync,
            &mut *permit,
            AdmittedWriterCustody::Confirmed(file),
            CoreError::Storage(format!(
                "{owner}: fsync staging file {}: {error}",
                temporary.display()
            )),
        ));
    }
    if let Err(error) = permit.before_replace() {
        return Err(cleanup_admitted_staging_v1(
            &temporary,
            staging_dir,
            parent_sync,
            &mut *permit,
            AdmittedWriterCustody::Confirmed(file),
            error,
        ));
    }
    let publish = match mode {
        AdmittedPublishMode::Replace => fs::rename(&temporary, path),
        AdmittedPublishMode::CreateNew => exclusive_rename_v1(&temporary, path),
    };
    if let Err(error) = publish {
        return Err(cleanup_admitted_staging_v1(
            &temporary,
            staging_dir,
            parent_sync,
            &mut *permit,
            AdmittedWriterCustody::Confirmed(file),
            CoreError::Storage(format!(
                "{owner}: publish staging file {} to {}: {error}",
                temporary.display(),
                path.display()
            )),
        ));
    }
    if let Err(error) = parent_sync.sync_parent(target_parent) {
        let primary = CoreError::Storage(format!(
            "{owner}: fsync target parent {}: {error}",
            target_parent.display()
        ));
        return Ok(AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(
            match permit.retire_writer_preserving_pending(file) {
                Ok(()) => primary,
                Err(retirement_error) => {
                    append_custody_error(primary, "uncertain writer retirement", &retirement_error)
                }
            },
        ));
    }
    if let Err(error) = parent_sync.sync_parent(staging_dir) {
        let primary = CoreError::Storage(format!(
            "{owner}: fsync staging parent {}: {error}",
            staging_dir.display()
        ));
        return Ok(AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(
            match permit.retire_writer_preserving_pending(file) {
                Ok(()) => primary,
                Err(retirement_error) => {
                    append_custody_error(primary, "uncertain writer retirement", &retirement_error)
                }
            },
        ));
    }
    match permit.committed(file, exact) {
        Ok(()) => Ok(AtomicFileWriteOutcomeV1::Durable),
        Err(error) => Ok(AtomicFileWriteOutcomeV1::RenamedButAdmissionCommitFailed(
            error,
        )),
    }
}

enum AdmittedWriterCustody {
    /// `created` bound the actual writer FD to the physical inode witness.
    Confirmed(File),
    /// `created` failed, so absence cannot justify a provisional refund.
    Unconfirmed(File),
}

fn cleanup_admitted_staging_v1(
    temporary: &Path,
    staging_dir: &Path,
    parent_sync: &dyn ParentDirectorySyncPort,
    permit: &mut dyn PublicationOutputPermit,
    custody: AdmittedWriterCustody,
    primary: CoreError,
) -> CoreError {
    let absence = match fs::remove_file(temporary) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CoreError::Storage(format!(
            "remove staging file {}: {error}",
            temporary.display()
        ))),
    }
    .and_then(|()| {
        parent_sync.sync_parent(staging_dir).map_err(|error| {
            CoreError::Storage(format!(
                "fsync staging cleanup parent {}: {error}",
                staging_dir.display()
            ))
        })
    });
    match absence {
        Ok(()) => {
            let retired = match custody {
                AdmittedWriterCustody::Confirmed(writer) => {
                    permit.confirmed_safe_absent_after_writer_close(writer)
                }
                AdmittedWriterCustody::Unconfirmed(writer) => {
                    permit.retire_writer_preserving_pending(writer)
                }
            };
            match retired {
                Ok(()) => primary,
                Err(retirement_error) => {
                    append_custody_error(primary, "confirmed writer retirement", &retirement_error)
                }
            }
        }
        Err(absence_error) => {
            // The name may still be present. Close the actual writer FD while
            // preserving the pending physical charge.
            let primary = append_custody_error(primary, "staging absence", &absence_error);
            let writer = match custody {
                AdmittedWriterCustody::Confirmed(writer)
                | AdmittedWriterCustody::Unconfirmed(writer) => writer,
            };
            match permit.retire_writer_preserving_pending(writer) {
                Ok(()) => primary,
                Err(retirement_error) => {
                    append_custody_error(primary, "pending writer retirement", &retirement_error)
                }
            }
        }
    }
}

fn append_custody_error(
    mut primary: CoreError,
    context: &str,
    additional: &CoreError,
) -> CoreError {
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::Typed { message, .. }
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. } => message,
    };
    let suffix = format!("; additionally {context}: {additional}");
    message.push_str(&suffix);
    primary
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
    reconcile_owned_staging_directory_with_deletion_v1(
        staging_dir,
        owner,
        parent_sync,
        ReconcileDeletionV1::Unqualified,
    )
}

pub(super) fn reconcile_owned_staging_directory_admitted_v1(
    staging_dir: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
    admission: &dyn DurableControlFileAdmissionPort,
) -> Result<(), CoreError> {
    reconcile_owned_staging_directory_with_deletion_v1(
        staging_dir,
        owner,
        parent_sync,
        ReconcileDeletionV1::Qualified(admission),
    )
}

#[derive(Clone, Copy)]
enum ReconcileDeletionV1<'a> {
    Unqualified,
    Qualified(&'a dyn DurableControlFileAdmissionPort),
}

impl ReconcileDeletionV1<'_> {
    fn delete(self, path: &Path, owner: &str, action: &str) -> Result<(), CoreError> {
        match self {
            Self::Unqualified => fs::remove_file(path).map_err(|error| {
                CoreError::Storage(format!("{owner}: {action} {}: {error}", path.display()))
            }),
            Self::Qualified(admission) => admission.delete_file_and_sync(path),
        }
    }

    fn requires_parent_sync(self) -> bool {
        matches!(self, Self::Unqualified)
    }
}

fn reconcile_owned_staging_directory_with_deletion_v1(
    staging_dir: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
    deletion: ReconcileDeletionV1<'_>,
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
        deletion.delete(&path, owner, "remove abandoned staging file")?;
        removed = true;
    }
    if removed && deletion.requires_parent_sync() {
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
    reconcile_legacy_atomic_temporaries_with_deletion_v1(
        directory,
        target_suffix,
        owner,
        parent_sync,
        ReconcileDeletionV1::Unqualified,
    )
}

pub(super) fn reconcile_legacy_atomic_temporaries_admitted_v1(
    directory: &Path,
    target_suffix: &str,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
    admission: &dyn DurableControlFileAdmissionPort,
) -> Result<(), CoreError> {
    reconcile_legacy_atomic_temporaries_with_deletion_v1(
        directory,
        target_suffix,
        owner,
        parent_sync,
        ReconcileDeletionV1::Qualified(admission),
    )
}

fn reconcile_legacy_atomic_temporaries_with_deletion_v1(
    directory: &Path,
    target_suffix: &str,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
    deletion: ReconcileDeletionV1<'_>,
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
        deletion.delete(&path, owner, "remove abandoned legacy temporary")?;
        removed = true;
    }
    if removed && deletion.requires_parent_sync() {
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

#[cfg(test)]
mod admitted_tests {
    use std::fs::File;
    use std::io;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use quanta_index_core::CoreError;
    use quanta_index_core::publication_admission::{
        DurableControlFileAdmissionPort, PublicationOutputPermit,
    };

    use super::{
        AtomicFileWriteOutcomeV1, FsParentDirectorySyncPort, ParentDirectorySyncPort,
        atomic_create_file_from_staging_admitted_v1, atomic_replace_file_from_staging_admitted_v1,
    };
    use crate::readiness::activation_catalog::ActivationCatalog;
    use crate::readiness::auxiliary_store::{
        AuxiliaryAuthorityStore, NoActiveSearchCorpusPinsV1, ScriptedIndexBytesV1,
    };
    use crate::search_corpus_lifecycle::SearchCorpusPairMutationCoordinator;
    use crate::search_corpus_retention::SearchCorpusHistoryRetentionPolicyV1;

    fn require(condition: bool, message: &'static str) -> io::Result<()> {
        if condition {
            Ok(())
        } else {
            Err(io::Error::other(message))
        }
    }

    fn require_eq<T: std::fmt::Debug + PartialEq>(actual: &T, expected: &T) -> io::Result<()> {
        if actual == expected {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "actual={actual:?}, expected={expected:?}"
            )))
        }
    }

    fn recorded(steps: &Mutex<Vec<&'static str>>) -> io::Result<Vec<&'static str>> {
        steps
            .lock()
            .map(|steps| steps.clone())
            .map_err(|error| io::Error::other(format!("fixture steps lock: {error}")))
    }

    fn cleanup_deletions(admission: &CleanupAdmission) -> io::Result<Vec<PathBuf>> {
        admission
            .deleted
            .lock()
            .map(|deleted| deleted.clone())
            .map_err(|error| io::Error::other(format!("fixture delete log: {error}")))
    }

    fn pending_cleanup(admission: &CleanupAdmission) -> io::Result<Option<PathBuf>> {
        admission
            .pending
            .lock()
            .map(|pending| pending.clone())
            .map_err(|error| io::Error::other(format!("fixture pending lock: {error}")))
    }

    fn fixture_retention() -> Result<SearchCorpusHistoryRetentionPolicyV1, CoreError> {
        SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 64, 64 * 1024 * 1024)
    }

    #[derive(Clone)]
    struct RecordingAdmission {
        steps: Arc<Mutex<Vec<&'static str>>>,
        expected: Option<Vec<u8>>,
        expected_len: u64,
        refuse: bool,
        fail_commit: bool,
    }

    impl DurableControlFileAdmissionPort for RecordingAdmission {
        fn begin_reader_open(
            &self,
            _path: &Path,
        ) -> Result<
            Box<dyn quanta_index_core::publication_admission::PublicationControlOpenTicket>,
            CoreError,
        > {
            Err(CoreError::InvalidContract(
                "fixture did not configure reader admission".into(),
            ))
        }

        fn delete_file_and_sync(&self, _path: &Path) -> Result<(), CoreError> {
            Err(CoreError::InvalidContract(
                "fixture did not configure deletion".into(),
            ))
        }

        fn reconcile_deletions(&self) -> Result<(), CoreError> {
            Ok(())
        }

        fn reserve_output(
            &self,
            max_encoded_bytes: u64,
        ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
            if max_encoded_bytes != self.expected_len {
                return Err(CoreError::InvalidContract(
                    "unexpected fixture bound".into(),
                ));
            }
            self.steps
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture steps lock: {error}")))?
                .push("reserve");
            if self.refuse {
                return Err(CoreError::InvalidContract("fixture refusal".into()));
            }
            Ok(Box::new(RecordingPermit {
                steps: Arc::clone(&self.steps),
                expected: self.expected.clone(),
                expected_len: self.expected_len,
                target: None,
                fail_commit: self.fail_commit,
            }))
        }
    }

    #[derive(Default)]
    struct CleanupAdmission {
        deleted: Mutex<Vec<PathBuf>>,
        pending: Mutex<Option<PathBuf>>,
        fail_after_first_unlink: AtomicBool,
    }

    impl DurableControlFileAdmissionPort for CleanupAdmission {
        fn begin_reader_open(
            &self,
            _path: &Path,
        ) -> Result<
            Box<dyn quanta_index_core::publication_admission::PublicationControlOpenTicket>,
            CoreError,
        > {
            Err(CoreError::InvalidContract(
                "fixture did not configure reader admission".into(),
            ))
        }

        fn delete_file_and_sync(&self, path: &Path) -> Result<(), CoreError> {
            std::fs::remove_file(path).map_err(|error| {
                CoreError::Storage(format!(
                    "fixture admitted unlink {}: {error}",
                    path.display()
                ))
            })?;
            self.deleted
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture delete log: {error}")))?
                .push(path.to_path_buf());
            if self.fail_after_first_unlink.swap(false, Ordering::SeqCst) {
                *self.pending.lock().map_err(|error| {
                    CoreError::Storage(format!("fixture pending delete: {error}"))
                })? = Some(path.to_path_buf());
                return Err(CoreError::Storage(
                    "fixture parent sync failed after unlink".into(),
                ));
            }
            let parent = path
                .parent()
                .ok_or_else(|| CoreError::InvalidContract("fixture delete parent absent".into()))?;
            File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|error| CoreError::Storage(format!("fixture delete sync: {error}")))
        }

        fn reconcile_deletions(&self) -> Result<(), CoreError> {
            let mut pending = self
                .pending
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture pending lock: {error}")))?;
            if let Some(path) = pending.as_ref() {
                match std::fs::symlink_metadata(path) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Ok(_) => {
                        return Err(CoreError::InvalidContract(
                            "fixture pending delete path reappeared".into(),
                        ));
                    }
                    Err(error) => {
                        return Err(CoreError::Storage(format!(
                            "fixture pending path inspection: {error}"
                        )));
                    }
                }
                let parent = path.parent().ok_or_else(|| {
                    CoreError::InvalidContract("fixture pending parent absent".into())
                })?;
                File::open(parent)
                    .and_then(|file| file.sync_all())
                    .map_err(|error| {
                        CoreError::Storage(format!("fixture pending sync: {error}"))
                    })?;
                *pending = None;
            }
            drop(pending);
            Ok(())
        }

        fn reserve_output(
            &self,
            _max_encoded_bytes: u64,
        ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
            Err(CoreError::InvalidContract(
                "fixture unexpectedly reserved output".into(),
            ))
        }
    }

    struct RecordingPermit {
        steps: Arc<Mutex<Vec<&'static str>>>,
        expected: Option<Vec<u8>>,
        expected_len: u64,
        target: Option<PathBuf>,
        fail_commit: bool,
    }

    impl RecordingPermit {
        fn record(&self, step: &'static str) -> Result<(), CoreError> {
            self.steps
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture steps lock: {error}")))?
                .push(step);
            Ok(())
        }
    }

    impl PublicationOutputPermit for RecordingPermit {
        fn cancel_rejected_created(&mut self, writer: std::fs::File) -> Result<(), CoreError> {
            self.retire_writer_preserving_pending(writer)?;
            Err(CoreError::InvalidContract(
                "unqualified fixture cannot prove rejected birth cancellation".into(),
            ))
        }
        fn bind_output(&mut self, _staging: &Path, target: &Path) -> Result<(), CoreError> {
            self.target = Some(target.to_path_buf());
            self.record("bind")
        }
        fn creation_started(&mut self) -> Result<(), CoreError> {
            self.record("start")
        }
        fn created(&mut self, writer: &File, staging: &Path) -> Result<(), CoreError> {
            if !staging.is_file()
                || writer
                    .metadata()
                    .map_err(|error| CoreError::Storage(format!("fixture writer stat: {error}")))?
                    .len()
                    != 0
            {
                return Err(CoreError::InvalidContract(
                    "fixture writer was not new".into(),
                ));
            }
            self.record("created")
        }
        fn before_replace(&mut self) -> Result<(), CoreError> {
            self.record("before_replace")
        }
        fn committed(&mut self, writer: File, actual_encoded_bytes: u64) -> Result<(), CoreError> {
            let target = self
                .target
                .as_ref()
                .ok_or_else(|| CoreError::InvalidContract("fixture target absent".into()))?;
            let actual = std::fs::read(target)
                .map_err(|error| CoreError::Storage(format!("fixture target read: {error}")))?;
            if actual_encoded_bytes != self.expected_len
                || writer
                    .metadata()
                    .map_err(|error| CoreError::Storage(format!("fixture writer stat: {error}")))?
                    .len()
                    != self.expected_len
                || u64::try_from(actual.len()).map_err(|error| {
                    CoreError::InvalidContract(format!("fixture read width: {error}"))
                })? != self.expected_len
                || self
                    .expected
                    .as_ref()
                    .is_some_and(|expected| actual != *expected)
            {
                return Err(CoreError::InvalidContract(
                    "fixture committed bytes differ".into(),
                ));
            }
            self.record("commit")?;
            if self.fail_commit {
                Err(CoreError::Storage("fixture commit refusal".into()))
            } else {
                Ok(())
            }
        }
        fn retire_writer_preserving_pending(&mut self, _writer: File) -> Result<(), CoreError> {
            self.record("retire_pending")
        }
        fn rename_started(&mut self, _target: &Path) -> Result<(), CoreError> {
            Err(CoreError::InvalidContract("unexpected rename".into()))
        }
        fn renamed_and_synced(&mut self) -> Result<(), CoreError> {
            Err(CoreError::InvalidContract(
                "unexpected rename settlement".into(),
            ))
        }
        fn delete_file_and_sync(&mut self) -> Result<(), CoreError> {
            Err(CoreError::InvalidContract("unexpected deletion".into()))
        }
        fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError> {
            self.record("uncreated_absent")
        }
        fn confirmed_safe_absent_after_writer_close(
            &mut self,
            _writer: File,
        ) -> Result<(), CoreError> {
            self.record("writer_absent")
        }
    }

    #[derive(Debug)]
    struct FailTargetParentSync(PathBuf);

    impl ParentDirectorySyncPort for FailTargetParentSync {
        fn sync_parent(&self, parent: &Path) -> std::io::Result<()> {
            if parent == self.0 {
                return Err(std::io::Error::other("fixture target parent sync failure"));
            }
            FsParentDirectorySyncPort.sync_parent(parent)
        }
    }

    #[test]
    fn refusal_precedes_file_creation() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let staging = root.path().join("staging");
        std::fs::create_dir(&staging)?;
        let steps = Arc::new(Mutex::new(Vec::new()));
        let admission = RecordingAdmission {
            steps: Arc::clone(&steps),
            expected: Some(b"abc".to_vec()),
            expected_len: 3,
            refuse: true,
            fail_commit: false,
        };
        require(
            atomic_replace_file_from_staging_admitted_v1(
                &root.path().join("target"),
                b"abc",
                &staging,
                "fixture",
                &FsParentDirectorySyncPort,
                &admission,
            )
            .is_err(),
            "reservation refusal did not prevent creation",
        )?;
        require_eq(&recorded(&steps)?, &vec!["reserve"])?;
        require_eq(&std::fs::read_dir(&staging)?.count(), &0)?;
        Ok(())
    }

    #[test]
    fn admitted_first_birth_refuses_existing_target_without_clobber()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let staging = root.path().join("staging");
        std::fs::create_dir(&staging)?;
        let target = root.path().join("target");
        std::fs::write(&target, b"existing identity")?;
        let steps = Arc::new(Mutex::new(Vec::new()));
        let admission = RecordingAdmission {
            steps: Arc::clone(&steps),
            expected: Some(b"abc".to_vec()),
            expected_len: 3,
            refuse: false,
            fail_commit: false,
        };
        require(
            atomic_create_file_from_staging_admitted_v1(
                &target,
                b"abc",
                &staging,
                "fixture first birth",
                &FsParentDirectorySyncPort,
                &admission,
            )
            .is_err(),
            "exclusive rename replaced an existing identity",
        )?;
        require_eq(&std::fs::read(&target)?, &b"existing identity".to_vec())?;
        require_eq(&std::fs::read_dir(&staging)?.count(), &0)?;
        require_eq(
            &recorded(&steps)?,
            &vec![
                "reserve",
                "bind",
                "start",
                "created",
                "before_replace",
                "writer_absent",
            ],
        )?;
        Ok(())
    }

    #[test]
    fn qualified_activation_cleanup_uses_admitted_deletion_for_owned_entries()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let activations = root.path().join("activations");
        let _initial = ActivationCatalog::open_with_lifecycle_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
        )?;
        let staged = activations.join(".staging/scv1-root-file.tmp-7-1");
        let legacy = activations.join(".head--corpus.json.tmp-7-2");
        let unrelated = activations.join("unrelated.txt");
        std::fs::write(&staged, b"staged")?;
        std::fs::write(&legacy, b"legacy")?;
        std::fs::write(&unrelated, b"foreign")?;
        let admission = Arc::new(CleanupAdmission::default());
        let _reopened = ActivationCatalog::open_with_lifecycle_and_durable_admission_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
            admission.clone(),
        )?;
        require(!staged.exists(), "owned staging entry survived cleanup")?;
        require(!legacy.exists(), "owned legacy entry survived cleanup")?;
        require_eq(&std::fs::read(&unrelated)?, &b"foreign".to_vec())?;
        require_eq(&cleanup_deletions(&admission)?, &vec![staged, legacy])?;
        Ok(())
    }

    #[test]
    fn qualified_auxiliary_cleanup_uses_admitted_deletion_for_staging_and_legacy()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let authorities = root.path().join("authorities");
        let _initial = AuxiliaryAuthorityStore::open(&authorities, fixture_retention()?)?;
        let search_corpus = authorities.join("search-corpus");
        let staged = search_corpus.join(".staging/scv1-history.tmp-7-1");
        let pair = search_corpus.join("A".repeat(64));
        std::fs::create_dir(&pair)?;
        let legacy = pair.join(".history.cbor.tmp-7-2");
        let unrelated = pair.join("unrelated.txt");
        std::fs::write(&staged, b"staged")?;
        std::fs::write(&legacy, b"legacy")?;
        std::fs::write(&unrelated, b"foreign")?;
        let admission = Arc::new(CleanupAdmission::default());
        let _reopened = AuxiliaryAuthorityStore::open_with_lifecycle_and_durable_admission_v1(
            &authorities,
            fixture_retention()?,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(NoActiveSearchCorpusPinsV1),
            Arc::new(ScriptedIndexBytesV1),
            admission.clone(),
        )?;
        require(!staged.exists(), "auxiliary staging entry survived cleanup")?;
        require(!legacy.exists(), "auxiliary legacy entry survived cleanup")?;
        require_eq(&std::fs::read(&unrelated)?, &b"foreign".to_vec())?;
        require_eq(&cleanup_deletions(&admission)?, &vec![staged, legacy])?;
        Ok(())
    }

    #[test]
    fn qualified_activation_cleanup_reconciles_absent_pending_before_scan()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let activations = root.path().join("activations");
        let _initial = ActivationCatalog::open_with_lifecycle_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
        )?;
        let staged = activations.join(".staging/scv1-root-file.tmp-7-1");
        std::fs::write(&staged, b"staged")?;
        let admission = Arc::new(CleanupAdmission {
            fail_after_first_unlink: AtomicBool::new(true),
            ..CleanupAdmission::default()
        });
        let failure = ActivationCatalog::open_with_lifecycle_and_durable_admission_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
            admission.clone(),
        )
        .err()
        .ok_or_else(|| io::Error::other("post-unlink fault was swallowed"))?;
        require(
            matches!(failure, CoreError::Storage(message) if message == "fixture parent sync failed after unlink"),
            "post-unlink failure changed category or reason",
        )?;
        require(!staged.exists(), "fault did not remove the name")?;
        require_eq(&pending_cleanup(&admission)?, &Some(staged.clone()))?;
        let _retried = ActivationCatalog::open_with_lifecycle_and_durable_admission_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
            admission.clone(),
        )?;
        require_eq(&pending_cleanup(&admission)?, &None)?;
        require_eq(&cleanup_deletions(&admission)?, &vec![staged])?;
        Ok(())
    }

    #[test]
    fn qualified_activation_cleanup_refuses_foreign_and_symlink_entries()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let activations = root.path().join("activations");
        let _initial = ActivationCatalog::open_with_lifecycle_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
        )?;
        let admission = Arc::new(CleanupAdmission::default());
        let foreign = activations.join(".staging/foreign.txt");
        std::fs::write(&foreign, b"foreign")?;
        let foreign_error = ActivationCatalog::open_with_lifecycle_and_durable_admission_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
            admission.clone(),
        )
        .err()
        .ok_or_else(|| io::Error::other("foreign staging entry was accepted"))?;
        require(
            matches!(foreign_error, CoreError::Storage(message) if message.contains("foreign staging entry")),
            "foreign staging refusal changed category or reason",
        )?;
        require_eq(&std::fs::read(&foreign)?, &b"foreign".to_vec())?;
        std::fs::remove_file(&foreign)?;
        let target = root.path().join("outside");
        std::fs::write(&target, b"outside")?;
        let staged_link = activations.join(".staging/scv1-root-file.tmp-7-1");
        symlink(&target, &staged_link)?;
        let symlink_error = ActivationCatalog::open_with_lifecycle_and_durable_admission_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
            admission.clone(),
        )
        .err()
        .ok_or_else(|| io::Error::other("staging symlink was accepted"))?;
        require(
            matches!(symlink_error, CoreError::Storage(message) if message.contains("foreign staging entry")),
            "symlink refusal changed category or reason",
        )?;
        require_eq(&std::fs::read(&target)?, &b"outside".to_vec())?;
        require(
            std::fs::symlink_metadata(&staged_link)?
                .file_type()
                .is_symlink(),
            "foreign staging symlink was removed",
        )?;
        require_eq(&cleanup_deletions(&admission)?, &Vec::<PathBuf>::new())?;
        Ok(())
    }

    #[test]
    fn admitted_activation_incarnation_refuses_before_create_and_reopens_same_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let activations = root.path().join("activations");
        let incarnation = activations.join(".activation-root-incarnation-v1");
        let refused_steps = Arc::new(Mutex::new(Vec::new()));
        let refusing: Arc<dyn DurableControlFileAdmissionPort> = Arc::new(RecordingAdmission {
            steps: Arc::clone(&refused_steps),
            expected: None,
            expected_len: 16,
            refuse: true,
            fail_commit: false,
        });
        require(
            ActivationCatalog::open_with_lifecycle_and_durable_admission_v1(
                &activations,
                SearchCorpusPairMutationCoordinator::shared(),
                refusing,
            )
            .is_err(),
            "reservation refusal did not stop first birth",
        )?;
        require(
            !incarnation.exists(),
            "incarnation was created before reservation",
        )?;
        require_eq(&recorded(&refused_steps)?, &vec!["reserve"])?;
        require_eq(
            &std::fs::read_dir(activations.join(".staging"))?.count(),
            &0,
        )?;

        let steps = Arc::new(Mutex::new(Vec::new()));
        let admitting: Arc<dyn DurableControlFileAdmissionPort> = Arc::new(RecordingAdmission {
            steps: Arc::clone(&steps),
            expected: None,
            expected_len: 16,
            refuse: false,
            fail_commit: false,
        });
        let first = ActivationCatalog::open_with_lifecycle_and_durable_admission_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::clone(&admitting),
        )?;
        let first_bytes = std::fs::read(&incarnation)?;
        require_eq(&first_bytes.len(), &16)?;
        require(
            first_bytes.iter().any(|byte| *byte != 0),
            "incarnation was all zero",
        )?;
        require_eq(&first.root_incarnation_v1().to_vec(), &first_bytes)?;
        drop(first);
        let reopened = ActivationCatalog::open_with_lifecycle_and_durable_admission_v1(
            &activations,
            SearchCorpusPairMutationCoordinator::shared(),
            admitting,
        )?;
        require_eq(&reopened.root_incarnation_v1().to_vec(), &first_bytes)?;
        require_eq(&std::fs::read(&incarnation)?, &first_bytes)?;
        require_eq(
            &recorded(&steps)?,
            &vec![
                "reserve",
                "bind",
                "start",
                "created",
                "before_replace",
                "commit",
            ],
        )?;
        Ok(())
    }

    #[test]
    fn commit_consumes_writer_only_after_both_parent_syncs()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let staging = root.path().join("staging");
        std::fs::create_dir(&staging)?;
        let steps = Arc::new(Mutex::new(Vec::new()));
        let admission = RecordingAdmission {
            steps: Arc::clone(&steps),
            expected: Some(b"abc".to_vec()),
            expected_len: 3,
            refuse: false,
            fail_commit: false,
        };
        let outcome = atomic_replace_file_from_staging_admitted_v1(
            &root.path().join("target"),
            b"abc",
            &staging,
            "fixture",
            &FsParentDirectorySyncPort,
            &admission,
        )?;
        require(
            matches!(outcome, AtomicFileWriteOutcomeV1::Durable),
            "synced writer did not report durable",
        )?;
        require_eq(
            &recorded(&steps)?,
            &vec![
                "reserve",
                "bind",
                "start",
                "created",
                "before_replace",
                "commit",
            ],
        )?;
        Ok(())
    }

    #[test]
    fn failed_parent_sync_does_not_commit() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let staging = root.path().join("staging");
        std::fs::create_dir(&staging)?;
        let steps = Arc::new(Mutex::new(Vec::new()));
        let admission = RecordingAdmission {
            steps: Arc::clone(&steps),
            expected: Some(b"abc".to_vec()),
            expected_len: 3,
            refuse: false,
            fail_commit: false,
        };
        let outcome = atomic_replace_file_from_staging_admitted_v1(
            &root.path().join("target"),
            b"abc",
            &staging,
            "fixture",
            &FailTargetParentSync(root.path().to_path_buf()),
            &admission,
        )?;
        require(
            matches!(
                outcome,
                AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(_)
            ),
            "failed parent sync was reported durable",
        )?;
        let observed = recorded(&steps)?;
        require(
            !observed.contains(&"commit"),
            "uncertain writer was committed",
        )?;
        require(
            observed.contains(&"retire_pending"),
            "uncertain writer FD was not retired",
        )?;
        require_eq(
            &std::fs::read(root.path().join("target"))?,
            &b"abc".to_vec(),
        )?;
        Ok(())
    }

    #[test]
    fn failed_admission_commit_is_not_reported_durable() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let staging = root.path().join("staging");
        std::fs::create_dir(&staging)?;
        let steps = Arc::new(Mutex::new(Vec::new()));
        let admission = RecordingAdmission {
            steps: Arc::clone(&steps),
            expected: Some(b"abc".to_vec()),
            expected_len: 3,
            refuse: false,
            fail_commit: true,
        };
        let outcome = atomic_replace_file_from_staging_admitted_v1(
            &root.path().join("target"),
            b"abc",
            &staging,
            "fixture",
            &FsParentDirectorySyncPort,
            &admission,
        )?;
        require(
            matches!(
                outcome,
                AtomicFileWriteOutcomeV1::RenamedButAdmissionCommitFailed(_)
            ),
            "failed admission commit was reported durable",
        )?;
        require_eq(
            &std::fs::read(root.path().join("target"))?,
            &b"abc".to_vec(),
        )?;
        Ok(())
    }
}
