//! Physical reclamation reuses the common file authority after Catalog fencing.

use std::{
    fmt,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{
    CoreError,
    index::{IndexError, ObjectUnlinkPort, PhysicalIncarnation},
};

use crate::publication_file_capacity::{DeleteTicket, PublicationFileCapacity};

pub struct NativeObjectReclaimIo {
    common: Arc<PublicationFileCapacity>,
    // One retained actual operation; no second extent ledger or GC scheduler.
    pending: Mutex<Option<PendingDelete>>,
}

struct PendingDelete {
    path: PathBuf,
    expected: PhysicalIncarnation,
    ticket: DeleteTicket,
}

impl fmt::Debug for NativeObjectReclaimIo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeObjectReclaimIo")
            .finish_non_exhaustive()
    }
}

impl NativeObjectReclaimIo {
    #[must_use]
    pub fn new(common: Arc<PublicationFileCapacity>) -> Self {
        Self {
            common,
            pending: Mutex::new(None),
        }
    }
}

fn index_error(error: &CoreError) -> IndexError {
    if matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        }
    ) {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("native object physical reclaim: {error}"))
    }
}

impl ObjectUnlinkPort for NativeObjectReclaimIo {
    fn unlink_and_sync(
        &self,
        path: &Path,
        expected: &PhysicalIncarnation,
    ) -> Result<(), IndexError> {
        let path_bytes = path.as_os_str().len();
        if path_bytes > 4096 {
            return Err(IndexError::ResourceLimited);
        }
        let mut retained_path = std::ffi::OsString::new();
        retained_path
            .try_reserve_exact(path_bytes)
            .map_err(|_error| IndexError::ResourceLimited)?;
        retained_path.push(path);
        let retained_path = PathBuf::from(retained_path);
        let mut pending = self
            .pending
            .lock()
            .map_err(|_error| IndexError::Storage("physical delete owner poisoned".into()))?;
        if let Some(prior) = pending.as_mut() {
            if prior.path != path || prior.expected != *expected {
                return Err(IndexError::ResourceLimited);
            }
            prior
                .ticket
                .confirmed_unchanged_after_failure()
                .map_err(|error| index_error(&error))?;
            let _retired = pending.take();
        }
        let mut ticket = self
            .common
            .begin_delete(path)
            .map_err(|error| index_error(&error))?;
        let (source, parent) = ticket
            .source_metadata()
            .map_err(|error| index_error(&error))?;
        let changed_nanos = i128::from(source.ctime())
            .checked_mul(1_000_000_000)
            .and_then(|nanos| nanos.checked_add(i128::from(source.ctime_nsec())))
            .ok_or(IndexError::ResourceLimited)?;
        if source.dev() != expected.device
            || source.ino() != expected.inode
            || source.len() != expected.bytes
            || source.nlink() != 1
            || changed_nanos != expected.changed_nanos
            || parent.dev() != expected.shard_device
            || parent.ino() != expected.shard_inode
        {
            return Err(IndexError::FenceLost);
        }
        if let Err(error) = ticket.perform_and_sync() {
            if ticket.has_started() {
                *pending = Some(PendingDelete {
                    path: retained_path,
                    expected: *expected,
                    ticket,
                });
            }
            drop(pending);
            return Err(index_error(&error));
        }
        drop(pending);
        Ok(())
    }

