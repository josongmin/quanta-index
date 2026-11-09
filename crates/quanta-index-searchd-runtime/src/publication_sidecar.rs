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

use crate::publication_file_capacity::{
    FileMutation, InodeObservation, PublicationFileCapacity, ReaderLease, append_failure,
};

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

    fn prepare_move(&self, target: &Path) -> Result<OwnedNamespaceTicket, CoreError> {
        if self.path_ticket.is_some() || self.removed {
            return Err(CoreError::InvalidContract(
                "sidecar path operation pending".into(),
            ));
        }
        let expected = InodeObservation::from_file(
            self.settled_lease
                .as_ref()
                .ok_or_else(|| {
                    CoreError::InvalidContract("sidecar move lacks settled lease".into())
                })?
                .file()?,
        )?;
        let mut ticket = self.deletions.begin_sidecar_move(self.target()?, target)?;
        ticket.require_source_identity(expected)?;
        Ok(ticket)
    }
}

impl PublicationOutputPermit for SidecarOutputPermit {
    fn cancel_rejected_created(&mut self, writer: File) -> Result<(), CoreError> {
        self.mutation()?
            .cancel_sidecar_rejected_created_temp(writer)
    }
    fn bind_output(&mut self, staging: &Path, target: &Path) -> Result<(), CoreError> {
        if self.staging.is_some() || self.mutation.is_some() || self.reserved_output.is_none() {
            return Err(CoreError::InvalidContract("sidecar already bound".into()));
        }
        // The binder authenticates funding and the current root before its
        // private-parent walk and path claim.
        let mut mutation = self
            .binder
            .begin_create_with_reserved_output_in_private_parent(
                target,
                self.bound,
                Arc::clone(&self.held_parent),
                &mut self.reserved_output,
            )?;
        let declared = (|| {
            if self.recover_control_create {
                mutation.declare_temp_path(staging)?;
            } else {
                mutation.declare_temp_path_from_claimed_parent(staging)?;
            }
            mutation.require_sidecar_staging_device()?;
            if self.recover_control_create {
                mutation.retain_control_create_for_recovery()?;
            }
            Ok(())
        })();
        if let Err(primary) = declared {
            let reclaimed = mutation.reclaim_unstarted_create_ticket();
            drop(mutation);
            return match reclaimed {
                Ok(ticket) => {
                    self.reserved_output = Some(ticket);
                    Err(primary)
                }
                Err(rollback) => Err(append_failure(
                    primary,
                    "sidecar bind rollback also failed",
                    &rollback,
                )),
            };
        }
        self.staging = Some(staging.to_owned());
        self.target = Some(target.to_owned());
        self.mutation = Some(mutation);
        Ok(())
    }

    fn creation_started(&mut self) -> Result<(), CoreError> {
        let mutation = self.mutation()?;
        mutation.require_sidecar_private_parents_current()?;
        mutation.creation_started()
    }

    fn create_staging(&mut self) -> Result<File, CoreError> {
        let mutation = self.mutation()?;
        mutation.start_sidecar_declared_temp()?;
        match mutation.open_declared_temp() {
            Ok(file) => Ok(file),
            Err(primary) => match mutation.confirmed_uncreated_absent_and_synced() {
                Ok(()) => Err(primary),
                Err(cleanup) => Err(append_failure(
                    primary,
                    "sidecar uncreated settlement also failed",
                    &cleanup,
                )),
            },
        }
    }

    fn created(&mut self, writer: &File, staging: &Path) -> Result<(), CoreError> {
        if self.staging.as_deref() != Some(staging) {
            return Err(CoreError::InvalidContract("sidecar temp changed".into()));
        }
        self.mutation()?.created(writer, staging)
    }

    fn before_replace(&mut self) -> Result<(), CoreError> {
        let target = self.target()?.to_owned();
        let mutation = self.mutation()?;
        mutation.require_sidecar_private_parents_current()?;
        mutation.before_replace(&target)
    }

    fn rename_staging(&mut self, writer: &File) -> Result<(), CoreError> {
        let target = self.target()?.to_owned();
        self.mutation()?
            .publish_declared_temp_replace(writer, &target)
    }

