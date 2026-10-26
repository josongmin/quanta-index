//! Bind sidecar output permits to the canonical capacity ledger.
//!
//! The caller's work or control allowance and Lance's writer admission
//! subdivide one `PublicationCapacity` held by the composition root.

use std::fs::File;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::{
    PublicationOutputClass, PublicationOutputPermit, PublicationOutputReservation,
};
use quanta_index_core::publication_capacity::{InodeKey, OutputTicket, PublicationAllowance};

use crate::publication_file_capacity::{FileMutation, PublicationFileCapacity, ReaderLease};

use crate::publication_file_deletions::{OwnedNamespaceTicket, PublicationFileDeletions};

pub fn sidecar_output_permit(
    deletions: Arc<PublicationFileDeletions>,
    held_parent: Arc<PublicationAllowance>,
    request: PublicationOutputReservation,
) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
    let recover_retired_create = request.class() == PublicationOutputClass::Control;
    output_permit(deletions, held_parent, request, recover_retired_create)
}

/// An exact-width staged service writer that transfers every original FD
/// through the permit may use the existing retired-create recovery owner.
///
/// Funding class and its held allowance remain unchanged.
pub fn recoverable_sidecar_output_permit(
    deletions: Arc<PublicationFileDeletions>,
    held_parent: Arc<PublicationAllowance>,
    request: PublicationOutputReservation,
) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
    output_permit(deletions, held_parent, request, true)
}

fn output_permit(
    deletions: Arc<PublicationFileDeletions>,
    held_parent: Arc<PublicationAllowance>,
    request: PublicationOutputReservation,
    recover_retired_create: bool,
) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
    let binder = deletions.binder();
    binder.validate_held_allowance(&held_parent)?;
    let reserved_output = held_parent.reserve_output(request.max_encoded_bytes())?;
    Ok(Box::new(SidecarOutputPermit {
        binder,
        deletions,
        held_parent,
        bound: request.max_encoded_bytes(),
        recover_control_create: recover_retired_create,
        reserved_output: Some(reserved_output),
        staging: None,
        target: None,
        mutation: None,
        settled_lease: None,
        path_ticket: None,
        pending_target: None,
        removed: false,
    }))
}

struct SidecarOutputPermit {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    held_parent: Arc<PublicationAllowance>,
    bound: u64,
    recover_control_create: bool,
    reserved_output: Option<OutputTicket>,
    staging: Option<PathBuf>,
    target: Option<PathBuf>,
    mutation: Option<FileMutation>,
    // The binder owns a distinct physical FD until the actual writer FD has
    // closed. Namespace moves/deletes never refund on pathname absence alone.
    settled_lease: Option<ReaderLease>,
    path_ticket: Option<OwnedNamespaceTicket>,
    pending_target: Option<PathBuf>,
    removed: bool,
}

impl SidecarOutputPermit {
    fn mutation(&mut self) -> Result<&mut FileMutation, CoreError> {
        self.mutation
            .as_mut()
            .ok_or_else(|| CoreError::InvalidContract("sidecar has no bound file mutation".into()))
    }

    fn target(&self) -> Result<&Path, CoreError> {
        self.target
            .as_deref()
            .ok_or_else(|| CoreError::InvalidContract("sidecar has no current target".into()))
    }
}

impl PublicationOutputPermit for SidecarOutputPermit {
    fn cancel_rejected_created(&mut self, writer: File) -> Result<(), CoreError> {
        self.mutation()?.cancel_rejected_created_temp(writer)
    }
    fn bind_output(&mut self, staging: &Path, target: &Path) -> Result<(), CoreError> {
        if self.staging.is_some() || self.mutation.is_some() || self.reserved_output.is_none() {
            return Err(CoreError::InvalidContract("sidecar already bound".into()));
        }
        self.binder.validate_held_allowance(&self.held_parent)?;
        self.binder.ensure_private_parent_directories(target)?;
        let ticket = self.reserved_output.take().ok_or_else(|| {
            CoreError::InvalidContract("sidecar reserved output disappeared".into())
        })?;
        let mut mutation = self.binder.begin_create_with_reserved_output(
            target,
            self.bound,
            Arc::clone(&self.held_parent),
            ticket,
        )?;
        mutation.declare_temp_path(staging)?;
        if self.recover_control_create {
            mutation.retain_control_create_for_recovery()?;
        }
        self.staging = Some(staging.to_owned());
        self.target = Some(target.to_owned());
        self.mutation = Some(mutation);
        Ok(())
    }

