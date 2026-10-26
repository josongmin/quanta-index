//! History text file operations under the composition's physical authority.

use std::ffi::OsString;
use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd as _;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::publication_admission::{
    PublicationControlFileMutation, PublicationObjectReader,
};
use quanta_index_core::publication_capacity::PublicationAllowance;
use quanta_index_core::{
    CoreError, HistoryTextDirectoryEntry, HistoryTextFileAdmissionFactory, HistoryTextFileIoPort,
    HistoryTextPhysicalLock, RequestBudgetV1,
};
use quanta_index_searchd::app::runtime::StateRootDirectoryV1;
use rusqlite::ffi::quanta_capacity_vfs::close_owned_file;
use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, fstat, openat, renameat_with, statat,
};

use crate::publication_file_capacity::{FileMutation, PublicationFileCapacity, ReaderLease};
use crate::publication_file_deletions::PublicationFileDeletions;

fn contract(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("history text file I/O: {message}"))
}

fn storage_io(context: &str, error: impl Into<io::Error>) -> CoreError {
    let error = error.into();
    CoreError::storage_io(&format!("history text file I/O {context}"), &error)
}

fn io_error(error: CoreError) -> io::Error {
    match error {
        absent @ CoreError::NotFound(_) => io::Error::new(io::ErrorKind::NotFound, absent),
        storage @ CoreError::StorageIo { kind, .. } => io::Error::new(kind, storage),
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::Storage(_)) => io::Error::other(other),
    }
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
    *message = format!("{message}; {context}: {additional}");
    primary
}

fn record_interruption(
    latch: &Mutex<Option<CoreError>>,
    observed: &CoreError,
) -> Result<(), CoreError> {
    if !matches!(
        observed,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled
                | SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
            ..
        }
    ) {
        return Ok(());
    }
    let mut first = latch
        .lock()
        .map_err(|_poisoned| CoreError::Storage("history interruption latch poisoned".into()))?;
    if first.is_none() {
        *first = Some(observed.clone());
    }
    drop(first);
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Scope {
    Read,
    Publication,
    Retirement,
}

struct Shared {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    held_root: Arc<StateRootDirectoryV1>,
    root: PathBuf,
    publication_bytes: u64,
    publication_files: u64,
    max_directory_entries: usize,
    #[cfg(test)]
    inject_next_close_eio: AtomicBool,
}

/// One same-ledger history authority; each publication receives one parent.
pub struct CanonicalHistoryTextFileAdmissionFactory {
    shared: Arc<Shared>,
}

impl CanonicalHistoryTextFileAdmissionFactory {
    pub fn new(
        deletions: Arc<PublicationFileDeletions>,
        held_root: StateRootDirectoryV1,
        publication_bytes: u64,
        publication_files: u64,
        max_directory_entries: usize,
    ) -> Result<Self, CoreError> {
        if publication_bytes == 0 || publication_files == 0 || max_directory_entries == 0 {
            return Err(contract("history limits must be positive"));
        }
        let binder = deletions.binder();
        binder.validate_current_state_root()?;
        held_root.require_current_v1()?;
        let held = fstat(&held_root).map_err(|error| storage_io("stat held root", error))?;
        let named = statat(
            rustix::fs::CWD,
            binder.state_root(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|error| storage_io("stat binder root", error))?;
        if (held.st_dev, held.st_ino) != (named.st_dev, named.st_ino) {
            return Err(contract("held root differs from binder root"));
        }
        Ok(Self {
            shared: Arc::new(Shared {
                root: binder.state_root().join("authorities").join("history-text"),
                binder,
                deletions,
                held_root: Arc::new(held_root),
                publication_bytes,
                publication_files,
                max_directory_entries,
                #[cfg(test)]
                inject_next_close_eio: AtomicBool::new(false),
            }),
        })
    }
}

impl HistoryTextFileAdmissionFactory for CanonicalHistoryTextFileAdmissionFactory {
    fn for_read(&self) -> Result<Arc<dyn HistoryTextFileIoPort>, CoreError> {
        self.shared.validate_root()?;
        Ok(Arc::new(HistoryIo {
            shared: Arc::clone(&self.shared),
            scope: Scope::Read,
            parent: None,
            budget: None,
            interruption: None,
        }))
    }

    fn for_publication(
        &self,
        budget: &RequestBudgetV1,
    ) -> Result<Arc<dyn HistoryTextFileIoPort>, CoreError> {
        budget.checkpoint("history publication allowance")?;
        self.shared.validate_root()?;
        budget.checkpoint("history publication allowance")?;
        let parent = self.shared.binder.reserve_publication_allowance(
            self.shared.publication_bytes,
            self.shared.publication_files,
        )?;
        Ok(Arc::new(HistoryIo {
            shared: Arc::clone(&self.shared),
            scope: Scope::Publication,
            parent: Some(parent),
            budget: Some(budget.clone()),
            interruption: Some(Arc::new(Mutex::new(None))),
        }))
    }

