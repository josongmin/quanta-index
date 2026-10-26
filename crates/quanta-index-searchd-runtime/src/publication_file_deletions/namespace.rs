//! Retain one original namespace ticket across uncertain rename, link or tree deletion.

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quanta_index_core::CoreError;
use quanta_index_core::publication_capacity::InodeKey;

use crate::publication_file_capacity::{
    DeleteTicket, DirectoryDeleteTicket, DirectoryMoveRecovery, DirectoryPathTicket,
    InodeObservation, PathRecovery, PathTicket,
};

use super::{PendingOperation, PublicationFileDeletions, busy, contract};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Link,
    MoveOverwrite,
    StagedMove,
    MoveCreate,
    DirectoryMoveOverwrite,
    DirectoryMoveCreate,
    DirectoryDelete,
}

enum NamespaceOperation {
    Path(PathTicket),
    DirectoryMove(DirectoryPathTicket),
    DirectoryDelete(DirectoryDeleteTicket),
}

impl NamespaceOperation {
    fn has_started(&self) -> bool {
        match self {
            Self::Path(ticket) => ticket.has_started(),
            Self::DirectoryMove(ticket) => ticket.has_started(),
            Self::DirectoryDelete(ticket) => ticket.has_started(),
        }
    }

    fn is_settled(&self) -> bool {
        match self {
            Self::Path(ticket) => ticket.is_settled(),
            Self::DirectoryMove(ticket) => ticket.is_settled(),
            Self::DirectoryDelete(ticket) => ticket.is_settled(),
        }
    }

    fn perform_and_sync(&mut self) -> Result<(), CoreError> {
        match self {
            Self::Path(ticket) => ticket.perform_and_sync(),
            Self::DirectoryMove(ticket) => ticket.perform_and_sync(),
            Self::DirectoryDelete(ticket) => ticket.perform_and_sync(&mut || Ok(())),
        }
    }

    fn reconcile_after_failure(&mut self) -> Result<bool, CoreError> {
        match self {
            Self::Path(ticket) => Ok(ticket.reconcile_after_failure()? == PathRecovery::Changed),
            Self::DirectoryMove(ticket) => {
                Ok(ticket.reconcile_after_failure()? == DirectoryMoveRecovery::Moved)
            }
            // The directory ticket owns a resumable cursor and the original
            // FDs. Its retry never repeats a recorded successful unlink.
            Self::DirectoryDelete(ticket) => {
                ticket.perform_and_sync(&mut || Ok(()))?;
                Ok(true)
            }
        }
    }

    fn change_started(&mut self) -> Result<(), CoreError> {
        match self {
            Self::Path(ticket) => ticket.change_started(),
            Self::DirectoryMove(_) | Self::DirectoryDelete(_) => {
                Err(contract("external callback requires a regular-file move"))
            }
        }
    }

    fn changed_and_synced(&mut self) -> Result<(), CoreError> {
        match self {
            Self::Path(ticket) => ticket.changed_and_synced(),
            Self::DirectoryMove(_) | Self::DirectoryDelete(_) => {
                Err(contract("external callback requires a regular-file move"))
            }
        }
    }
}

enum NamespaceOutcome {
    Unreported,
    Failed(CoreError),
}

pub(super) struct PendingNamespace {
    kind: Kind,
    from: PathBuf,
    to: Option<PathBuf>,
    token: Arc<()>,
    active: bool,
    operation: NamespaceOperation,
    outcome: NamespaceOutcome,
    staged_source: Option<StagedSource>,
    cleanup: Option<StagedCleanup>,
}

#[derive(Clone, Copy)]
struct StagedSource {
    observed: InodeObservation,
    parent: InodeKey,
}

struct StagedCleanup {
    ticket: Option<DeleteTicket>,
    failure: Option<CoreError>,
}

/// Exclusive handle to the one original namespace operation. A started Drop
/// leaves the ticket and its held FDs with the shared owner for reconciliation.
pub struct OwnedNamespaceTicket {
    owner: Arc<PublicationFileDeletions>,
    token: Option<Arc<()>>,
    completed: bool,
}

impl PublicationFileDeletions {
    /// Begin one canonical regular-file hard link under the shared slot.
    pub fn begin_link(
        self: &Arc<Self>,
        from: &Path,
        to: &Path,
    ) -> Result<OwnedNamespaceTicket, CoreError> {
        self.begin_namespace(&[Kind::Link], from, Some(to), || {
            self.binder
                .begin_link(from, to)
                .map(|ticket| (Kind::Link, NamespaceOperation::Path(ticket)))
        })
    }