    fn creation_started(&mut self) -> Result<(), CoreError> {
        self.mutation()?.creation_started()
    }

    fn created(&mut self, writer: &File, staging: &Path) -> Result<(), CoreError> {
        if self.staging.as_deref() != Some(staging) {
            return Err(CoreError::InvalidContract("sidecar temp changed".into()));
        }
        self.mutation()?.created(writer, staging)
    }

    fn before_replace(&mut self) -> Result<(), CoreError> {
        let target = self.target()?.to_owned();
        self.mutation()?.before_replace(&target)
    }

    fn committed(&mut self, writer: File, actual_encoded_bytes: u64) -> Result<(), CoreError> {
        let settlement = (|| {
            if actual_encoded_bytes > self.bound {
                return Err(CoreError::InvalidContract("sidecar exceeded bound".into()));
            }
            let actual = writer
                .metadata()
                .map_err(|error| CoreError::storage_io("sidecar writer metadata", &error))?
                .len();
            if actual != actual_encoded_bytes {
                return Err(CoreError::InvalidContract(
                    "sidecar encoded width differs from writer".into(),
                ));
            }
            let target = self.target()?.to_owned();
            self.mutation()?
                .replaced_and_synced(&writer, &target)
                .map(|_retained| ())
        })();
        if let Err(primary) = settlement {
            return match self.retire_writer_preserving_pending(writer) {
                Ok(()) => Err(primary),
                Err(retirement) => Err(with_retirement_context(primary, &retirement)),
            };
        }
        // The binder owns this identity; the actual writer FD is transferred
        // below before we take a settled lease.
        self.mutation()?.confirmed_safe_writer_fd_retired(writer)?;
        self.settled_lease = Some(self.mutation()?.take_owned_lease()?);
        // The reader lease now retains the inode charge. Retire the settled
        // writer's namespace claim before this output can move or be removed.
        self.mutation = None;
        Ok(())
    }

    fn retire_writer_preserving_pending(&mut self, writer: File) -> Result<(), CoreError> {
        self.mutation()?.confirmed_safe_writer_fd_retired(writer)
    }

    fn rename_started(&mut self, target: &Path) -> Result<(), CoreError> {
        if self.path_ticket.is_some() || self.removed {
            return Err(CoreError::InvalidContract(
                "sidecar path operation pending".into(),
            ));
        }
        self.binder.ensure_private_parent_directories(target)?;
        let mut ticket = self.deletions.begin_move(self.target()?, target)?;
        ticket.change_started()?;
        self.path_ticket = Some(ticket);
        self.pending_target = Some(target.to_owned());
        Ok(())
    }

    fn renamed_and_synced(&mut self) -> Result<(), CoreError> {
        self.path_ticket
            .as_mut()
            .ok_or_else(|| CoreError::InvalidContract("sidecar move was not started".into()))?
            .changed_and_synced()?;
        self.path_ticket = None;
        self.target = self.pending_target.take();
        Ok(())
    }

    fn delete_file_and_sync(&mut self) -> Result<(), CoreError> {
        if self.removed || self.path_ticket.is_some() || self.mutation.is_some() {
            return Err(CoreError::InvalidContract(
                "sidecar removal requires a settled output without a pending move".into(),
            ));
        }
        let lease = self.settled_lease.as_ref().ok_or_else(|| {
            CoreError::InvalidContract("sidecar removal lacks its committed inode lease".into())
        })?;
        let metadata = lease
            .file()?
            .metadata()
            .map_err(|error| CoreError::storage_io("sidecar deletion witness metadata", &error))?;
        self.deletions.delete_owned_file_and_sync(
            self.target()?,
            InodeKey {
                device: metadata.dev(),
                inode: metadata.ino(),
            },
            metadata.len(),
        )?;
        self.removed = true;
        Ok(())
    }

    fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError> {
        self.mutation()?.confirmed_uncreated_absent_and_synced()
    }

    fn confirmed_safe_absent_after_writer_close(&mut self, writer: File) -> Result<(), CoreError> {
        self.mutation()?
            .confirmed_safe_absent_after_writer_close(writer)
    }
}

fn with_retirement_context(mut primary: CoreError, retirement: &CoreError) -> CoreError {
    use std::fmt::Write as _;
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. }
        | CoreError::Typed { message, .. } => message,
    };
    let _format_result = write!(
        message,
        "; sidecar writer retirement also failed: {retirement}"
    );
    primary
}
