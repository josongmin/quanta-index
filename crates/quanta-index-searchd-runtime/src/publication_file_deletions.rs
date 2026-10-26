//! One bounded owner for a possibly started publication file deletion.

use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;
use quanta_index_core::publication_capacity::InodeKey;

use crate::publication_file_capacity::{
    ControlCreateRecovery, DeleteTicket, InodeObservation, PublicationFileCapacity,
};

mod namespace;
pub use namespace::OwnedNamespaceTicket;

struct PendingDelete {
    path: PathBuf,
    source: InodeObservation,
    parent: InodeKey,
    ticket: DeleteTicket,
    unlink_attempted: bool,
    original_error: Option<CoreError>,
}

enum PendingOperation {
    Delete(Box<PendingDelete>),
    Namespace(Box<namespace::PendingNamespace>),
}

/// A fixed one-operation slot shared by work and retirement admissions.
/// Concurrent deletion of another path refuses until this exact ticket settles.
pub struct PublicationFileDeletions {
    binder: Arc<PublicationFileCapacity>,
    pending: Mutex<Option<PendingOperation>>,
}

impl PublicationFileDeletions {
    #[must_use]
    pub fn new(binder: Arc<PublicationFileCapacity>) -> Self {
        Self {
            binder,
            pending: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn binder(&self) -> Arc<PublicationFileCapacity> {
        Arc::clone(&self.binder)
    }

    fn canonical_path(&self, path: &Path) -> Result<PathBuf, CoreError> {
        if !path.is_absolute() {
            return Err(contract("delete pathname must be absolute"));
        }
        let relative = path
            .strip_prefix(self.binder.state_root())
            .map_err(|_error| contract("delete pathname escapes held root"))?;
        let mut canonical = self.binder.state_root().to_path_buf();
        let mut has_name = false;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(contract("delete pathname has non-normal component"));
            };
            canonical.push(name);
            has_name = true;
        }
        if !has_name || canonical.as_os_str() != path.as_os_str() {
            return Err(contract("delete pathname differs from held canonical path"));
        }
        Ok(canonical)
    }

    /// Execute or reconcile one authenticated unlink. A failed started ticket
    /// remains in this slot and a retry never issues a second unlink syscall.
    pub fn delete_file_and_sync(&self, path: &Path) -> Result<(), CoreError> {
        self.delete_matching_file_and_sync(path, None)
    }

    /// Remove only the committed output identified by the caller's owned FD.
    /// A replaced pathname must never authorize removal of the new inode.
    pub fn delete_owned_file_and_sync(
        &self,
        path: &Path,
        inode: InodeKey,
        bytes: u64,
    ) -> Result<(), CoreError> {
        self.delete_matching_file_and_sync(path, Some((inode, bytes)))
    }

    /// Reconcile the retained ticket even when its pathname is already absent
    /// from a subsequent directory scan. This never starts a fresh unlink.
    pub fn retry_pending_deletion(&self) -> Result<(), CoreError> {
        let mut pending = self.lock_pending()?;
        self.reconcile_pending(&mut pending)
    }

    /// Drain retired Control creates before reserving another allowance. The
    /// shared deletion slot stays held across temporary recovery and deletion.
    pub fn reconcile_retired_control_creates(&self) -> Result<(), CoreError> {
        let mut pending = self.lock_pending()?;
        // A cold Control probe must not consume another operation's outcome.
        // Its logical caller needs the original key and deletion receipt.
        if matches!(pending.as_ref(), Some(PendingOperation::Namespace(_))) {
            return Err(busy(pending.as_ref()));
        }
        self.reconcile_pending(&mut pending)?;
        while let Some(target) = self.binder.next_retired_control_create()? {
            match self.binder.recover_retired_control_create(&target)? {
                ControlCreateRecovery::RetainedTarget | ControlCreateRecovery::Absent => {}
                ControlCreateRecovery::RetainedTemporary { path, inode, bytes } => {
                    self.delete_matching_locked(&path, Some((inode, bytes)), &mut pending)?;
                }
            }
        }
        drop(pending);
        Ok(())
    }