    fn sync_promoted_parents(&mut self) -> Result<(), CoreError> {
        self.mutation()?.sync_promoted_parents()
    }

    fn committed(&mut self, writer: File, actual_encoded_bytes: u64) -> Result<(), CoreError> {
        let validation = (|| {
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
            Ok(())
        })();
        if let Err(primary) = validation {
            return match self.retire_writer_preserving_pending(writer) {
                Ok(()) => Err(primary),
                Err(retirement) => Err(append_failure(
                    primary,
                    "sidecar writer retirement also failed",
                    &retirement,
                )),
            };
        }
        // The caller completed durable promotion before committed(). Retire
        // its actual writer, then authenticate the binder's original pin and
        // destination without repeating the file and parent sync protocol.
        self.mutation()?.confirmed_safe_writer_fd_retired(writer)?;
        let _retained = self
            .mutation()?
            .settle_after_external_file_and_parent_sync()?;
        let lease = self.mutation()?.take_sidecar_owned_lease();
        // Retire the namespace claim after the pin transfers or closes.
        // The settled inode remains charged even if lease admission refused.
        self.mutation = None;
        self.settled_lease = Some(lease?);
        Ok(())
    }

    fn retire_writer_preserving_pending(&mut self, writer: File) -> Result<(), CoreError> {
        self.mutation()?.confirmed_safe_writer_fd_retired(writer)
    }

    fn rename_started(&mut self, target: &Path) -> Result<(), CoreError> {
        let mut ticket = self.prepare_move(target)?;
        ticket.change_started()?;
        self.path_ticket = Some(ticket);
        self.pending_target = Some(target.to_owned());
        Ok(())
    }

    fn move_output(&mut self, target: &Path) -> Result<(), CoreError> {
        let mut ticket = self.prepare_move(target)?;
        ticket.perform_move()?;
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
        self.deletions.delete_sidecar_owned_file_and_sync(
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

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs::{self, File};
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::path::Path;
    use std::sync::Arc;

    use quanta_index_core::RequestBudgetV1;
    use quanta_index_core::publication_admission::{
        PublicationOutputClass, PublicationOutputReservation,
    };
    use quanta_index_core::publication_capacity::PublicationCapacity;
    use quanta_index_searchd::app::runtime::StateRootLease;
    use rustix::fs::{Mode, OFlags, RenameFlags, fsync, openat, renameat_with};

    use super::sidecar_output_permit;
    use crate::publication_file_capacity::PublicationFileCapacity;
    use crate::publication_file_deletions::PublicationFileDeletions;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};

    type Fixture = (
        tempfile::TempDir,
        std::path::PathBuf,
        StateRootLease,
        PublicationCapacity,
        Arc<PublicationFileCapacity>,
        Arc<PublicationFileDeletions>,
    );

    fn fixture() -> Result<Fixture, Box<dyn Error>> {
        fixture_with_open_handles(16)
    }

    fn fixture_with_open_handles(max_open_handles: usize) -> Result<Fixture, Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let requested_root = directory.path().join("state");
        fs::create_dir(&requested_root)?;
        fs::set_permissions(&requested_root, fs::Permissions::from_mode(0o700))?;
        let lease = StateRootLease::acquire(&requested_root)?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let authority = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 1024,
                max_files: 32,
                max_pending: 32,
                max_entries: 64,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let binder = PublicationFileCapacity::new(
            lease.clone(),
            authority.clone(),
            max_open_handles,
            64,
            256,
        )?;
        let deletions = Arc::new(PublicationFileDeletions::new(Arc::clone(&binder)));
        Ok((directory, root, lease, authority, binder, deletions))
    }