    fn confirm_absence_and_sync(
        &self,
        path: &Path,
        expected: &PhysicalIncarnation,
    ) -> Result<(), IndexError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_error| IndexError::Storage("physical delete owner poisoned".into()))?;
        if let Some(prior) = pending.as_mut() {
            if prior.path != path || prior.expected != *expected {
                return Err(IndexError::FenceLost);
            }
            if prior.ticket.is_settled() {
                self.common
                    .sweep_retired()
                    .map_err(|error| index_error(&error))?;
            } else {
                prior
                    .ticket
                    .unlinked_and_synced()
                    .map_err(|error| index_error(&error))?;
            }
            let _retired = pending.take();
            drop(pending);
            return Ok(());
        }
        let synced = self
            .common
            .sync_confirmed_absence(
                path,
                quanta_index_core::publication_capacity::InodeKey {
                    device: expected.shard_device,
                    inode: expected.shard_inode,
                },
            )
            .map_err(|error| index_error(&error));
        drop(pending);
        synced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publication_inventory::{PublicationBootInventoryLimits, seed_publication_capacity};
    use quanta_index_core::{
        RequestBudgetV1, index::ObjectNamespaceBinding, publication_capacity::PublicationCapacity,
    };
    use quanta_index_searchd::app::runtime::StateRootLease;
    use std::{
        fs::{self, File},
        os::unix::fs::PermissionsExt as _,
        path::PathBuf,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    type Fixture = (
        tempfile::TempDir,
        PathBuf,
        PublicationCapacity,
        Arc<PublicationFileCapacity>,
        PhysicalIncarnation,
    );
    fn require(value: bool, message: &str) -> TestResult {
        if value {
            Ok(())
        } else {
            Err(std::io::Error::other(message).into())
        }
    }
    fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&directory.path().join("state"))?;
        let root = lease.state_root_identity_v1().to_path_buf();
        let path = root.join("physical-object");
        fs::write(&path, b"physical-bytes")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let source = fs::metadata(&path)?;
        let parent = fs::metadata(&root)?;
        // This facet owns source/parent I/O, not Catalog namespace permission.
        // The store-side laws above require the real managed permit separately.
        let expected = PhysicalIncarnation {
            namespace: ObjectNamespaceBinding {
                root_device: 0,
                root_inode: 0,
                objects_device: 0,
                objects_inode: 0,
                lease_device: 0,
                lease_inode: 0,
            },
            shard_device: parent.dev(),
            shard_inode: parent.ino(),
            device: source.dev(),
            inode: source.ino(),
            bytes: 14,
            changed_nanos: i128::from(source.ctime())
                .checked_mul(1_000_000_000)
                .and_then(|nanos| nanos.checked_add(i128::from(source.ctime_nsec())))
                .ok_or(IndexError::ResourceLimited)?,
        };
        let authority = seed_publication_capacity(
            &lease,
            &root,
            PublicationBootInventoryLimits {
                max_bytes: 1024 * 1024,
                max_files: 64,
                max_pending: 128,
                max_entries: 256,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &RequestBudgetV1::unbounded(),
        )?;
        let common = PublicationFileCapacity::new(lease, authority.clone(), 32, 256, 256)?;
        Ok((directory, path, authority, common, expected))
    }

    #[test]
    fn physical_unlink_keeps_extent_until_last_actual_reader_close() -> TestResult {
        let (_directory, path, authority, common, expected) = fixture()?;
        let baseline = authority.snapshot()?;
        let opening = common.begin_reader_open(&path)?;
        let reader = opening.opened_owned(File::open(&path)?)?;
        NativeObjectReclaimIo::new(Arc::clone(&common)).unlink_and_sync(&path, &expected)?;
        require(!path.exists(), "physical object survived successful unlink")?;
        require(
            authority.snapshot()?.used_bytes == baseline.used_bytes,
            "live reader refunded an extent",
        )?;
        require(!common.is_quiescent()?, "live reader lost custody")?;
        reader.retire()?;
        common.sweep_retired()?;
        let after = authority.snapshot()?;
        require(
            after.used_bytes + 14 == baseline.used_bytes
                && after.used_files + 1 == baseline.used_files,
            "final actual reader close did not refund exactly one object",
        )?;
        require(
            common.is_quiescent()?,
            "confirmed reclaim retained an operation",
        )?;
        Ok(())
    }

    #[test]
    fn physical_incarnation_mismatch_refuses_before_unlink_and_releases_admission() -> TestResult {
        let (_directory, path, authority, common, expected) = fixture()?;
        let baseline = authority.snapshot()?;
        for mismatch in 0..3 {
            let mut wrong = expected;
            match mismatch {
                0 => wrong.changed_nanos += 1,
                1 => wrong.shard_inode = 0,
                _ => wrong.bytes = 13,
            }
            require(
                matches!(
                    NativeObjectReclaimIo::new(Arc::clone(&common)).unlink_and_sync(&path, &wrong),
                    Err(IndexError::FenceLost)
                ),
                "wrong physical incarnation was accepted",
            )?;
            require(
                fs::read(&path)? == b"physical-bytes",
                "refusal changed independent byte golden",
            )?;
            let after = authority.snapshot()?;
            require(
                after.used_bytes == baseline.used_bytes
                    && after.used_files == baseline.used_files
                    && common.is_quiescent()?,
                "refusal leaked or refunded physical admission",
            )?;
        }
        Ok(())
    }

    #[test]
    fn interrupted_unlink_keeps_actual_ticket_until_matching_sync_replay() -> TestResult {
        let (_directory, path, authority, common, expected) = fixture()?;
        let baseline = authority.snapshot()?;
        let io = NativeObjectReclaimIo::new(Arc::clone(&common));
        let mut ticket = common.begin_delete(&path)?;
        ticket.deletion_started()?;
        // Actual unlink prefix, deliberately interrupted before parent sync.
        // This is not an injected failing fsync syscall.
        fs::remove_file(&path)?;
        *io.pending
            .lock()
            .map_err(|_error| std::io::Error::other("test delete owner poisoned"))? =
            Some(PendingDelete {
                path: path.clone(),
                expected,
                ticket,
            });
        require(
            authority.snapshot()?.used_bytes == baseline.used_bytes,
            "interrupted unlink refunded its still-owned source",
        )?;
        require(
            common
                .sync_confirmed_absence(
                    &path,
                    quanta_index_core::publication_capacity::InodeKey {
                        device: expected.shard_device,
                        inode: expected.shard_inode,
                    },
                )
                .is_err(),
            "absence bypassed an owned physical operation",
        )?;
        let mut wrong = expected;
        wrong.bytes = 13;
        require(
            matches!(
                io.confirm_absence_and_sync(&path, &wrong),
                Err(IndexError::FenceLost)
            ),
            "wrong incarnation consumed the pending physical delete",
        )?;
        require(
            authority.snapshot()?.used_bytes == baseline.used_bytes,
            "failed replay refunded pending source custody",
        )?;
        io.confirm_absence_and_sync(&path, &expected)?;
        let after = authority.snapshot()?;
        require(
            after.used_bytes.checked_add(14) == Some(baseline.used_bytes)
                && after.used_files.checked_add(1) == Some(baseline.used_files)
                && common.is_quiescent()?,
            "matching replay did not retire exact physical debt",
        )?;
        io.confirm_absence_and_sync(&path, &expected)?;
        require(
            authority.snapshot()?.used_bytes == after.used_bytes,
            "already-confirmed absence refunded twice",
        )?;
        Ok(())
    }

    #[test]
    fn settled_delete_replay_never_requires_an_already_closed_source_fd() -> TestResult {
        let (_directory, path, authority, common, expected) = fixture()?;
        let baseline = authority.snapshot()?;
        let io = NativeObjectReclaimIo::new(Arc::clone(&common));
        let mut ticket = common.begin_delete(&path)?;
        ticket.perform_and_sync()?;
        // A settled ticket may survive a subsequent retirement-sweep error.
        // Replay must retry sweep rather than demand its closed original File.
        *io.pending
            .lock()
            .map_err(|_error| std::io::Error::other("test delete owner poisoned"))? =
            Some(PendingDelete {
                path: path.clone(),
                expected,
                ticket,
            });
        io.confirm_absence_and_sync(&path, &expected)?;
        let after = authority.snapshot()?;
        require(
            after.used_bytes.checked_add(14) == Some(baseline.used_bytes)
                && after.used_files.checked_add(1) == Some(baseline.used_files)
                && common.is_quiescent()?,
            "settled replay duplicated debt or required a closed FD",
        )?;
        io.confirm_absence_and_sync(&path, &expected)?;
        require(
            authority.snapshot()?.used_bytes == after.used_bytes,
            "settled replay refunded twice",
        )?;
        Ok(())
    }
}