    /// Begin one canonical regular-file move under the shared slot.
    pub fn begin_move(
        self: &Arc<Self>,
        from: &Path,
        to: &Path,
    ) -> Result<OwnedNamespaceTicket, CoreError> {
        self.begin_namespace(
            &[Kind::MoveOverwrite, Kind::DirectoryMoveOverwrite],
            from,
            Some(to),
            || {
                if self.binder.namespace_source_is_directory(from)? {
                    self.binder.begin_directory_move(from, to).map(|ticket| {
                        (
                            Kind::DirectoryMoveOverwrite,
                            NamespaceOperation::DirectoryMove(ticket),
                        )
                    })
                } else {
                    self.binder
                        .begin_move(from, to)
                        .map(|ticket| (Kind::MoveOverwrite, NamespaceOperation::Path(ticket)))
                }
            },
        )
    }

    /// The caller explicitly authorizes discarding its staged hard-link alias
    /// only if this exact move is proved unchanged after a started failure.
    pub fn begin_staged_move(
        self: &Arc<Self>,
        from: &Path,
        to: &Path,
    ) -> Result<OwnedNamespaceTicket, CoreError> {
        self.begin_namespace(&[Kind::StagedMove], from, Some(to), || {
            if self.binder.namespace_source_is_directory(from)? {
                return Err(contract("staged move source must be a regular file"));
            }
            self.binder
                .begin_move(from, to)
                .map(|ticket| (Kind::StagedMove, NamespaceOperation::Path(ticket)))
        })
    }

    /// Begin a create-only move; an existing target cannot be overwritten.
    pub fn begin_move_if_absent(
        self: &Arc<Self>,
        from: &Path,
        to: &Path,
    ) -> Result<OwnedNamespaceTicket, CoreError> {
        self.begin_namespace(
            &[Kind::MoveCreate, Kind::DirectoryMoveCreate],
            from,
            Some(to),
            || {
                if self.binder.namespace_source_is_directory(from)? {
                    self.binder.begin_directory_move(from, to).map(|ticket| {
                        (
                            Kind::DirectoryMoveCreate,
                            NamespaceOperation::DirectoryMove(ticket),
                        )
                    })
                } else {
                    self.binder
                        .begin_move_if_absent(from, to)
                        .map(|ticket| (Kind::MoveCreate, NamespaceOperation::Path(ticket)))
                }
            },
        )
    }

    /// Begin one bounded directory move under the shared slot.
    pub fn begin_directory_move(
        self: &Arc<Self>,
        from: &Path,
        to: &Path,
    ) -> Result<OwnedNamespaceTicket, CoreError> {
        self.begin_namespace(&[Kind::DirectoryMoveCreate], from, Some(to), || {
            self.binder.begin_directory_move(from, to).map(|ticket| {
                (
                    Kind::DirectoryMoveCreate,
                    NamespaceOperation::DirectoryMove(ticket),
                )
            })
        })
    }

    /// Begin one bounded directory deletion under the shared slot.
    pub fn begin_directory_delete(
        self: &Arc<Self>,
        path: &Path,
    ) -> Result<OwnedNamespaceTicket, CoreError> {
        self.begin_namespace(&[Kind::DirectoryDelete], path, None, || {
            self.binder
                .begin_directory_delete(path, &mut || Ok(()))
                .map(|ticket| {
                    (
                        Kind::DirectoryDelete,
                        NamespaceOperation::DirectoryDelete(ticket),
                    )
                })
        })
    }