    fn for_retirement(&self) -> Result<Arc<dyn HistoryTextFileIoPort>, CoreError> {
        self.shared.validate_root()?;
        Ok(Arc::new(HistoryIo {
            shared: Arc::clone(&self.shared),
            scope: Scope::Retirement,
            parent: None,
            budget: None,
            interruption: None,
        }))
    }
}

struct HistoryIo {
    shared: Arc<Shared>,
    scope: Scope,
    parent: Option<Arc<PublicationAllowance>>,
    budget: Option<RequestBudgetV1>,
    interruption: Option<Arc<Mutex<Option<CoreError>>>>,
}

impl HistoryIo {
    fn checkpoint_new_work(&self, stage: &'static str) -> Result<(), CoreError> {
        let Some(budget) = self.budget.as_ref() else {
            return Ok(());
        };
        if let Err(observed) = budget.checkpoint(stage) {
            if let Some(latch) = self.interruption.as_ref() {
                record_interruption(latch, &observed)?;
            }
            return Err(observed);
        }
        Ok(())
    }

    fn require_publication(&self) -> Result<Arc<PublicationAllowance>, CoreError> {
        if self.scope != Scope::Publication {
            return Err(contract("scope cannot create or grow history files"));
        }
        let parent = self
            .parent
            .as_ref()
            .ok_or_else(|| contract("publication parent missing"))?;
        self.checkpoint_new_work("history publication work")?;
        self.shared.binder.validate_held_allowance(parent)?;
        Ok(Arc::clone(parent))
    }

    fn require_mutation(&self) -> Result<(), CoreError> {
        if self.scope == Scope::Read {
            return Err(contract("read scope cannot mutate history files"));
        }
        self.shared.validate_root()
    }

    fn metadata_anchored(&self, path: &Path) -> Result<Option<Metadata>, CoreError> {
        self.shared.require_path(path)?;
        self.shared.deletions.reconcile_retired_control_creates()?;
        self.shared.recover_visible(path)?;
        let entry = self.shared.open_entry(path)?;
        let Some(entry) = entry else {
            return Ok(None);
        };
        let metadata = entry
            .metadata()
            .map_err(|error| storage_io("history metadata", error));
        let closed = self.shared.close(entry, "close metadata observer");
        match (metadata, closed) {
            (Ok(metadata), Ok(())) => Ok(Some(metadata)),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(close)) => Err(append_custody(error, "observer close", &close)),
        }
    }

    fn reader_anchored(
        &self,
        path: &Path,
    ) -> Result<Option<Box<dyn PublicationObjectReader>>, CoreError> {
        self.shared.require_path(path)?;
        self.shared.deletions.reconcile_retired_control_creates()?;
        self.shared.recover_visible(path)?;
        let Some((parent, _leaf)) = self.shared.open_parent(path)? else {
            return Ok(None);
        };
        self.shared.close(parent, "close reader parent preflight")?;
        if self.shared.binder.path_observation(path)?.is_none() {
            return Ok(None);
        }
        let opening = self.shared.binder.begin_reader_open(path)?;
        let file = self
            .shared
            .open_entry(path)?
            .ok_or_else(|| contract("history reader disappeared after admission"))?;
        let checked = file
            .metadata()
            .map_err(|error| storage_io("stat history reader", error))
            .and_then(|metadata| {
                if metadata.is_file() {
                    Ok(())
                } else {
                    Err(contract("history reader is not a regular file"))
                }
            });
        if let Err(error) = checked {
            self.shared.close(file, "close rejected history reader")?;
            return Err(error);
        }
        Ok(Some(Box::new(HistoryReader(opening.opened_owned(file)?))))
    }