    fn lock_pending(&self) -> Result<MutexGuard<'_, Option<PendingOperation>>, CoreError> {
        // Serialize the synchronous physical phase, including its fsync. A
        // live or uncertain logical ticket still refuses foreign operations
        // after this guard is acquired; mutex contention is not exhaustion.
        // Guarded code calls only the binder/Core, which cannot reenter this
        // owner, and does not await while holding the guard.
        self.pending.lock().map_err(|error| {
            CoreError::Storage(format!("publication file delete owner lock: {error}"))
        })
    }

    fn reconcile_pending(&self, pending: &mut Option<PendingOperation>) -> Result<(), CoreError> {
        let Some(prior) = pending.as_mut() else {
            return Ok(());
        };
        let PendingOperation::Delete(prior) = prior else {
            return namespace::reconcile_pending_namespace(self, pending).map(|_changed| ());
        };
        if !prior.unlink_attempted {
            return Err(contract("pending deletion lacks an unlink attempt"));
        }
        if prior.ticket.is_settled() {
            self.binder.sweep_retired()?;
            drop(pending.take());
            return Ok(());
        }
        match self.binder.path_observation(&prior.path)? {
            None => {
                prior.ticket.unlinked_and_synced()?;
                drop(pending.take());
                Ok(())
            }
            Some(current) if current == prior.source => {
                let (source, parent) = prior.ticket.source_metadata()?;
                if source.dev() != prior.source.inode.device
                    || source.ino() != prior.source.inode.inode
                    || source.len() != prior.source.bytes
                    || source.nlink() != prior.source.links
                    || parent.dev() != prior.parent.device
                    || parent.ino() != prior.parent.inode
                {
                    return Err(contract("pending deletion source or parent changed"));
                }
                prior.ticket.confirmed_unchanged_after_failure()?;
                let original = prior
                    .original_error
                    .take()
                    .ok_or_else(|| contract("pending unchanged deletion lacks original error"))?;
                drop(pending.take());
                Err(original)
            }
            Some(_) => Err(contract("pending deletion pathname changed")),
        }
    }

    fn delete_matching_file_and_sync(
        &self,
        path: &Path,
        expected: Option<(InodeKey, u64)>,
    ) -> Result<(), CoreError> {
        let path = self.canonical_path(path)?;
        let mut pending = self.lock_pending()?;
        self.delete_matching_locked(&path, expected, &mut pending)
    }

    fn delete_matching_locked(
        &self,
        path: &Path,
        expected: Option<(InodeKey, u64)>,
        pending: &mut Option<PendingOperation>,
    ) -> Result<(), CoreError> {
        let path = self.canonical_path(path)?;
        if let Some(PendingOperation::Delete(prior)) = pending.as_mut() {
            if prior.path != path {
                return Err(busy(pending.as_ref()));
            }
            if expected.is_some_and(|(inode, bytes)| {
                prior.source.inode != inode || prior.source.bytes != bytes
            }) {
                return Err(contract("pending deletion belongs to another output"));
            }
            self.reconcile_pending(pending)
        } else if pending.is_some() {
            Err(busy(pending.as_ref()))
        } else {
            let ticket = self.binder.begin_delete(&path)?;
            let (source, parent) = ticket.source_metadata()?;
            let observed = InodeObservation {
                inode: InodeKey {
                    device: source.dev(),
                    inode: source.ino(),
                },
                bytes: source.len(),
                links: source.nlink(),
            };
            if expected
                .is_some_and(|(inode, bytes)| observed.inode != inode || observed.bytes != bytes)
            {
                return Err(contract("delete pathname differs from owned output"));
            }
            if self.binder.path_observation(&path)? != Some(observed) {
                return Err(contract("delete source changed before owned attempt"));
            }
            *pending = Some(PendingOperation::Delete(Box::new(PendingDelete {
                path,
                source: observed,
                parent: InodeKey {
                    device: parent.dev(),
                    inode: parent.ino(),
                },
                ticket,
                unlink_attempted: true,
                original_error: None,
            })));
            let Some(PendingOperation::Delete(current)) = pending.as_mut() else {
                return Err(contract("installed delete ticket disappeared"));
            };
            match current.ticket.perform_and_sync() {
                Ok(()) => {
                    drop(pending.take());
                    Ok(())
                }
                Err(error) => {
                    if current.ticket.has_started() {
                        current.original_error = Some(error.clone());
                    } else {
                        drop(pending.take());
                    }
                    Err(error)
                }
            }
        }
    }
}

fn contract(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("publication file deletion: {message}"))
}

fn busy(pending: Option<&PendingOperation>) -> CoreError {
    let detail = match pending {
        None => "slot has no pending operation".into(),
        Some(PendingOperation::Delete(prior)) => format!(
            "delete path={}, unlink_attempted={}, settled={}, original_error={:?}",
            prior.path.display(),
            prior.unlink_attempted,
            prior.ticket.is_settled(),
            prior.original_error,
        ),
        Some(PendingOperation::Namespace(prior)) => prior.busy_detail(),
    };
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: format!(
            "publication file deletion: one pending operation owns the slot; {detail}"
        ),
    }
}

#[cfg(test)]
mod tests;