    fn begin_namespace(
        self: &Arc<Self>,
        permitted: &[Kind],
        from: &Path,
        to: Option<&Path>,
        create: impl FnOnce() -> Result<(Kind, NamespaceOperation), CoreError>,
    ) -> Result<OwnedNamespaceTicket, CoreError> {
        let from = self.canonical_path(from)?;
        let to = to.map(|path| self.canonical_path(path)).transpose()?;
        let mut pending = self.lock_pending()?;
        if let Some(prior) = pending.as_ref() {
            let PendingOperation::Namespace(prior) = prior else {
                return Err(busy(pending.as_ref()));
            };
            if !permitted.contains(&prior.kind) || prior.from != from || prior.to != to {
                return Err(busy(pending.as_ref()));
            }
            if reconcile_pending_namespace(self, &mut pending)? {
                return Ok(OwnedNamespaceTicket {
                    owner: Arc::clone(self),
                    token: None,
                    completed: true,
                });
            }
            return Err(contract(
                "unchanged namespace operation lacked original error",
            ));
        }
        let (kind, operation) = create()?;
        if !permitted.contains(&kind) {
            return Err(contract("namespace operation kind differs from request"));
        }
        let staged_source = if kind == Kind::StagedMove {
            let NamespaceOperation::Path(ticket) = &operation else {
                return Err(contract("staged move lacks a regular-file ticket"));
            };
            let (source, parent) = ticket.source_metadata()?;
            if source.nlink() < 2 {
                return Err(contract("staged move source lacks a same-inode alias"));
            }
            Some(StagedSource {
                observed: InodeObservation {
                    inode: InodeKey {
                        device: source.dev(),
                        inode: source.ino(),
                    },
                    bytes: source.len(),
                    links: source.nlink(),
                },
                parent: InodeKey {
                    device: parent.dev(),
                    inode: parent.ino(),
                },
            })
        } else {
            None
        };
        let token = Arc::new(());
        *pending = Some(PendingOperation::Namespace(Box::new(PendingNamespace {
            kind,
            from,
            to,
            token: Arc::clone(&token),
            active: true,
            operation,
            outcome: NamespaceOutcome::Unreported,
            staged_source,
            cleanup: None,
        })));
        drop(pending);
        Ok(OwnedNamespaceTicket {
            owner: Arc::clone(self),
            token: Some(token),
            completed: false,
        })
    }

    /// Reconcile a retained namespace operation even after a scan no longer
    /// finds its old pathname. Never creates a new ticket or repeats a rename.
    pub fn reconcile_pending_namespace(&self) -> Result<(), CoreError> {
        let mut pending = self.lock_pending()?;
        if matches!(pending.as_ref(), Some(PendingOperation::Delete(_))) {
            self.reconcile_pending(&mut pending)
        } else {
            reconcile_pending_namespace(self, &mut pending).map(|_changed| ())
        }
    }

    /// Recover matching move or prior source cleanup without a new path probe.
    pub(crate) fn recover_directory_move_if_pending(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<bool, CoreError> {
        let from = self.canonical_path(from)?;
        let to = self.canonical_path(to)?;
        let mut pending = self.lock_pending()?;
        let Some(prior) = pending.as_ref() else {
            return Ok(false);
        };
        let PendingOperation::Namespace(prior) = prior else {
            return Err(busy(pending.as_ref()));
        };
        if prior.kind == Kind::DirectoryDelete && prior.from == from && prior.to.is_none() {
            if reconcile_pending_namespace(self, &mut pending)? {
                return Ok(false);
            }
            return Err(contract("source cleanup lacked an original outcome"));
        }
        if prior.kind != Kind::DirectoryMoveCreate
            || prior.from != from
            || prior.to.as_ref() != Some(&to)
        {
            return Err(busy(pending.as_ref()));
        }
        let result = reconcile_pending_namespace(self, &mut pending);
        drop(pending);
        result
    }

    /// Recover only the matching original delete, preserving its path receipt.
    pub(crate) fn recover_directory_delete_if_pending(
        &self,
        path: &Path,
    ) -> Result<Option<u64>, CoreError> {
        let path = self.canonical_path(path)?;
        let mut pending = self.lock_pending()?;
        let Some(prior) = pending.as_ref() else {
            return Ok(None);
        };
        let PendingOperation::Namespace(prior) = prior else {
            return Err(busy(pending.as_ref()));
        };
        if prior.kind != Kind::DirectoryDelete || prior.from != path || prior.to.is_some() {
            return Err(busy(pending.as_ref()));
        }
        let NamespaceOperation::DirectoryDelete(ticket) = &prior.operation else {
            return Err(contract("tree recovery lacks its original delete plan"));
        };
        let bytes = ticket.planned_file_bytes()?;
        let recovered = reconcile_pending_namespace(self, &mut pending)?;
        drop(pending);
        if recovered {
            Ok(Some(bytes))
        } else {
            Err(contract("tree recovery lacked an original outcome"))
        }
    }
}

pub(super) fn reconcile_pending_namespace(
    owner: &PublicationFileDeletions,
    pending: &mut Option<PendingOperation>,
) -> Result<bool, CoreError> {
    let Some(PendingOperation::Namespace(prior)) = pending.as_mut() else {
        return Ok(false);
    };
    if prior.active {
        return Err(busy(pending.as_ref()));
    }
    if !prior.operation.has_started() {
        return Err(contract("unstarted namespace claim outlived its handle"));
    }
    if prior.kind == Kind::StagedMove {
        if prior.cleanup.is_none() {
            if prior.operation.is_settled() || prior.operation.reconcile_after_failure()? {
                owner.binder.sweep_retired()?;
                drop(pending.take());
                return Ok(true);
            }
            prior.cleanup = Some(StagedCleanup {
                ticket: None,
                failure: None,
            });
        }
        let primary = prior.original_error();
        match reconcile_staged_cleanup(owner, prior) {
            Ok(()) => {
                drop(pending.take());
                return Err(primary);
            }
            Err(cleanup) => return Err(with_cleanup_context(primary, &cleanup)),
        }
    }
    if prior.operation.is_settled() || prior.operation.reconcile_after_failure()? {
        owner.binder.sweep_retired()?;
        drop(pending.take());
        return Ok(true);
    }
    let error = match &prior.outcome {
        NamespaceOutcome::Failed(error) => error.clone(),
        NamespaceOutcome::Unreported => CoreError::Storage(
            "external namespace outcome was not reported after unchanged proof".into(),
        ),
    };
    drop(pending.take());
    Err(error)
}

impl PendingNamespace {
    pub(super) fn busy_detail(&self) -> String {
        format!(
            "namespace kind={:?}, from={}, to={:?}, active={}, started={}, settled={}, cleanup={}",
            self.kind,
            self.from.display(),
            self.to,
            self.active,
            self.operation.has_started(),
            self.operation.is_settled(),
            self.cleanup.is_some(),
        )
    }

