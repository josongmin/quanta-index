//! Physical file writes for a leased offline target before domain adapters open.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path};
use std::sync::Arc;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{CoreError, RequestBudgetV1};
use quanta_index_searchd::app::runtime::StateRootDirectoryV1;
use quanta_index_searchd::app::state_format::{
    StateMigrationFaultPointV1, StateMigrationFaultPort,
};
use rusqlite::ffi::quanta_capacity_vfs::close_owned_file;
use rustix::fs::{Mode, OFlags, RenameFlags, fsync, openat, renameat_with};
use sha2::{Digest as _, Sha256};

use crate::publication_file_capacity::PublicationFileCapacity;
use crate::publication_file_deletions::PublicationFileDeletions;

const COPY_CHUNK: usize = 16 * 1024;

fn contract(message: impl Into<String>) -> CoreError {
    CoreError::InvalidContract(format!("offline target file I/O: {}", message.into()))
}

fn storage(context: &str, error: impl Into<std::io::Error>) -> CoreError {
    let error = error.into();
    CoreError::storage_io(&format!("offline target file I/O {context}"), &error)
}

fn append_custody(mut primary: CoreError, context: &str, additional: &CoreError) -> CoreError {
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. }
        | CoreError::Typed { message, .. } => message,
    };
    let _written = write!(message, "; {context}: {additional}");
    primary
}