    fn ensure_directory_anchored(&self, path: &Path) -> Result<(), CoreError> {
        let _parent = self.require_publication()?;
        self.shared.require_path(path)?;
        self.checkpoint_new_work("history create directory")?;
        self.shared
            .binder
            .ensure_private_parent_directories(&path.join(".qiv-history-directory-anchor"))?;
        let directory = self
            .shared
            .open_entry(path)?
            .ok_or_else(|| contract("created history directory is absent"))?;
        let checked = directory
            .metadata()
            .map_err(|error| storage_io("stat ensured history directory", error))
            .and_then(|metadata| {
                if metadata.is_dir() {
                    Ok(())
                } else {
                    Err(contract("ensured history entry is not a directory"))
                }
            });
        let closed = self
            .shared
            .close(directory, "close ensured history directory");
        match (checked, closed) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(close)) => Err(append_custody(error, "directory close", &close)),
        }
    }

    fn sync_directory_anchored(&self, path: &Path) -> Result<(), CoreError> {
        self.shared.require_path(path)?;
        let directory = self
            .shared
            .open_entry(path)?
            .ok_or_else(|| CoreError::NotFound("history directory absent".into()))?;
        let synced = directory
            .metadata()
            .map_err(|error| storage_io("stat synced directory", error))
            .and_then(|metadata| {
                if !metadata.is_dir() {
                    return Err(contract("sync target is not a directory"));
                }
                rustix::fs::fsync(&directory)
                    .map_err(|error| storage_io("fsync history directory", error))
            });
        let closed = self.shared.close(directory, "close synced directory");
        match (synced, closed) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(close)) => Err(append_custody(error, "directory close", &close)),
        }
    }

    fn delete_file_anchored(&self, path: &Path) -> Result<(), CoreError> {
        self.require_mutation()?;
        self.shared.require_path(path)?;
        self.shared.deletions.reconcile_retired_control_creates()?;
        self.shared.recover_visible(path)?;
        self.shared.deletions.delete_file_and_sync(path)
    }

    fn link_file_anchored(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        let _parent = self.require_publication()?;
        self.shared.require_path(source)?;
        self.shared.require_path(target)?;
        self.shared.recover_visible(source)?;
        self.checkpoint_new_work("history link file")?;
        let mut ticket = self.shared.binder.begin_link(source, target)?;
        ticket.perform_and_sync()
    }

    fn move_directory_anchored(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        let _parent = self.require_publication()?;
        self.shared.require_path(source)?;
        self.shared.require_path(target)?;
        if self
            .shared
            .deletions
            .recover_directory_move_if_pending(source, target)?
        {
            return Ok(());
        }
        let mut entries = 0;
        self.recover_tree(source, &mut entries)?;
        self.checkpoint_new_work("history move directory")?;
        let mut ticket = self.shared.deletions.begin_directory_move(source, target)?;
        ticket.perform_and_sync()
    }

    fn recover_tree(&self, path: &Path, count: &mut usize) -> Result<(), CoreError> {
        self.checkpoint_new_work("history tree traversal")?;
        for entry in self.list_anchored(path)? {
            self.checkpoint_new_work("history tree entry")?;
            *count = count
                .checked_add(1)
                .ok_or_else(|| contract("history tree entry count overflow"))?;
            if *count > self.shared.max_directory_entries {
                return Err(contract("history tree entry limit exceeded"));
            }
            if entry.metadata.is_dir() {
                self.recover_tree(&path.join(&entry.name), count)?;
            }
        }
        Ok(())
    }

    fn remove_tree_anchored(&self, path: &Path) -> Result<Option<u64>, CoreError> {
        self.require_mutation()?;
        self.shared.require_path(path)?;
        if let Some(bytes) = self
            .shared
            .deletions
            .recover_directory_delete_if_pending(path)?
        {
            return Ok(Some(bytes));
        }
        let Some(metadata) = self.metadata_anchored(path)? else {
            return Ok(None);
        };
        if !metadata.is_dir() {
            return Err(contract("history tree target is not a directory"));
        }
        let mut entries = 0;
        self.recover_tree(path, &mut entries)?;
        let mut ticket = self.shared.deletions.begin_directory_delete(path)?;
        let bytes = ticket.planned_file_bytes()?;
        ticket.perform_and_sync()?;
        Ok(Some(bytes))
    }

    fn acquire_lock_anchored(
        &self,
        path: &Path,
        blocking: bool,
    ) -> Result<Box<dyn HistoryTextPhysicalLock>, CoreError> {
        self.checkpoint_new_work("history acquire lock")?;
        self.shared.require_path(path)?;
        if self.metadata_anchored(path)?.is_none() {
            let writer = self.create_writer_anchored(path)?;
            writer.sync_and_settle()?;
        }
        let opening = self.shared.binder.begin_reader_open(path)?;
        let file = self
            .shared
            .open_entry(path)?
            .ok_or_else(|| contract("history lock disappeared after admission"))?;
        let reader = opening.opened_owned(file)?;
        let operation = if blocking {
            rustix::fs::FlockOperation::LockExclusive
        } else {
            rustix::fs::FlockOperation::NonBlockingLockExclusive
        };
        if let Err(error) = rustix::fs::flock(reader.file()?, operation) {
            let primary = storage_io("lock history original FD", error);
            return Err(match reader.retire() {
                Ok(()) => primary,
                Err(close) => append_custody(primary, "lock FD retirement", &close),
            });
        }
        Ok(Box::new(HistoryLock(Mutex::new(Some(reader)))))
    }

    fn create_writer_anchored(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError> {
        let parent = self.require_publication()?;
        self.shared.require_path(path)?;
        self.checkpoint_new_work("history visible writer admission")?;
        self.shared.binder.ensure_private_parent_directories(path)?;
        let mut mutation = self
            .shared
            .binder
            .begin_visible_create_with_allowance(path, parent)?;
        let (directory, leaf) = self
            .shared
            .open_parent(path)?
            .ok_or_else(|| CoreError::NotFound("history writer parent disappeared".into()))?;
        mutation.creation_started()?;
        let opened = openat(
            &directory,
            Path::new(&leaf),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(|error| storage_io("create visible history writer", error));
        let file = match opened {
            Ok(file) => file,
            Err(primary) => {
                let closed = self.shared.close(directory, "close failed writer parent");
                let primary = match closed {
                    Ok(()) => primary,
                    Err(close) => append_custody(primary, "parent close", &close),
                };
                if self.shared.binder.path_observation(path)?.is_none() {
                    return Err(match mutation.confirmed_uncreated_absent_and_synced() {
                        Ok(()) => primary,
                        Err(cleanup) => append_custody(primary, "no-FD create proof", &cleanup),
                    });
                }
                return Err(primary);
            }
        };
        let registered = mutation.created_visible(&file);
        let closed = self.shared.close(directory, "close created writer parent");
        if let Err(primary) = registered {
            self.shared.binder.mark_close_unwitnessed();
            let closed_writer = self.shared.close(file, "close rejected writer FD");
            let error = match closed_writer {
                Ok(()) => primary,
                Err(close) => append_custody(primary, "writer close", &close),
            };
            return Err(match closed {
                Ok(()) => error,
                Err(close) => append_custody(error, "parent close", &close),
            });
        }
        if let Err(primary) = closed {
            return Err(match mutation.cancel_visible_create(file) {
                Ok(()) => primary,
                Err(cleanup) => append_custody(primary, "visible create cleanup", &cleanup),
            });
        }
        Ok(Box::new(HistoryWriter {
            file: Some(file),
            mutation: Some(mutation),
            binder: Arc::clone(&self.shared.binder),
            failed: None,
            budget: self.budget.clone(),
            interruption: self.interruption.clone(),
        }))
    }

    fn staged_write(
        &self,
        path: &Path,
        width: u64,
        no_replace: bool,
        write_payload: impl FnOnce(&mut File) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let parent = self.require_publication()?;
        self.shared.require_path(path)?;
        self.checkpoint_new_work("history staged output admission")?;
        self.shared.binder.ensure_private_parent_directories(path)?;
        if no_replace && self.metadata_anchored(path)?.is_some() {
            return Err(CoreError::StorageIo {
                kind: io::ErrorKind::AlreadyExists,
                raw_os_error: None,
                message: "history copy target already exists".into(),
            });
        }
        let mut mutation =
            self.shared
                .binder
                .begin_create_with_allowance(path, width.max(1), parent)?;
        let temp = mutation.allocate_private_temp_path()?;
        mutation.retain_control_create_for_recovery()?;
        let temp_name = temp
            .file_name()
            .ok_or_else(|| contract("private history temp has no basename"))?;
        if temp.parent() != Some(self.shared.binder.state_root()) {
            return Err(contract("private history temp escapes leased root"));
        }
        mutation.creation_started()?;
        let opened = openat(
            self.shared.held_root.as_ref(),
            Path::new(temp_name),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map(File::from)
        .map_err(|error| storage_io("create private history temp", error));
        let mut file = match opened {
            Ok(file) => file,
            Err(primary) => {
                if self.shared.binder.path_observation(&temp)?.is_none() {
                    return Err(match mutation.confirmed_uncreated_absent_and_synced() {
                        Ok(()) => primary,
                        Err(cleanup) => append_custody(primary, "no-FD temp proof", &cleanup),
                    });
                }
                return Err(primary);
            }
        };
        if let Err(primary) = mutation.created(&file, &temp) {
            return Err(match mutation.cancel_rejected_created_temp(file) {
                Ok(()) => primary,
                Err(cleanup) => append_custody(primary, "rejected temp cleanup", &cleanup),
            });
        }
        let prepared = self
            .checkpoint_new_work("history staged payload write")
            .and_then(|()| write_payload(&mut file))
            .and_then(|()| {
                let length = file
                    .metadata()
                    .map_err(|error| storage_io("stat staged history payload", error))?
                    .len();
                if length != width {
                    return Err(contract("staged history payload length differs"));
                }
                file.sync_all()
                    .map_err(|error| storage_io("fsync staged history payload", error))
            })
            .and_then(|()| self.checkpoint_new_work("history staged publication"))
            .and_then(|()| mutation.before_replace(path));
        if let Err(primary) = prepared {
            return Err(match mutation.cancel_declared_temp(file) {
                Ok(()) => primary,
                Err(cleanup) => append_custody(primary, "staged temp cleanup", &cleanup),
            });
        }
        let destination = self.shared.open_parent(path).and_then(|parent| {
            parent.ok_or_else(|| CoreError::NotFound("history target parent disappeared".into()))
        });
        let (directory, leaf) = match destination {
            Ok(parent) => parent,
            Err(primary) => {
                return Err(match mutation.cancel_declared_temp(file) {
                    Ok(()) => primary,
                    Err(cleanup) => append_custody(primary, "staged temp cleanup", &cleanup),
                });
            }
        };
        let flags = if no_replace {
            RenameFlags::NOREPLACE
        } else {
            RenameFlags::empty()
        };
        let published = renameat_with(
            self.shared.held_root.as_ref(),
            Path::new(temp_name),
            &directory,
            Path::new(&leaf),
            flags,
        )
        .map_err(|error| storage_io("publish staged history file", error));
        let closed_parent = self.shared.close(directory, "close staged target parent");
        let published = match (published, closed_parent) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(error), Err(close)) => Err(append_custody(error, "parent close", &close)),
        };
        let synced = published.and_then(|()| {
            rustix::fs::fsync(self.shared.held_root.as_ref())
                .map_err(|error| storage_io("fsync private history temp parent", error))
        });
        let settled = synced.and_then(|()| {
            mutation
                .settle_after_file_and_parent_sync(&file)
                .map(|_retained| ())
        });
        let retired = mutation.confirmed_safe_writer_fd_retired(file);
        match (settled, retired) {
            (Ok(()), Ok(())) => {
                mutation.confirmed_safe_pin_retired()?;
                self.shared.binder.sweep_retired()
            }
            (Err(primary), Ok(())) => Err(primary),
            (Ok(()), Err(close)) => Err(close),
            (Err(primary), Err(close)) => {
                Err(append_custody(primary, "staged writer close", &close))
            }
        }
    }

    fn list_anchored(&self, path: &Path) -> Result<Vec<HistoryTextDirectoryEntry>, CoreError> {
        self.shared.require_path(path)?;
        self.shared.deletions.reconcile_retired_control_creates()?;
        self.shared.binder.validate_list_scope(path)?;
        let directory = self
            .shared
            .open_entry(path)?
            .ok_or_else(|| CoreError::NotFound("history directory absent".into()))?;
        let result = (|| {
            let initial = directory
                .metadata()
                .map_err(|error| storage_io("stat listed directory", error))?;
            if !initial.is_dir() {
                return Err(contract("listed history entry is not a directory"));
            }
            let mut entries = Dir::read_from(&directory)
                .map_err(|error| storage_io("read held history directory", error))?;
            let mut result = Vec::new();
            while let Some(entry) = entries.read() {
                self.checkpoint_new_work("history directory entry")?;
                let entry = entry.map_err(|error| storage_io("read history entry", error))?;
                let bytes = entry.file_name().to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                if bytes.is_empty() || bytes.contains(&b'/') || bytes.contains(&0) {
                    return Err(contract("history directory entry is invalid"));
                }
                if result.len() >= self.shared.max_directory_entries {
                    return Err(contract("history directory entry limit exceeded"));
                }
                let name = OsString::from_vec(bytes.to_vec());
                self.shared.recover_visible(&path.join(&name))?;
                let child = self
                    .shared
                    .open_entry(&path.join(&name))?
                    .ok_or_else(|| contract("listed history entry disappeared"))?;
                let metadata = child
                    .metadata()
                    .map_err(|error| storage_io("stat listed history child", error));
                let closed = self.shared.close(child, "close listed history child");
                let metadata = match (metadata, closed) {
                    (Ok(metadata), Ok(())) => metadata,
                    (Err(error), Ok(())) | (Ok(_), Err(error)) => return Err(error),
                    (Err(error), Err(close)) => {
                        return Err(append_custody(error, "entry close", &close));
                    }
                };
                result.push(HistoryTextDirectoryEntry { name, metadata });
            }
            let current = self
                .shared
                .open_entry(path)?
                .ok_or_else(|| contract("listed history directory disappeared"))?;
            let metadata = current
                .metadata()
                .map_err(|error| storage_io("stat current listed directory", error));
            let closed = self.shared.close(current, "close current listed directory");
            let metadata = match (metadata, closed) {
                (Ok(metadata), Ok(())) => metadata,
                (Err(error), Ok(())) | (Ok(_), Err(error)) => return Err(error),
                (Err(error), Err(close)) => {
                    return Err(append_custody(error, "directory close", &close));
                }
            };
            if metadata.dev() != initial.dev() || metadata.ino() != initial.ino() {
                return Err(contract("listed history directory was replaced"));
            }
            Ok(result)
        })();
        let closed = self
            .shared
            .close(directory, "close listed history directory");
        match (result, closed) {
            (Ok(entries), Ok(())) => Ok(entries),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(close)) => Err(append_custody(error, "directory close", &close)),
        }
    }
}