    #[test]
    fn committed_output_releases_claim_after_lease_ceiling_refusal() -> Result<(), Box<dyn Error>> {
        use quanta_index_core::publication_admission::PublicationOutputPermit;

        let (_directory, root, _lease, authority, binder, deletions) =
            fixture_with_open_handles(1)?;
        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(8, 2)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 4, 1)?;
        let mut first = sidecar_output_permit(Arc::clone(&deletions), Arc::clone(&held), request)?;
        let mut second = sidecar_output_permit(deletions, Arc::clone(&held), request)?;
        let stage = |permit: &mut dyn PublicationOutputPermit,
                     name: &str|
         -> Result<File, Box<dyn Error>> {
            let staging = root.join(format!("{name}.tmp"));
            let target = root.join(name);
            permit.bind_output(&staging, &target)?;
            let mut writer = permit.create_staging()?;
            permit.created(&writer, &staging)?;
            writer.write_all(b"four")?;
            writer.sync_all()?;
            permit.rename_staging(&writer)?;
            permit.sync_promoted_parents()?;
            Ok(writer)
        };

        let first_writer = stage(first.as_mut(), "first")?;
        first.committed(first_writer, 4)?;
        let second_writer = stage(second.as_mut(), "second")?;
        assert!(matches!(
            second.committed(second_writer, 4),
            Err(quanta_index_core::CoreError::Typed { message, .. })
                if message.contains("owned FD pin ceiling exceeded")
        ));
        drop(second);
        drop(first);
        drop(held);