fn normal_components(path: &Path) -> Result<Vec<&std::ffi::OsStr>, CoreError> {
    let components = path
        .components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => Err(contract(
                "path must contain only normal relative components",
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty() {
        return Err(contract("empty relative file path"));
    }
    Ok(components)
}

/// All paths and pending outcomes use the already seeded target authority.
/// The caller must retain this value and its source lease until checked close.
pub(crate) struct OfflineTargetFileIo {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    root: StateRootDirectoryV1,
    budget: RequestBudgetV1,
}

impl OfflineTargetFileIo {
    pub(crate) fn new(
        binder: Arc<PublicationFileCapacity>,
        deletions: Arc<PublicationFileDeletions>,
        root: StateRootDirectoryV1,
        budget: RequestBudgetV1,
    ) -> Result<Self, CoreError> {
        if !Arc::ptr_eq(&binder, &deletions.binder()) {
            return Err(contract("deletion owner belongs to another binder"));
        }
        root.require_current_v1()?;
        binder.validate_current_state_root()?;
        let held = rustix::fs::fstat(&root).map_err(|error| storage("stat held root", error))?;
        let named = std::fs::metadata(binder.state_root())
            .map_err(|error| storage("stat binder root", error))?;
        let held_device = crate::publication_inventory::checked_device_id(held.st_dev)
            .map_err(|error| contract(format!("held root device exceeds u64: {error}")))?;
        let held_inode = held.st_ino;
        if held_device != named.dev() || held_inode != named.ino() {
            return Err(contract("held root FD differs from binder root"));
        }
        Ok(Self {
            binder,
            deletions,
            root,
            budget,
        })
    }

    fn close_file(&self, file: File, context: &str) -> Result<(), CoreError> {
        close_owned_file(file).map_err(|error| {
            self.binder.mark_close_unwitnessed();
            storage(context, error)
        })
    }

    fn close_with_primary(&self, file: File, primary: CoreError, context: &str) -> CoreError {
        match self.close_file(file, context) {
            Ok(()) => primary,
            Err(close) => append_custody(primary, context, &close),
        }
    }

    fn open_parent(
        &self,
        root: &impl std::os::fd::AsFd,
        relative: &Path,
    ) -> Result<(File, std::ffi::OsString), CoreError> {
        let components = normal_components(relative)?;
        let leaf = components
            .last()
            .ok_or_else(|| contract("file leaf absent"))?
            .to_os_string();
        let mut parent = File::from(
            root.as_fd()
                .try_clone_to_owned()
                .map_err(|error| storage("clone held root FD", error))?,
        );
        for name in components.iter().take(components.len().saturating_sub(1)) {
            let next = match openat(
                &parent,
                Path::new(name),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(opened) => File::from(opened),
                Err(error) => {
                    let primary = storage("walk nofollow parent", error);
                    return Err(self.close_with_primary(
                        parent,
                        primary,
                        "close source/target ancestor",
                    ));
                }
            };
            if let Err(primary) = self.close_file(parent, "close source/target ancestor") {
                return Err(self.close_with_primary(next, primary, "close rejected next ancestor"));
            }
            parent = next;
        }
        Ok((parent, leaf))
    }

    fn open_source(&self, root: &File, relative: &Path) -> Result<File, CoreError> {
        let (parent, leaf) = self.open_parent(root, relative)?;
        let opened = openat(
            &parent,
            Path::new(&leaf),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|error| storage("open nofollow source", error));
        let closed = self.close_file(parent, "close source parent");
        match (opened, closed) {
            (Ok(file), Ok(())) => Ok(file),
            (Ok(file), Err(primary)) => {
                Err(self.close_with_primary(file, primary, "close rejected source"))
            }
            (Err(primary), Ok(())) => Err(primary),
            (Err(primary), Err(close)) => {
                Err(append_custody(primary, "source parent close", &close))
            }
        }
    }

    pub(crate) fn ensure_relative_directories(&self, relative: &Path) -> Result<(), CoreError> {
        let _components = normal_components(relative)?;
        self.budget
            .checkpoint("offline target directory admission")?;
        self.root.require_current_v1()?;
        self.binder.ensure_private_parent_directories(
            &self
                .binder
                .state_root()
                .join(relative)
                .join(".qiv-parent-probe"),
        )
    }

    fn publish<F>(
        &self,
        relative: &Path,
        expected_bytes: u64,
        replace: bool,
        after_sync_fault: Option<&dyn StateMigrationFaultPort>,
        fill: F,
    ) -> Result<(), CoreError>
    where
        F: FnOnce(&mut File) -> Result<(), CoreError>,
    {
        let _components = normal_components(relative)?;
        self.budget.checkpoint("offline target file allowance")?;
        self.root.require_current_v1()?;
        let target = self.binder.state_root().join(relative);
        self.binder.ensure_private_parent_directories(&target)?;
        if !replace && self.binder.path_observation(&target)?.is_some() {
            return Err(contract("copy target already exists"));
        }
        let bound = expected_bytes.max(1);
        let parent = self.binder.reserve_publication_allowance(bound, 1)?;
        let mut mutation = self
            .binder
            .begin_create_with_allowance(&target, bound, parent)?;
        let temporary = mutation.allocate_private_temp_path()?;
        mutation.retain_control_create_for_recovery()?;
        mutation.creation_started()?;
        let name = temporary
            .file_name()
            .ok_or_else(|| contract("temp filename absent"))?;
        let created = openat(
            &self.root,
            Path::new(name),
            OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::CREATE | OFlags::EXCL,
            Mode::from_raw_mode(0o600),
        );
        let writer = match created {
            Ok(writer) => File::from(writer),
            Err(error) => {
                let primary = storage("create staged target", error);
                return match rustix::fs::statat(
                    &self.root,
                    Path::new(name),
                    rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
                ) {
                    Err(rustix::io::Errno::NOENT) => {
                        let proof = fsync(&self.root)
                            .map_err(|error| storage("sync uncreated temp parent", error))
                            .and_then(|()| mutation.confirmed_uncreated_absent_and_synced());
                        Err(match proof {
                            Ok(()) => primary,
                            Err(error) => append_custody(primary, "uncreated proof", &error),
                        })
                    }
                    Ok(_) => Err(primary),
                    Err(error) => Err(append_custody(
                        primary,
                        "stat failed birth",
                        &storage("stat failed temp", error),
                    )),
                };
            }
        };
        if let Err(primary) = mutation.created(&writer, &temporary) {
            return Err(match mutation.cancel_rejected_created_temp(writer) {
                Ok(()) => primary,
                Err(cleanup) => append_custody(primary, "rejected birth cleanup", &cleanup),
            });
        }
        let mut writer = Some(writer);
        let mut renamed = false;
        let result = (|| {
            fill(
                writer
                    .as_mut()
                    .ok_or_else(|| contract("writer FD absent"))?,
            )?;
            let actual = writer
                .as_ref()
                .ok_or_else(|| contract("writer FD absent"))?;
            let observed = actual
                .metadata()
                .map_err(|error| storage("stat target writer", error))?;
            if observed.len() != expected_bytes {
                return Err(contract("writer width differs from expected receipt"));
            }
            actual
                .sync_all()
                .map_err(|error| storage("sync target writer", error))?;
            mutation.before_replace(&target)?;
            let (parent, leaf) = self.open_parent(&self.root, relative)?;
            self.root.require_current_v1()?;
            let flags = if replace {
                RenameFlags::empty()
            } else {
                RenameFlags::NOREPLACE
            };
            let moved = renameat_with(
                &self.root,
                Path::new(name),
                &parent,
                Path::new(&leaf),
                flags,
            )
            .map_err(|error| storage("publish staged target", error));
            if moved.is_ok() {
                renamed = true;
            }
            let parent_sync = if moved.is_ok() {
                fsync(&parent).map_err(|error| storage("sync target parent", error))
            } else {
                Ok(())
            };
            let parent_close = self.close_file(parent, "close target parent");
            let transition = moved.and(parent_sync);
            match (transition, parent_close) {
                (Ok(()), Ok(())) => {}
                (Err(primary), Ok(())) | (Ok(()), Err(primary)) => return Err(primary),
                (Err(primary), Err(close)) => {
                    return Err(append_custody(primary, "target parent retirement", &close));
                }
            }
            fsync(&self.root).map_err(|error| storage("sync temporary parent", error))?;
            if let Some(fault) = after_sync_fault {
                fault.reach(StateMigrationFaultPointV1::BeforeCutoverRename)?;
            }
            let actual = writer
                .as_ref()
                .ok_or_else(|| contract("writer FD absent"))?;
            let _retained = mutation.settle_after_file_and_parent_sync(actual)?;
            let actual = writer.take().ok_or_else(|| contract("writer FD absent"))?;
            mutation.confirmed_safe_writer_fd_retired(actual)?;
            mutation.confirmed_safe_pin_retired()?;
            Ok(())
        })();
        if let Err(primary) = result {
            if let Some(actual) = writer.take() {
                let retirement = if renamed {
                    mutation.confirmed_safe_writer_fd_retired(actual)
                } else {
                    mutation.cancel_declared_temp(actual)
                };
                return match retirement {
                    Ok(()) => Err(primary),
                    Err(error) => Err(append_custody(primary, "writer cleanup", &error)),
                };
            }
            return Err(primary);
        }
        Ok(())
    }

    /// Copy a frozen regular source in bounded chunks and retire its original
    /// opened FD before any target namespace promotion.
    pub(crate) fn copy_object(
        &self,
        source_root: &File,
        relative: &Path,
        expected_bytes: u64,
        expected_sha256_hex: &str,
    ) -> Result<(), CoreError> {
        self.publish(relative, expected_bytes, false, None, |writer| {
            let mut source = self.open_source(source_root, relative)?;
            let copied = (|| {
                let before = source
                    .metadata()
                    .map_err(|error| storage("stat source", error))?;
                if !before.is_file() || before.len() != expected_bytes {
                    return Err(contract("source file type or frozen width changed"));
                }
                let mut hash = Sha256::new();
                let mut total = 0_u64;
                let mut chunk = [0_u8; COPY_CHUNK];
                loop {
                    self.budget.checkpoint("offline object copy chunk")?;
                    let count = source
                        .read(&mut chunk)
                        .map_err(|error| storage("read source", error))?;
                    if count == 0 {
                        break;
                    }
                    total = total
                        .checked_add(
                            u64::try_from(count)
                                .map_err(|error| contract(format!("chunk width: {error}")))?,
                        )
                        .ok_or_else(|| contract("copy width overflow"))?;
                    if total > expected_bytes {
                        return Err(contract("source exceeded frozen width"));
                    }
                    let bytes = chunk
                        .get(..count)
                        .ok_or_else(|| contract("source read exceeded its supplied buffer"))?;
                    writer
                        .write_all(bytes)
                        .map_err(|error| storage("write staged object", error))?;
                    hash.update(bytes);
                }
                if total != expected_bytes {
                    return Err(contract("source shorter than frozen width"));
                }
                let mut digest = String::with_capacity(64);
                for byte in hash.finalize() {
                    write!(&mut digest, "{byte:02x}")
                        .map_err(|error| contract(format!("format digest: {error}")))?;
                }
                if digest != expected_sha256_hex {
                    return Err(CoreError::Typed {
                        code: SearchPlaneErrorCodeV2::SearchTrackManifestDigestMismatch,
                        message: format!(
                            "offline source digest changed for {}",
                            relative.display()
                        ),
                    });
                }
                let after = source
                    .metadata()
                    .map_err(|error| storage("restat source", error))?;
                if after.dev() != before.dev()
                    || after.ino() != before.ino()
                    || after.len() != before.len()
                    || after.nlink() != before.nlink()
                {
                    return Err(contract("source identity or width changed while copying"));
                }
                Ok(())
            })();
            let closed = self.close_file(source, "close original source FD");
            match (copied, closed) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(primary), Ok(())) | (Ok(()), Err(primary)) => Err(primary),
                (Err(primary), Err(close)) => {
                    Err(append_custody(primary, "source FD retirement", &close))
                }
            }
        })
    }

    pub(crate) fn create_empty_file(&self, relative: &Path) -> Result<(), CoreError> {
        self.publish(relative, 0, false, None, |_writer| Ok(()))
    }

    /// Write already encoded metadata. A fault before the durable file sync
    /// leaves the real temporary under the original pending-create owner.
    pub(crate) fn write_bytes(
        &self,
        relative: &Path,
        bytes: &[u8],
        fault: Option<&dyn StateMigrationFaultPort>,
    ) -> Result<(), CoreError> {
        let width = u64::try_from(bytes.len())
            .map_err(|error| contract(format!("encoded width: {error}")))?;
        self.publish(relative, width, true, fault, |writer| {
            self.budget.checkpoint("offline metadata write")?;
            writer
                .write_all(bytes)
                .map_err(|error| storage("write staged metadata", error))?;
            if let Some(fault) = fault {
                fault.reach(StateMigrationFaultPointV1::BeforeManifestSync)?;
            }
            Ok(())
        })
    }

    /// Read a present target only through the binder's opened-reader ticket.
    /// Absence is an optional result; namespace and identity failures are not.
    pub(crate) fn read_bytes(
        &self,
        relative: &Path,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, CoreError> {
        let _components = normal_components(relative)?;
        self.budget.checkpoint("offline target read admission")?;
        self.root.require_current_v1()?;
        let path = self.binder.state_root().join(relative);
        let guard = match self.binder.begin_reader_open(&path) {
            Ok(guard) => guard,
            Err(CoreError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let (parent, leaf) = self.open_parent(&self.root, relative)?;
        let opened = openat(
            &parent,
            Path::new(&leaf),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|error| storage("open admitted target reader", error));
        let opened = match opened {
            Ok(opened) => opened,
            Err(primary) => {
                return Err(self.close_with_primary(
                    parent,
                    primary,
                    "close rejected reader parent",
                ));
            }
        };
        let lease = match guard.opened_owned(opened) {
            Ok(lease) => lease,
            Err(primary) => {
                return Err(self.close_with_primary(
                    parent,
                    primary,
                    "close rejected reader parent",
                ));
            }
        };
        let read = (|| {
            let file = lease.file()?;
            let before = file
                .metadata()
                .map_err(|error| storage("stat admitted reader", error))?;
            let limit = u64::try_from(max_bytes)
                .map_err(|error| contract(format!("reader limit exceeds u64: {error}")))?;
            if !before.is_file() || before.len() > limit {
                return Err(contract("admitted target exceeds configured read bound"));
            }
            let mut result = Vec::new();
            let mut input = file;
            let mut chunk = [0_u8; COPY_CHUNK];
            loop {
                self.budget.checkpoint("offline target read chunk")?;
                let count = input
                    .read(&mut chunk)
                    .map_err(|error| storage("read admitted target", error))?;
                if count == 0 {
                    break;
                }
                let next = result
                    .len()
                    .checked_add(count)
                    .ok_or_else(|| contract("admitted read width overflow"))?;
                if next > max_bytes {
                    return Err(contract("admitted target grew beyond read bound"));
                }
                let bytes = chunk
                    .get(..count)
                    .ok_or_else(|| contract("target read exceeded its supplied buffer"))?;
                result.extend_from_slice(bytes);
            }
            let after = file
                .metadata()
                .map_err(|error| storage("restat admitted reader", error))?;
            if result.len()
                != usize::try_from(before.len())
                    .map_err(|error| contract(format!("reader width exceeds usize: {error}")))?
                || before.dev() != after.dev()
                || before.ino() != after.ino()
                || before.len() != after.len()
                || before.nlink() != after.nlink()
            {
                return Err(contract("admitted target changed during read"));
            }
            Ok(result)
        })();
        let parent_retirement = self.close_file(parent, "close admitted reader parent");
        let lease_retirement = lease.retire();
        let read = match (read, parent_retirement) {
            (Ok(bytes), Ok(())) => Ok(bytes),
            (Err(primary), Ok(())) | (Ok(_), Err(primary)) => Err(primary),
            (Err(primary), Err(close)) => {
                Err(append_custody(primary, "reader parent retirement", &close))
            }
        };
        match (read, lease_retirement) {
            (Ok(bytes), Ok(())) => Ok(Some(bytes)),
            (Err(primary), Ok(())) | (Ok(_), Err(primary)) => Err(primary),
            (Err(primary), Err(close)) => {
                Err(append_custody(primary, "reader FD retirement", &close))
            }
        }
    }

    /// Sync an existing admitted regular file through its actual reader FD.
    pub(crate) fn sync_file(&self, relative: &Path) -> Result<u64, CoreError> {
        let _components = normal_components(relative)?;
        // Final durability and FD retirement must remain callable after the
        // publishing request is cancelled or its deadline expires.
        self.root.require_current_v1()?;
        let path = self.binder.state_root().join(relative);
        let guard = self.binder.begin_reader_open(&path)?;
        let (parent, leaf) = self.open_parent(&self.root, relative)?;
        let opened = openat(
            &parent,
            Path::new(&leaf),
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|error| storage("open sync target", error));
        let opened = match opened {
            Ok(opened) => opened,
            Err(primary) => {
                return Err(self.close_with_primary(parent, primary, "close sync target parent"));
            }
        };
        let lease = match guard.opened_owned(opened) {
            Ok(lease) => lease,
            Err(primary) => {
                return Err(self.close_with_primary(parent, primary, "close rejected sync parent"));
            }
        };
        let sync = (|| {
            let file = lease.file()?;
            let metadata = file
                .metadata()
                .map_err(|error| storage("stat sync target", error))?;
            file.sync_all()
                .map_err(|error| storage("sync target", error))?;
            fsync(&parent).map_err(|error| storage("sync target parent", error))?;
            Ok(metadata.len())
        })();
        let parent_retirement = self.close_file(parent, "close synced target parent");
        let retirement = lease.retire();
        let sync = match (sync, parent_retirement) {
            (Ok(bytes), Ok(())) => Ok(bytes),
            (Err(primary), Ok(())) | (Ok(_), Err(primary)) => Err(primary),
            (Err(primary), Err(close)) => {
                Err(append_custody(primary, "target parent retirement", &close))
            }
        };
        match (sync, retirement) {
            (Ok(bytes), Ok(())) => Ok(bytes),
            (Err(primary), Ok(())) | (Ok(_), Err(primary)) => Err(primary),
            (Err(primary), Err(close)) => {
                Err(append_custody(primary, "sync reader retirement", &close))
            }
        }
    }

    pub(crate) fn reconcile_pending(&self) -> Result<(), CoreError> {
        self.deletions.reconcile_retired_control_creates()
    }
}