impl HistoryTextFileIoPort for HistoryIo {
    fn original_interruption(&self) -> Result<Option<CoreError>, CoreError> {
        let Some(latch) = self.interruption.as_ref() else {
            return Ok(None);
        };
        let first = latch.lock().map_err(|_poisoned| {
            CoreError::Storage("history interruption latch poisoned".into())
        })?;
        Ok(first.clone())
    }

    fn validate_root(&self, root: &Path) -> Result<(), CoreError> {
        if root.as_os_str() != self.shared.root.as_os_str() {
            return Err(contract("history root differs from held authority"));
        }
        self.shared.validate_root()
    }

    fn metadata(&self, path: &Path) -> Result<Option<Metadata>, CoreError> {
        self.metadata_anchored(path)
    }

    fn list_directory(&self, path: &Path) -> Result<Vec<HistoryTextDirectoryEntry>, CoreError> {
        self.list_anchored(path)
    }

    fn ensure_directory(&self, path: &Path) -> Result<(), CoreError> {
        self.ensure_directory_anchored(path)
    }

    fn open_reader(
        &self,
        path: &Path,
    ) -> Result<Option<Box<dyn PublicationObjectReader>>, CoreError> {
        self.reader_anchored(path)
    }

    fn create_writer(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError> {
        self.create_writer_anchored(path)
    }

    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
        let width = u64::try_from(bytes.len())
            .map_err(|error| CoreError::Storage(format!("history payload width: {error}")))?;
        self.staged_write(path, width, false, |file| {
            self.checkpoint_new_work("history atomic payload write")?;
            file.write_all(bytes)
                .map_err(|error| storage_io("write staged history bytes", error))
        })
    }