        let retained = authority.snapshot()?;
        assert_eq!(retained.used_bytes, baseline.used_bytes + 8);
        assert_eq!(retained.used_files, baseline.used_files + 2);
        assert_eq!(retained.uncertain_pending, baseline.uncertain_pending);
        assert_eq!(fs::read(root.join("second"))?, b"four");
        binder
            .begin_delete(&root.join("second"))?
            .perform_and_sync()?;
        binder
            .begin_delete(&root.join("first"))?
            .perform_and_sync()?;
        assert_eq!(authority.snapshot()?, baseline);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    fn published_first_output_settles_after_second_reservation_refusal()
    -> Result<(), Box<dyn Error>> {
        let (_directory, root, lease, authority, binder, deletions) = fixture()?;
        let root_fd = lease.try_clone_root_directory_v1()?;
        let baseline = authority.snapshot()?;
        let held_work = binder.reserve_publication_allowance(3, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Lexical, 3, 1)?;
        let mut first =
            sidecar_output_permit(Arc::clone(&deletions), Arc::clone(&held_work), request)?;
        let staging = root.join("first.tmp");
        let target = root.join("first.final");
        first.bind_output(&staging, &target)?;
        first.creation_started()?;
        let mut writer = File::from(openat(
            &root_fd,
            Path::new("first.tmp"),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        first.created(&writer, &staging)?;
        writer.write_all(b"abc")?;
        writer.sync_all()?;
        let original = writer.metadata()?;
        first.before_replace()?;
        renameat_with(
            &root_fd,
            Path::new("first.tmp"),
            &root_fd,
            Path::new("first.final"),
            RenameFlags::NOREPLACE,
        )?;
        assert!(!staging.exists());
        assert_eq!(fs::read(&target)?, b"abc");

        // The published first output still owns the original writer FD and
        // needs the shared parent-sync barrier before it can settle.
        let before_refusal = authority.snapshot()?;
        let second = PublicationOutputReservation::new(PublicationOutputClass::Lexical, 1, 1)?;
        assert!(matches!(
            sidecar_output_permit(Arc::clone(&deletions), Arc::clone(&held_work), second),
            Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                message,
            }) if message == "publication physical capacity: output exceeds held publication allowance: requested_bytes=1 free_bytes=0 free_files=0"
        ));
        assert_eq!(held_work.available_capacity()?, (0, 0));
        assert_eq!(authority.snapshot()?, before_refusal);

        fsync(&root_fd)?;
        first.committed(writer, 3)?;
        let published = fs::symlink_metadata(&target)?;
        assert!(published.is_file() && !published.file_type().is_symlink());
        assert_eq!(
            (published.dev(), published.ino()),
            (original.dev(), original.ino())
        );
        drop(first);
        drop(held_work);
        let after = authority.snapshot()?;
        assert_eq!(after.used_bytes, baseline.used_bytes + 3);
        assert_eq!(after.used_files, baseline.used_files + 1);
        assert_eq!(after.retained_inodes, baseline.retained_inodes + 1);
        assert_eq!(after.uncertain_pending, baseline.uncertain_pending);
        assert_eq!(after.active_tickets, baseline.active_tickets);
        assert_eq!(after.active_allowances, baseline.active_allowances);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    fn insecure_parent_bind_refusal_keeps_same_permit_retryable() -> Result<(), Box<dyn Error>> {
        let (_directory, root, lease, authority, binder, deletions) = fixture()?;
        let root_fd = lease.try_clone_root_directory_v1()?;
        let parent = root.join("repaired-parent");
        fs::create_dir(&parent)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;
        let baseline = authority.snapshot()?;
        let held_work = binder.reserve_publication_allowance(3, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Lexical, 3, 1)?;
        let mut permit =
            sidecar_output_permit(Arc::clone(&deletions), Arc::clone(&held_work), request)?;
        let charged = authority.snapshot()?;
        let staging = root.join("retry.tmp");
        let target = parent.join("retry.final");
        assert!(matches!(
            permit.bind_output(&staging, &target),
            Err(quanta_index_core::CoreError::InvalidContract(_))
        ));
        assert_eq!(authority.snapshot()?, charged);
        assert_eq!(held_work.available_capacity()?, (0, 0));
        assert!(!staging.exists() && !target.exists());
        assert!(binder.is_quiescent()?);

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        let parent_fd = File::open(&parent)?;
        fsync(&parent_fd)?;
        fsync(&root_fd)?;
        permit.bind_output(&staging, &target)?;
        permit.creation_started()?;
        let mut writer = File::from(openat(
            &root_fd,
            Path::new("retry.tmp"),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        permit.created(&writer, &staging)?;
        writer.write_all(b"abc")?;
        writer.sync_all()?;
        let original = writer.metadata()?;
        permit.before_replace()?;
        renameat_with(
            &root_fd,
            Path::new("retry.tmp"),
            &parent_fd,
            Path::new("retry.final"),
            RenameFlags::NOREPLACE,
        )?;
        fsync(&parent_fd)?;
        fsync(&root_fd)?;
        permit.committed(writer, 3)?;
        let published = fs::symlink_metadata(&target)?;
        assert_eq!(fs::read(&target)?, b"abc");
        assert_eq!(
            (published.dev(), published.ino()),
            (original.dev(), original.ino())
        );
        drop(permit);
        drop(held_work);
        let after = authority.snapshot()?;
        assert_eq!(after.used_bytes, baseline.used_bytes + 3);
        assert_eq!(after.used_files, baseline.used_files + 1);
        assert_eq!(after.retained_inodes, baseline.retained_inodes + 1);
        assert_eq!(after.uncertain_pending, baseline.uncertain_pending);
        assert_eq!(after.active_tickets, baseline.active_tickets);
        assert_eq!(after.active_allowances, baseline.active_allowances);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    #[cfg(target_os = "macos")]
    #[ignore = "requires QIDX_P08_TEST_IMAGE pointing to a private mountable image"]
    fn foreign_device_staging_bind_refuses_and_keeps_ticket() -> Result<(), Box<dyn Error>> {
        use std::process::Command;

        struct MountedImage(std::path::PathBuf);
        impl Drop for MountedImage {
            fn drop(&mut self) {
                let _detached = Command::new("/usr/bin/hdiutil")
                    .arg("detach")
                    .arg(&self.0)
                    .output();
            }
        }

        let image = std::env::var_os("QIDX_P08_TEST_IMAGE").ok_or("missing test image")?;
        let (_directory, root, _lease, authority, binder, deletions) = fixture()?;
        let mountpoint = root.join("foreign-staging");
        fs::create_dir(&mountpoint)?;
        let _mount = MountedImage(mountpoint.clone());
        let attached = Command::new("/usr/bin/hdiutil")
            .args(["attach", "-nobrowse", "-owners", "off", "-mountpoint"])
            .arg(&mountpoint)
            .arg(image)
            .output()?;
        if !attached.status.success() {
            return Err(String::from_utf8_lossy(&attached.stderr)
                .into_owned()
                .into());
        }
        assert_ne!(fs::metadata(&root)?.dev(), fs::metadata(&mountpoint)?.dev());

        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 4, 1)?;
        let mut permit = sidecar_output_permit(deletions, Arc::clone(&held), request)?;
        let foreign = mountpoint.join("staging");
        let target = root.join("target");
        let reserved = authority.snapshot()?;
        assert!(matches!(
            permit.bind_output(&foreign, &target),
            Err(quanta_index_core::CoreError::InvalidContract(message))
                if message.contains("device")
        ));
        assert!(!foreign.exists() && !target.exists());
        assert_eq!(authority.snapshot()?, reserved);

        let staging = root.join("staging");
        permit.bind_output(&staging, &target)?;
        let writer = permit.create_staging()?;
        permit.created(&writer, &staging)?;
        permit.cancel_rejected_created(writer)?;
        drop(permit);
        drop(held);
        assert!(!staging.exists() && !target.exists());
        assert_eq!(authority.snapshot()?, baseline);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    fn occupied_staging_bind_refusal_keeps_original_permit_retryable() -> Result<(), Box<dyn Error>>
    {
        let (_directory, root, _lease, authority, binder, deletions) = fixture()?;
        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Lexical, 4, 1)?;
        let mut permit = sidecar_output_permit(Arc::clone(&deletions), Arc::clone(&held), request)?;
        let staging = root.join("occupied.tmp");
        let target = root.join("occupied.final");
        fs::write(&staging, b"other")?;
        let charged = authority.snapshot()?;
        assert!(permit.bind_output(&staging, &target).is_err());
        assert_eq!(authority.snapshot()?, charged);
        assert_eq!(held.available_capacity()?, (0, 0));
        assert_eq!(fs::read(&staging)?, b"other");
        assert!(!target.exists());
        assert!(binder.is_quiescent()?);

        fs::remove_file(&staging)?;
        permit.bind_output(&staging, &target)?;
        let mut writer = permit.create_staging()?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        permit.rename_staging(&writer)?;
        permit.sync_promoted_parents()?;
        permit.committed(writer, 4)?;
        assert_eq!(fs::read(&target)?, b"four");
        drop(permit);
        drop(held);
        let after = authority.snapshot()?;
        assert_eq!(after.used_bytes, baseline.used_bytes + 4);
        assert_eq!(after.used_files, baseline.used_files + 1);
        assert_eq!(after.uncertain_pending, baseline.uncertain_pending);
        assert!(binder.is_quiescent()?);

        Ok(())
    }

    #[test]
    fn control_scope_bind_refusal_keeps_original_permit_retryable() -> Result<(), Box<dyn Error>> {
        let (_directory, root, _lease, authority, binder, deletions) = fixture()?;
        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Control, 4, 1)?;
        let mut permit = sidecar_output_permit(Arc::clone(&deletions), Arc::clone(&held), request)?;
        let staging = root.join("control-scope.tmp");
        let target = root.join("control-scope.final");
        let blocker = binder.begin_create(&staging)?;
        let charged = authority.snapshot()?;
        assert!(permit.bind_output(&staging, &target).is_err());
        assert_eq!(authority.snapshot()?, charged);
        assert_eq!(held.available_capacity()?, (0, 0));
        assert!(!staging.exists() && !target.exists());

        drop(blocker);
        permit.bind_output(&staging, &target)?;
        let mut writer = permit.create_staging()?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        permit.rename_staging(&writer)?;
        permit.sync_promoted_parents()?;
        permit.committed(writer, 4)?;
        assert_eq!(fs::read(&target)?, b"four");
        drop(permit);
        drop(held);
        let after = authority.snapshot()?;
        assert_eq!(after.used_bytes, baseline.used_bytes + 4);
        assert_eq!(after.used_files, baseline.used_files + 1);
        assert_eq!(after.uncertain_pending, baseline.uncertain_pending);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    fn anchored_create_refuses_parent_swap_before_start() -> Result<(), Box<dyn Error>> {
        let (_directory, root, _lease, authority, binder, deletions) = fixture()?;
        let parent_path = root.join("create-parent");
        let displaced = root.join("create-parent-displaced");
        fs::create_dir(&parent_path)?;
        fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o700))?;
        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 4, 1)?;
        let mut permit = sidecar_output_permit(deletions, Arc::clone(&held), request)?;
        let staging = parent_path.join("staging");
        let target = parent_path.join("target");
        permit.bind_output(&staging, &target)?;
        let reserved = authority.snapshot()?;
        fs::rename(&parent_path, &displaced)?;
        fs::create_dir(&parent_path)?;
        fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o700))?;
        assert!(matches!(
            permit.create_staging(),
            Err(quanta_index_core::CoreError::InvalidContract(_))
        ));
        assert!(!staging.exists() && !displaced.join("staging").exists());
        assert_eq!(authority.snapshot()?, reserved);

        fs::remove_dir(&parent_path)?;
        fs::rename(&displaced, &parent_path)?;
        let writer = permit.create_staging()?;
        permit.created(&writer, &staging)?;
        permit.cancel_rejected_created(writer)?;
        drop(permit);
        drop(held);
        assert!(!staging.exists() && !target.exists());
        assert_eq!(authority.snapshot()?, baseline);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    fn anchored_create_refuses_same_inode_parent_chmod_and_retries_original_ticket()
    -> Result<(), Box<dyn Error>> {
        let (_directory, root, _lease, authority, binder, deletions) = fixture()?;
        let parent = root.join("chmod-create-parent");
        fs::create_dir(&parent)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 4, 1)?;
        let mut permit = sidecar_output_permit(Arc::clone(&deletions), Arc::clone(&held), request)?;
        let staging = parent.join("staging");
        let target = parent.join("target");
        permit.bind_output(&staging, &target)?;
        let reserved = authority.snapshot()?;
        let parent_inode = fs::metadata(&parent)?.ino();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;
        assert!(matches!(
            permit.create_staging(),
            Err(quanta_index_core::CoreError::InvalidContract(message))
                if message.contains("insecure identity, mode or device")
        ));
        assert_eq!(fs::metadata(&parent)?.ino(), parent_inode);
        assert!(!staging.exists() && !target.exists());
        assert_eq!(authority.snapshot()?, reserved);

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        let mut writer = permit.create_staging()?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;
        assert!(permit.rename_staging(&writer).is_err());
        assert!(staging.exists() && !target.exists());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        permit.rename_staging(&writer)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;
        assert!(permit.sync_promoted_parents().is_err());
        assert!(!staging.exists() && target.exists());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        permit.sync_promoted_parents()?;
        permit.committed(writer, 4)?;
        drop(permit);
        drop(held);
        assert_eq!(fs::read(&target)?, b"four");
        let after = authority.snapshot()?;
        assert_eq!(after.used_bytes, baseline.used_bytes + 4);
        assert_eq!(after.used_files, baseline.used_files + 1);
        assert_eq!(after.uncertain_pending, baseline.uncertain_pending);
        assert!(binder.is_quiescent()?);

        // Source upload and RepoMap use these hooks around their own held-FD
        // create and rename syscalls rather than create_staging/rename_staging.
        let external_held = binder.reserve_publication_allowance(4, 1)?;
        let external_request =
            PublicationOutputReservation::new(PublicationOutputClass::Control, 4, 1)?;
        let mut external =
            sidecar_output_permit(deletions, Arc::clone(&external_held), external_request)?;
        let external_staging = parent.join("external-staging");
        let external_target = parent.join("external-target");
        external.bind_output(&external_staging, &external_target)?;
        let external_reserved = authority.snapshot()?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;
        assert!(matches!(
            external.creation_started(),
            Err(quanta_index_core::CoreError::InvalidContract(message))
                if message.contains("insecure identity, mode or device")
        ));
        assert_eq!(authority.snapshot()?, external_reserved);
        assert!(!external_staging.exists() && !external_target.exists());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        external.creation_started()?;
        let parent_fd = File::open(&parent)?;
        let mut external_writer = File::from(openat(
            &parent_fd,
            Path::new("external-staging"),
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        external.created(&external_writer, &external_staging)?;
        external_writer.write_all(b"next")?;
        external_writer.sync_all()?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;
        assert!(matches!(
            external.before_replace(),
            Err(quanta_index_core::CoreError::InvalidContract(message))
                if message.contains("insecure identity, mode or device")
        ));
        assert!(external_staging.exists() && !external_target.exists());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        external.before_replace()?;
        renameat_with(
            &parent_fd,
            Path::new("external-staging"),
            &parent_fd,
            Path::new("external-target"),
            RenameFlags::NOREPLACE,
        )?;
        external.sync_promoted_parents()?;
        external.committed(external_writer, 4)?;
        drop(external);
        drop(external_held);
        assert_eq!(fs::read(&external_target)?, b"next");
        let after_external = authority.snapshot()?;
        assert_eq!(after_external.used_bytes, baseline.used_bytes + 8);
        assert_eq!(after_external.used_files, baseline.used_files + 2);
        assert_eq!(after_external.uncertain_pending, baseline.uncertain_pending);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    fn committed_delete_refuses_insecure_parent_and_retries_owned_ticket()
    -> Result<(), Box<dyn Error>> {
        let (_directory, root, _lease, authority, binder, deletions) = fixture()?;
        let parent = root.join("delete-parent");
        fs::create_dir(&parent)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 4, 1)?;
        let mut permit = sidecar_output_permit(deletions, Arc::clone(&held), request)?;
        let staging = parent.join("staging");
        let target = parent.join("target");
        permit.bind_output(&staging, &target)?;
        let mut writer = permit.create_staging()?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        permit.rename_staging(&writer)?;
        permit.sync_promoted_parents()?;
        permit.committed(writer, 4)?;
        let committed = authority.snapshot()?;

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;
        assert!(matches!(
            permit.delete_file_and_sync(),
            Err(quanta_index_core::CoreError::InvalidContract(message))
                if message.contains("insecure identity, mode or device")
        ));
        assert_eq!(fs::read(&target)?, b"four");
        assert_eq!(authority.snapshot()?, committed);

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        permit.delete_file_and_sync()?;
        drop(permit);
        drop(held);
        assert!(!target.exists() && !staging.exists());
        assert_eq!(authority.snapshot()?, baseline);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    fn anchored_staging_cancel_refuses_parent_swap_without_removing_replacement()
    -> Result<(), Box<dyn Error>> {
        let (_directory, root, _lease, authority, binder, deletions) = fixture()?;
        let parent_path = root.join("anchored-parent");
        let displaced = root.join("anchored-parent-displaced");
        fs::create_dir(&parent_path)?;
        fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o700))?;
        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Lexical, 4, 1)?;
        let mut permit = sidecar_output_permit(Arc::clone(&deletions), Arc::clone(&held), request)?;
        let staging = parent_path.join("staging");
        let target = parent_path.join("target");
        permit.bind_output(&staging, &target)?;
        let mut writer = permit.create_staging()?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        fs::rename(&parent_path, &displaced)?;
        fs::create_dir(&parent_path)?;
        fs::set_permissions(&parent_path, fs::Permissions::from_mode(0o700))?;
        fs::write(&staging, b"other")?;
        assert!(permit.rename_staging(&writer).is_err());
        permit.cancel_rejected_created(writer)?;
        assert_eq!(fs::read(&staging)?, b"other");
        assert!(!displaced.join("staging").exists());
        assert!(!target.exists());
        drop(permit);
        drop(held);
        assert_eq!(authority.snapshot()?, baseline);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    fn anchored_committed_move_retries_post_rename_parent_refusal() -> Result<(), Box<dyn Error>> {
        let (_directory, root, _lease, authority, binder, deletions) = fixture()?;
        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 4, 1)?;
        let mut permit = sidecar_output_permit(Arc::clone(&deletions), Arc::clone(&held), request)?;
        let staging = root.join("move-staging");
        let first = root.join("move-first");
        let parent = root.join("move-parent");
        let second = parent.join("move-second");
        permit.bind_output(&staging, &first)?;
        let mut writer = permit.create_staging()?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        let original = writer.metadata()?;
        permit.rename_staging(&writer)?;
        permit.sync_promoted_parents()?;
        permit.committed(writer, 4)?;
        permit.move_output(&second)?;
        let held_move = authority.snapshot()?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;
        assert!(matches!(
            permit.renamed_and_synced(),
            Err(quanta_index_core::CoreError::InvalidContract(message))
                if message.contains("insecure identity, mode or device")
        ));
        assert!(!first.exists());
        assert_eq!(fs::metadata(&second)?.ino(), original.ino());
        assert_eq!(authority.snapshot()?, held_move);

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        permit.renamed_and_synced()?;
        assert!(!first.exists() && !staging.exists());
        assert_eq!(fs::read(&second)?, b"four");
        let moved = fs::metadata(&second)?;
        assert_eq!((moved.dev(), moved.ino()), (original.dev(), original.ino()));
        drop(permit);
        drop(held);
        let after = authority.snapshot()?;
        assert_eq!(after.used_bytes, baseline.used_bytes + 4);
        assert_eq!(after.used_files, baseline.used_files + 1);
        assert_eq!(after.uncertain_pending, baseline.uncertain_pending);
        assert!(binder.is_quiescent()?);
        Ok(())
    }

    #[test]
    fn anchored_move_refuses_same_inode_parent_chmod_before_rename_and_retries_ticket()
    -> Result<(), Box<dyn Error>> {
        let (_directory, root, _lease, authority, binder, deletions) = fixture()?;
        let baseline = authority.snapshot()?;
        let held = binder.reserve_publication_allowance(4, 1)?;
        let request = PublicationOutputReservation::new(PublicationOutputClass::Semantic, 4, 1)?;
        let mut permit = sidecar_output_permit(deletions, Arc::clone(&held), request)?;
        let staging = root.join("chmod-move-staging");
        let first = root.join("chmod-move-first");
        let parent = root.join("chmod-move-parent");
        let second = parent.join("chmod-move-second");
        permit.bind_output(&staging, &first)?;
        let mut writer = permit.create_staging()?;
        permit.created(&writer, &staging)?;
        writer.write_all(b"four")?;
        writer.sync_all()?;
        permit.rename_staging(&writer)?;
        permit.sync_promoted_parents()?;
        permit.committed(writer, 4)?;

        fs::create_dir(&parent)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        fs::create_dir(&second)?;
        assert!(matches!(
            permit.move_output(&second),
            Err(quanta_index_core::CoreError::InvalidContract(message))
                if message.contains("not a regular file")
        ));
        assert!(first.exists() && second.is_dir());
        fs::remove_dir(&second)?;
        let mut ticket = binder.begin_sidecar_move(&first, &second)?;
        let reserved = authority.snapshot()?;
        let parent_inode = fs::metadata(&parent)?.ino();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o777))?;
        assert!(matches!(
            ticket.perform_change(),
            Err(quanta_index_core::CoreError::InvalidContract(message))
                if message.contains("insecure identity, mode or device")
        ));
        assert_eq!(fs::metadata(&parent)?.ino(), parent_inode);
        assert!(first.exists() && !second.exists());
        assert_eq!(authority.snapshot()?, reserved);

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        ticket.perform_change()?;
        ticket.changed_and_synced()?;
        assert!(!first.exists());
        assert_eq!(fs::read(&second)?, b"four");
        drop(ticket);
        drop(permit);
        drop(held);
        let after = authority.snapshot()?;
        assert_eq!(after.used_bytes, baseline.used_bytes + 4);
        assert_eq!(after.used_files, baseline.used_files + 1);
        assert_eq!(after.uncertain_pending, baseline.uncertain_pending);
        assert!(binder.is_quiescent()?);
        Ok(())
    }
}