    fn original_error(&self) -> CoreError {
        match &self.outcome {
            NamespaceOutcome::Failed(error) => error.clone(),
            NamespaceOutcome::Unreported => CoreError::Storage(
                "external namespace outcome was not reported after unchanged proof".into(),
            ),
        }
    }
}

fn reconcile_staged_cleanup(
    owner: &PublicationFileDeletions,
    prior: &mut PendingNamespace,
) -> Result<(), CoreError> {
    let source = prior
        .staged_source
        .ok_or_else(|| contract("staged cleanup lacks original source witness"))?;
    let cleanup = prior
        .cleanup
        .as_mut()
        .ok_or_else(|| contract("staged cleanup state absent"))?;
    if let Some(ticket) = cleanup.ticket.as_mut() {
        if ticket.is_settled() {
            owner.binder.sweep_retired()?;
            return Ok(());
        }
        if ticket.has_started() {
            return match owner.binder.path_observation(&prior.from)? {
                None => ticket.unlinked_and_synced(),
                Some(actual) if actual == source.observed => {
                    ticket.confirmed_unchanged_after_failure()?;
                    drop(cleanup.ticket.take());
                    Err(cleanup
                        .failure
                        .clone()
                        .ok_or_else(|| contract("staged delete lacks first cleanup error"))?)
                }
                Some(_) => Err(contract(
                    "staged cleanup source changed after unlink attempt",
                )),
            };
        }
    }
    let ticket = owner.binder.begin_delete(&prior.from)?;
    let (actual, parent) = ticket.source_metadata()?;
    if actual.dev() != source.observed.inode.device
        || actual.ino() != source.observed.inode.inode
        || actual.len() != source.observed.bytes
        || actual.nlink() != source.observed.links
        || parent.dev() != source.parent.device
        || parent.ino() != source.parent.inode
    {
        return Err(contract(
            "staged cleanup source or parent differs from original ticket",
        ));
    }
    cleanup.ticket = Some(ticket);
    let delete = cleanup
        .ticket
        .as_mut()
        .ok_or_else(|| contract("staged cleanup delete ticket absent"))?;
    match delete.perform_and_sync() {
        Ok(()) => Ok(()),
        Err(error) => {
            if cleanup.failure.is_none() {
                cleanup.failure = Some(error.clone());
            }
            if !delete.has_started() {
                drop(cleanup.ticket.take());
            }
            Err(error)
        }
    }
}

fn with_cleanup_context(primary: CoreError, cleanup: &CoreError) -> CoreError {
    let context = format!("; staged alias cleanup also failed: {cleanup:?}");
    match primary {
        CoreError::InvalidContract(mut message) => {
            message.push_str(&context);
            CoreError::InvalidContract(message)
        }
        CoreError::Typed { code, mut message } => {
            message.push_str(&context);
            CoreError::Typed { code, message }
        }
        CoreError::NotReady(mut message) => {
            message.push_str(&context);
            CoreError::NotReady(message)
        }
        CoreError::NotImplemented(mut message) => {
            message.push_str(&context);
            CoreError::NotImplemented(message)
        }
        CoreError::NotFound(mut message) => {
            message.push_str(&context);
            CoreError::NotFound(message)
        }
        CoreError::Storage(mut message) => {
            message.push_str(&context);
            CoreError::Storage(message)
        }
        CoreError::StorageIo {
            kind,
            raw_os_error,
            mut message,
        } => {
            message.push_str(&context);
            CoreError::StorageIo {
                kind,
                raw_os_error,
                message,
            }
        }
    }
}

impl OwnedNamespaceTicket {
    /// Read the original tree-delete plan without a second filesystem scan.
    pub fn planned_file_bytes(&self) -> Result<u64, CoreError> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| contract("tree receipt ticket is complete"))?;
        let pending = self.owner.lock_pending()?;
        let Some(PendingOperation::Namespace(prior)) = pending.as_ref() else {
            return Err(contract("tree receipt has no original namespace ticket"));
        };
        if !prior.active || !Arc::ptr_eq(token, &prior.token) {
            return Err(contract("tree receipt differs from active namespace owner"));
        }
        let result = match &prior.operation {
            NamespaceOperation::DirectoryDelete(ticket) => ticket.planned_file_bytes(),
            NamespaceOperation::Path(_) | NamespaceOperation::DirectoryMove(_) => {
                Err(contract("path-byte receipt requires a tree-delete ticket"))
            }
        };
        drop(pending);
        result
    }
    fn with_pending(
        &mut self,
        callback: impl FnOnce(&mut PendingNamespace) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if self.completed {
            return Ok(());
        }
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| contract("namespace handle lost its token"))?;
        let mut pending = self.owner.lock_pending()?;
        let Some(PendingOperation::Namespace(prior)) = pending.as_mut() else {
            return Err(contract("namespace handle lost its original ticket"));
        };
        if !prior.active || !Arc::ptr_eq(token, &prior.token) {
            return Err(contract("namespace handle differs from slot owner"));
        }
        let result = callback(prior);
        if result.is_ok() && prior.operation.is_settled() {
            drop(pending.take());
            self.completed = true;
            self.token = None;
        } else if let Err(error) = &result {
            if prior.operation.has_started() {
                if matches!(&prior.outcome, NamespaceOutcome::Unreported) {
                    prior.outcome = NamespaceOutcome::Failed(error.clone());
                }
            } else {
                drop(pending.take());
                self.completed = true;
                self.token = None;
            }
        }
        drop(pending);
        result
    }

    /// Execute an anchored filesystem operation and settle its original ticket.
    pub fn perform_and_sync(&mut self) -> Result<(), CoreError> {
        self.with_pending(|prior| prior.operation.perform_and_sync())
    }

    /// Start a caller-owned regular-file move before its existing syscall.
    pub fn change_started(&mut self) -> Result<(), CoreError> {
        if self.completed {
            return Err(contract("completed namespace operation cannot start again"));
        }
        self.with_pending(|prior| {
            if prior.kind != Kind::MoveOverwrite {
                return Err(contract("external callback requires a regular-file move"));
            }
            prior.operation.change_started()
        })
    }

    /// Settle a caller-owned regular-file move after its rename and sync.
    pub fn changed_and_synced(&mut self) -> Result<(), CoreError> {
        if self.completed {
            return Err(contract(
                "completed namespace operation cannot settle again",
            ));
        }
        self.with_pending(|prior| {
            if prior.kind != Kind::MoveOverwrite {
                return Err(contract("external callback requires a regular-file move"));
            }
            prior.operation.changed_and_synced()
        })
    }
}

impl Drop for OwnedNamespaceTicket {
    fn drop(&mut self) {
        let Some(token) = &self.token else {
            return;
        };
        let mut pending = match self.owner.pending.lock() {
            Ok(pending) => pending,
            Err(_poisoned) => {
                // The active handle cannot transfer custody to a poisoned
                // slot. Quarantine the physical root instead of silently
                // treating this as an ordinary unresolved operation.
                self.owner.binder.mark_close_unwitnessed();
                return;
            }
        };
        let Some(PendingOperation::Namespace(prior)) = pending.as_mut() else {
            return;
        };
        if !Arc::ptr_eq(token, &prior.token) {
            return;
        }
        if prior.operation.has_started() {
            prior.active = false;
        } else {
            drop(pending.take());
        }
    }
}

#[cfg(test)]
mod tests;