    fn delete_file(&self, path: &Path) -> Result<(), CoreError> {
        self.delete_file_anchored(path)
    }

    fn link_file(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        self.link_file_anchored(source, target)
    }

    fn copy_file(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        let _parent = self.require_publication()?;
        self.shared.require_path(source)?;
        self.shared.require_path(target)?;
        self.shared.recover_visible(source)?;
        let opening = self.shared.binder.begin_reader_open(source)?;
        let source_file = self
            .shared
            .open_entry(source)?
            .ok_or_else(|| contract("history copy source disappeared"))?;
        let reader = opening.opened_owned(source_file)?;
        let width = reader
            .file()?
            .metadata()
            .map_err(|error| storage_io("stat admitted history copy source", error))?
            .len();
        let copied = self.staged_write(target, width, true, |target_file| {
            let mut source_file = reader.file()?;
            let _position = source_file
                .seek(SeekFrom::Start(0))
                .map_err(|error| storage_io("seek history copy source", error))?;
            let mut actual = 0_u64;
            let mut chunk = [0_u8; 16 * 1024];
            loop {
                self.checkpoint_new_work("history copy chunk")?;
                let read = source_file
                    .read(&mut chunk)
                    .map_err(|error| storage_io("read history copy bytes", error))?;
                if read == 0 {
                    break;
                }
                self.checkpoint_new_work("history copy write")?;
                let payload = chunk
                    .get(..read)
                    .ok_or_else(|| contract("history copy chunk exceeds buffer"))?;
                target_file
                    .write_all(payload)
                    .map_err(|error| storage_io("write history copy bytes", error))?;
                actual = actual
                    .checked_add(
                        u64::try_from(read)
                            .map_err(|error| contract(&format!("history copy width: {error}")))?,
                    )
                    .ok_or_else(|| contract("history copy width overflow"))?;
            }
            if actual != width {
                return Err(contract("history copy source changed width"));
            }
            Ok(())
        });
        let retired = reader.retire();
        match (copied, retired) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(primary), Ok(())) | (Ok(()), Err(primary)) => Err(primary),
            (Err(primary), Err(close)) => Err(append_custody(primary, "copy reader close", &close)),
        }
    }

    fn move_directory_if_absent(&self, source: &Path, target: &Path) -> Result<(), CoreError> {
        self.move_directory_anchored(source, target)
    }

    fn recover_directory_move(&self, source: &Path, target: &Path) -> Result<bool, CoreError> {
        self.require_mutation()?;
        self.shared.require_path(source)?;
        self.shared.require_path(target)?;
        self.shared
            .deletions
            .recover_directory_move_if_pending(source, target)
    }

    fn recover_tree_delete(&self, path: &Path) -> Result<Option<u64>, CoreError> {
        self.require_mutation()?;
        self.shared.require_path(path)?;
        self.shared
            .deletions
            .recover_directory_delete_if_pending(path)
    }

    fn remove_tree(&self, path: &Path) -> Result<Option<u64>, CoreError> {
        self.remove_tree_anchored(path)
    }

    fn sync_directory(&self, path: &Path) -> Result<(), CoreError> {
        self.sync_directory_anchored(path)
    }

    fn acquire_lock(
        &self,
        path: &Path,
        blocking: bool,
    ) -> Result<Box<dyn HistoryTextPhysicalLock>, CoreError> {
        self.acquire_lock_anchored(path, blocking)
    }
}

impl Shared {
    fn recover_visible(&self, path: &Path) -> Result<(), CoreError> {
        self.require_path(path)?;
        if !self.binder.has_retired_safe_visible_create_pending(path)? {
            return Ok(());
        }
        let file = self
            .open_entry(path)?
            .ok_or_else(|| contract("retired visible writer pathname is absent"))?;
        let _retained = self.binder.recover_pending_with_fd(path, path, file)?;
        Ok(())
    }

    fn validate_root(&self) -> Result<(), CoreError> {
        self.binder.validate_current_state_root()?;
        self.held_root.require_current_v1()?;
        let held =
            fstat(self.held_root.as_ref()).map_err(|error| storage_io("stat held root", error))?;
        let named = statat(
            rustix::fs::CWD,
            self.binder.state_root(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|error| storage_io("stat named root", error))?;
        if (held.st_dev, held.st_ino) != (named.st_dev, named.st_ino) {
            return Err(contract("named root differs from held lease"));
        }
        Ok(())
    }

    fn require_path(&self, path: &Path) -> Result<(), CoreError> {
        if !path.is_absolute() {
            return Err(contract("history path must be absolute"));
        }
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_error| contract("history path escapes authority root"))?;
        let mut canonical = self.root.clone();
        for part in relative.components() {
            let Component::Normal(name) = part else {
                return Err(contract("history path contains a non-normal component"));
            };
            canonical.push(name);
        }
        if canonical.as_os_str() != path.as_os_str() {
            return Err(contract("history path is not canonical"));
        }
        self.validate_root()
    }

    fn close(&self, file: File, context: &str) -> Result<(), CoreError> {
        close_owned_file(file).map_err(|error| {
            self.binder.mark_close_unwitnessed();
            storage_io(context, error)
        })?;
        #[cfg(test)]
        if self.inject_next_close_eio.swap(false, Ordering::SeqCst) {
            self.binder.mark_close_unwitnessed();
            return Err(storage_io(
                context,
                io::Error::from_raw_os_error(rustix::io::Errno::IO.raw_os_error()),
            ));
        }
        Ok(())
    }

    fn open_parent(&self, path: &Path) -> Result<Option<(File, OsString)>, CoreError> {
        self.require_path(path)?;
        let relative = path
            .strip_prefix(self.binder.state_root())
            .map_err(|_error| contract("path escapes held state root"))?;
        let mut components = relative.components().peekable();
        let mut directory = File::from(
            self.held_root
                .as_fd()
                .try_clone_to_owned()
                .map_err(|error| storage_io("clone held root", error))?,
        );
        while let Some(part) = components.next() {
            let Component::Normal(name) = part else {
                let close = self.close(directory, "close rejected directory");
                return Err(match close {
                    Ok(()) => contract("non-normal path"),
                    Err(close) => {
                        append_custody(contract("non-normal path"), "directory close", &close)
                    }
                });
            };
            if components.peek().is_none() {
                return Ok(Some((directory, name.to_os_string())));
            }
            let child = openat(
                &directory,
                Path::new(name),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map(File::from);
            let closed = self.close(directory, "close traversed directory");
            directory = match (child, closed) {
                (Ok(child), Ok(())) => child,
                (Ok(child), Err(primary)) => {
                    return Err(match self.close(child, "close unaccepted directory") {
                        Ok(()) => primary,
                        Err(close) => append_custody(primary, "child close", &close),
                    });
                }
                (Err(rustix::io::Errno::NOENT), Ok(())) => return Ok(None),
                (Err(rustix::io::Errno::NOENT), Err(close)) => return Err(close),
                (Err(error), Ok(())) => return Err(storage_io("open held path component", error)),
                (Err(error), Err(close)) => {
                    return Err(append_custody(
                        storage_io("open held path component", error),
                        "directory close",
                        &close,
                    ));
                }
            };
        }
        self.close(directory, "close empty path directory")?;
        Err(contract("history path has no leaf"))
    }

    fn open_entry(&self, path: &Path) -> Result<Option<File>, CoreError> {
        let Some((parent, leaf)) = self.open_parent(path)? else {
            return Ok(None);
        };
        let found = (|| -> Result<Option<File>, CoreError> {
            match statat(&parent, Path::new(&leaf), AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => {
                    let kind = FileType::from_raw_mode(stat.st_mode);
                    if !matches!(kind, FileType::Directory | FileType::RegularFile)
                        || stat.st_uid != rustix::process::geteuid().as_raw()
                        || stat.st_mode & 0o022 != 0
                    {
                        Err(contract("entry type, owner, or mode invalid"))
                    } else {
                        let mut flags =
                            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
                        if kind == FileType::Directory {
                            flags |= OFlags::DIRECTORY;
                        }
                        let file = File::from(
                            openat(&parent, Path::new(&leaf), flags, Mode::empty()).map_err(
                                |error| storage_io("open anchored history entry", error),
                            )?,
                        );
                        let checked = file
                            .metadata()
                            .map_err(|error| storage_io("fstat history entry", error))
                            .and_then(|metadata| {
                                let expected =
                                    crate::publication_inventory::checked_device_id(stat.st_dev)
                                        .map_err(|error| {
                                            CoreError::Storage(format!(
                                                "history device conversion: {error}"
                                            ))
                                        })?;
                                if metadata.dev() != expected || metadata.ino() != stat.st_ino {
                                    Err(contract("history entry changed during open"))
                                } else {
                                    Ok(())
                                }
                            });
                        if let Err(primary) = checked {
                            Err(match self.close(file, "close rejected history entry") {
                                Ok(()) => primary,
                                Err(close) => append_custody(primary, "entry close", &close),
                            })
                        } else {
                            Ok(Some(file))
                        }
                    }
                }
                Err(rustix::io::Errno::NOENT) => Ok(None),
                Err(error) => Err(storage_io("stat anchored history entry", error)),
            }
        })();
        let closed = self.close(parent, "close history entry parent");
        match (found, closed) {
            (Ok(Some(file)), Ok(())) => Ok(Some(file)),
            (Ok(Some(file)), Err(primary)) => {
                Err(match self.close(file, "close unaccepted history entry") {
                    Ok(()) => primary,
                    Err(close) => append_custody(primary, "entry close", &close),
                })
            }
            (Ok(None), Ok(())) => Ok(None),
            (Err(error), Ok(())) | (Ok(None), Err(error)) => Err(error),
            (Err(primary), Err(close)) => {
                Err(append_custody(primary, "entry parent close", &close))
            }
        }
    }
}

struct HistoryReader(ReaderLease);

struct HistoryLock(Mutex<Option<ReaderLease>>);

impl HistoryTextPhysicalLock for HistoryLock {
    fn retire(self: Box<Self>) -> Result<(), CoreError> {
        let mut this = *self;
        let holder = match this.0.get_mut() {
            Ok(holder) => holder,
            Err(poisoned) => poisoned.into_inner(),
        };
        let reader = holder
            .take()
            .ok_or_else(|| contract("history lock already retired"))?;
        reader.retire()
    }
}

impl Drop for HistoryLock {
    fn drop(&mut self) {
        let holder = match self.0.get_mut() {
            Ok(holder) => holder,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(reader) = holder.take() {
            let _retired = reader.retire();
        }
    }
}

struct HistoryWriter {
    file: Option<File>,
    mutation: Option<FileMutation>,
    binder: Arc<PublicationFileCapacity>,
    failed: Option<CoreError>,
    budget: Option<RequestBudgetV1>,
    interruption: Option<Arc<Mutex<Option<CoreError>>>>,
}

impl HistoryWriter {
    fn file(&mut self) -> io::Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("history writer FD retired"))
    }

    fn mutation(&mut self) -> io::Result<&mut FileMutation> {
        self.mutation
            .as_mut()
            .ok_or_else(|| io::Error::other("history writer mutation retired"))
    }
}

impl Read for HistoryWriter {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file()?.read(buffer)
    }
}

impl Write for HistoryWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if let Some(failed) = &self.failed {
            return Err(io_error(failed.clone()));
        }
        if let Some(budget) = &self.budget
            && let Err(error) = budget.checkpoint("history writer write")
        {
            let recorded = self
                .interruption
                .as_ref()
                .map_or(Ok(()), |latch| record_interruption(latch, &error));
            let failure = match recorded {
                Ok(()) => error,
                Err(poisoned) => poisoned,
            };
            self.failed = Some(failure.clone());
            return Err(io_error(failure));
        }
        let position = match self.file()?.stream_position() {
            Ok(position) => position,
            Err(error) => {
                self.failed = Some(CoreError::storage_io("position history writer", &error));
                return Err(error);
            }
        };
        let width = match u64::try_from(buffer.len()) {
            Ok(width) => width,
            Err(error) => {
                let failed = contract(&format!("history write width conversion: {error}"));
                self.failed = Some(failed.clone());
                return Err(io_error(failed));
            }
        };
        let Some(target) = position.checked_add(width) else {
            let failed = contract("history writer offset overflow");
            self.failed = Some(failed.clone());
            return Err(io_error(failed));
        };
        if let Err(error) = self.mutation()?.reserve_end(target) {
            self.failed = Some(error.clone());
            return Err(io_error(error));
        }
        match self.file()?.write(buffer) {
            Ok(0) if !buffer.is_empty() => {
                let failed = CoreError::StorageIo {
                    kind: io::ErrorKind::WriteZero,
                    raw_os_error: None,
                    message: "history original writer returned zero before payload completion"
                        .into(),
                };
                self.failed = Some(failed.clone());
                Err(io_error(failed))
            }
            Ok(written) => Ok(written),
            Err(error) => {
                self.failed = Some(CoreError::storage_io("write history original FD", &error));
                Err(error)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(failed) = &self.failed {
            return Err(io_error(failed.clone()));
        }
        match self.file()?.flush() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.failed = Some(CoreError::storage_io("flush history original FD", &error));
                Err(error)
            }
        }
    }
}

impl Seek for HistoryWriter {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file()?.seek(position)
    }
}

impl PublicationControlFileMutation for HistoryWriter {
    fn metadata(&self) -> Result<Metadata, CoreError> {
        self.file
            .as_ref()
            .ok_or_else(|| contract("history writer FD retired"))?
            .metadata()
            .map_err(|error| storage_io("writer metadata", error))
    }

    fn set_len(&mut self, length: u64) -> Result<(), CoreError> {
        if let Some(failed) = &self.failed {
            return Err(failed.clone());
        }
        if let Some(budget) = &self.budget
            && let Err(error) = budget.checkpoint("history writer resize")
        {
            let recorded = self
                .interruption
                .as_ref()
                .map_or(Ok(()), |latch| record_interruption(latch, &error));
            let failure = match recorded {
                Ok(()) => error,
                Err(poisoned) => poisoned,
            };
            self.failed = Some(failure.clone());
            return Err(failure);
        }
        if let Err(error) = self
            .mutation
            .as_mut()
            .ok_or_else(|| contract("history writer mutation retired"))?
            .reserve_end(length)
        {
            self.failed = Some(error.clone());
            return Err(error);
        }
        let result = self
            .file
            .as_ref()
            .ok_or_else(|| contract("history writer FD retired"))?
            .set_len(length)
            .map_err(|error| storage_io("resize history writer", error));
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }

    fn sync_and_settle(self: Box<Self>) -> Result<(), CoreError> {
        let mut this = *self;
        if let Some(primary) = this.failed.take() {
            let file = this
                .file
                .take()
                .ok_or_else(|| contract("failed history writer FD absent"))?;
            let close = this
                .mutation
                .as_mut()
                .ok_or_else(|| contract("failed history mutation absent"))?
                .confirmed_safe_writer_fd_retired(file);
            return Err(match close {
                Ok(()) => primary,
                Err(close) => append_custody(primary, "failed writer retirement", &close),
            });
        }
        let file = this
            .file
            .take()
            .ok_or_else(|| contract("history writer FD already retired"))?;
        let mutation = this
            .mutation
            .as_mut()
            .ok_or_else(|| contract("history writer mutation absent"))?;
        let settlement = mutation
            .settle_after_file_and_parent_sync(&file)
            .map(|_retained| ());
        let closed = mutation.confirmed_safe_writer_fd_retired(file);
        match (settlement, closed) {
            (Ok(()), Ok(())) => {
                mutation.confirmed_safe_pin_retired()?;
                this.binder.sweep_retired()
            }
            (Err(primary), Ok(())) => Err(primary),
            (Ok(()), Err(close)) => Err(close),
            (Err(primary), Err(close)) => Err(append_custody(primary, "writer retirement", &close)),
        }
    }

    fn retire_preserving_pending(self: Box<Self>) -> Result<(), CoreError> {
        let mut this = *self;
        let file = this
            .file
            .take()
            .ok_or_else(|| contract("history writer FD already retired"))?;
        this.mutation
            .as_mut()
            .ok_or_else(|| contract("history writer mutation absent"))?
            .confirmed_safe_writer_fd_retired(file)
    }
}

impl Drop for HistoryWriter {
    fn drop(&mut self) {
        let Some(file) = self.file.take() else {
            return;
        };
        if let Some(mutation) = self.mutation.as_mut() {
            if mutation.confirmed_safe_writer_fd_retired(file).is_err() {
                self.binder.mark_close_unwitnessed();
            }
        } else if close_owned_file(file).is_err() {
            self.binder.mark_close_unwitnessed();
        }
    }
}

impl Read for HistoryReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut file = self.0.file().map_err(io_error)?;
        file.read(buffer)
    }
}

impl Seek for HistoryReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let mut file = self.0.file().map_err(io_error)?;
        file.seek(position)
    }
}

impl PublicationObjectReader for HistoryReader {
    fn metadata(&self) -> Result<Metadata, CoreError> {
        self.0
            .file()?
            .metadata()
            .map_err(|error| storage_io("reader metadata", error))
    }

    fn sync_posix(&self) -> Result<(), CoreError> {
        rustix::fs::fsync(self.0.file()?).map_err(|error| storage_io("reader POSIX fsync", error))
    }

    fn retire(self: Box<Self>) -> Result<(), CoreError> {
        let Self(reader) = *self;
        reader.retire()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod product_tests;

#[cfg(test)]
mod long_pin_product_tests;
