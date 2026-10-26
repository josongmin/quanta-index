//! One GC candidate's physical delete custody across bounded sweep steps.

use std::fs::File;
use std::path::Path;

use quanta_index_core::CoreError;
use quanta_index_core::pool_write_admission::PoolWriteAdmissionPort;
use quanta_index_core::publication_admission::PublicationObjectDeleteTicket;
use rustix::fs::{AtFlags, statat, unlinkat};

use super::{hex_digest, storage};

/// Stored in the existing GC progress before the first namespace mutation.
/// A started ticket cannot be dropped on an uncertain unlink or parent sync.
pub(crate) struct GcDeleteCustody {
    digest: [u8; 32],
    bytes: u64,
    directory: File,
    ticket: Option<Box<dyn PublicationObjectDeleteTicket>>,
    unlink_attempted: bool,
    unlinked: bool,
    quota_pending: bool,
}

enum AttemptedUnlink {
    Absent,
    ConfirmedUnchanged,
}

impl GcDeleteCustody {
    pub(crate) fn new(
        digest: [u8; 32],
        bytes: u64,
        directory: File,
        ticket: Option<Box<dyn PublicationObjectDeleteTicket>>,
    ) -> Self {
        Self {
            digest,
            bytes,
            directory,
            ticket,
            unlink_attempted: false,
            unlinked: false,
            quota_pending: false,
        }
    }

    /// A failed unlink with independently confirmed unchanged source releases
    /// its ticket. Every post-unlink or unresolved started state stays owned.
    pub(crate) fn must_retain_after_error(&self) -> bool {
        self.unlinked
            || self
                .ticket
                .as_ref()
                .map_or(self.unlink_attempted, |ticket| {
                    ticket.has_started() && !ticket.is_settled()
                })
    }

    pub(crate) fn is_qualified(&self) -> bool {
        self.ticket.is_some()
    }

    fn inspect_attempted_unlink(&mut self, name: &str) -> Result<AttemptedUnlink, CoreError> {
        match statat(&self.directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => {
                self.unlinked = true;
                Ok(AttemptedUnlink::Absent)
            }
            Ok(_) => {
                let ticket = self.ticket.as_mut().ok_or_else(|| {
                    CoreError::Storage(
                        "prior GC unlink left an entry without unchanged-source custody".into(),
                    )
                })?;
                if !ticket.is_settled() {
                    ticket.confirmed_unchanged_after_failure()?;
                }
                Ok(AttemptedUnlink::ConfirmedUnchanged)
            }
            Err(error) => Err(storage("inspect uncertain GC unlink", error)),
        }
    }

    pub(crate) fn delete_and_settle(
        &mut self,
        quota: Option<&dyn PoolWriteAdmissionPort>,
    ) -> Result<(), CoreError> {
        let name = hex_digest(&self.digest);
        if !self.unlinked {
            if self.unlink_attempted {
                match self.inspect_attempted_unlink(&name)? {
                    AttemptedUnlink::Absent => {}
                    AttemptedUnlink::ConfirmedUnchanged => {
                        return Err(CoreError::Storage(
                            "prior GC unlink failed with the original source unchanged".into(),
                        ));
                    }
                }
            }
            if !self.unlinked {
                if let Some(ticket) = self.ticket.as_mut()
                    && !ticket.has_started()
                {
                    ticket.deletion_started()?;
                }
                self.unlink_attempted = true;
                if let Err(error) = unlinkat(&self.directory, Path::new(&name), AtFlags::empty()) {
                    // An interrupted syscall may already have removed the
                    // entry. Keep the attempt state before returning so no
                    // retry can unlink a replacement at this pathname.
                    return match self.inspect_attempted_unlink(&name) {
                        Ok(_) => Err(storage("delete unmarked object", error)),
                        Err(proof) => Err(CoreError::Storage(format!(
                            "delete unmarked object: {error}; outcome proof: {proof}"
                        ))),
                    };
                }
                self.unlinked = true;
            }
        }
        if let Some(quota) = quota
            && !self.quota_pending
        {
            quota.canonical_unlinked_pending_sync(self.digest, self.bytes)?;
            self.quota_pending = true;
        }
        if self
            .ticket
            .as_ref()
            .is_none_or(|ticket| !ticket.is_settled())
        {
            #[cfg(test)]
            crate::publication_faults::reach(
                crate::publication_faults::Cut::PoolGcDirectorySync,
                crate::publication_faults::Side::Before,
            )
            .map_err(|error| storage("sync object deletion", error))?;
            if let Some(ticket) = self.ticket.as_mut() {
                ticket.unlinked_and_synced()?;
            } else {
                self.directory
                    .sync_all()
                    .map_err(|error| storage("sync object deletion", error))?;
            }
        }
        if let Some(quota) = quota {
            quota.confirmed_canonical_unlink(self.digest, self.bytes)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::error::Error;
    use std::fs::{self, File, Metadata};
    use std::os::unix::fs::MetadataExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use quanta_index_core::CoreError;
    use quanta_index_core::publication_admission::PublicationObjectDeleteTicket;

    use super::{GcDeleteCustody, hex_digest};
    use crate::publication_faults::{Cut, Side, with_error_at};

    struct FakeDeleteTicket {
        path: PathBuf,
        source: Option<File>,
        parent: File,
        started: bool,
        settled: bool,
        abandoned: Arc<AtomicUsize>,
    }

    pub(crate) fn fake_gc_delete_ticket(
        path: &Path,
    ) -> Result<Box<dyn PublicationObjectDeleteTicket>, CoreError> {
        let parent = path
            .parent()
            .ok_or_else(|| CoreError::InvalidContract("test GC delete parent absent".into()))?;
        let source = File::open(path).map_err(|error| CoreError::Storage(error.to_string()))?;
        let parent = File::open(parent).map_err(|error| CoreError::Storage(error.to_string()))?;
        Ok(Box::new(FakeDeleteTicket {
            path: path.to_owned(),
            source: Some(source),
            parent,
            started: false,
            settled: false,
            abandoned: Arc::new(AtomicUsize::new(0)),
        }))
    }

    impl PublicationObjectDeleteTicket for FakeDeleteTicket {
        fn source_metadata(&self) -> Result<(Metadata, Metadata), CoreError> {
            let source = self.source.as_ref().ok_or_else(|| {
                CoreError::InvalidContract("test source FD already retired".into())
            })?;
            Ok((
                source
                    .metadata()
                    .map_err(|error| CoreError::Storage(error.to_string()))?,
                self.parent
                    .metadata()
                    .map_err(|error| CoreError::Storage(error.to_string()))?,
            ))
        }

        fn deletion_started(&mut self) -> Result<(), CoreError> {
            if self.started || !self.path.exists() {
                return Err(CoreError::InvalidContract(
                    "test delete start differs".into(),
                ));
            }
            self.started = true;
            Ok(())
        }

        fn unlinked_and_synced(&mut self) -> Result<(), CoreError> {
            if !self.started || self.path.exists() {
                return Err(CoreError::InvalidContract(
                    "test unlink absent proof differs".into(),
                ));
            }
            self.parent
                .sync_all()
                .map_err(|error| CoreError::Storage(error.to_string()))?;
            let _retired = self.source.take().ok_or_else(|| {
                CoreError::InvalidContract("test source FD already retired".into())
            })?;
            self.settled = true;
            Ok(())
        }

        fn confirmed_unchanged_after_failure(&mut self) -> Result<(), CoreError> {
            if !self.started || !self.path.exists() {
                return Err(CoreError::InvalidContract(
                    "test unchanged proof differs".into(),
                ));
            }
            let source = self.source_metadata()?.0;
            let current = fs::symlink_metadata(&self.path)
                .map_err(|error| CoreError::Storage(error.to_string()))?;
            if (source.dev(), source.ino()) != (current.dev(), current.ino()) {
                return Err(CoreError::InvalidContract(
                    "test unchanged inode differs".into(),
                ));
            }
            self.parent
                .sync_all()
                .map_err(|error| CoreError::Storage(error.to_string()))?;
            let _retired = self.source.take().ok_or_else(|| {
                CoreError::InvalidContract("test source FD already retired".into())
            })?;
            self.settled = true;
            Ok(())
        }

        fn has_started(&self) -> bool {
            self.started
        }

        fn is_settled(&self) -> bool {
            self.settled
        }
    }

    impl Drop for FakeDeleteTicket {
        fn drop(&mut self) {
            if self.started && !self.settled {
                let _prior = self.abandoned.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    fn require(condition: bool, reason: &str) -> Result<(), Box<dyn Error>> {
        if condition {
            Ok(())
        } else {
            Err(reason.to_owned().into())
        }
    }

    fn absent(path: &Path) -> Result<bool, Box<dyn Error>> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Ok(_) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    #[test]
    fn failed_parent_sync_keeps_started_ticket_until_same_slot_retry() -> Result<(), Box<dyn Error>>
    {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("objects");
        fs::create_dir(&directory)?;
        let digest = [0xab; 32];
        let path = directory.join(hex_digest(&digest));
        fs::write(&path, b"charged object")?;
        let abandoned = Arc::new(AtomicUsize::new(0));
        let ticket: Box<dyn PublicationObjectDeleteTicket> = Box::new(FakeDeleteTicket {
            path: path.clone(),
            source: Some(File::open(&path)?),
            parent: File::open(&directory)?,
            started: false,
            settled: false,
            abandoned: Arc::clone(&abandoned),
        });
        let mut pending = GcDeleteCustody::new(digest, 14, File::open(&directory)?, Some(ticket));
        let (failed, fired) = with_error_at(Cut::PoolGcDirectorySync, Side::Before, 1, || {
            pending.delete_and_settle(None)
        });
        require(fired && failed.is_err(), "parent sync fault did not fire")?;
        require(absent(&path)?, "canonical entry remained after unlink")?;
        require(
            pending.must_retain_after_error(),
            "started ticket was discardable",
        )?;
        require(
            abandoned.load(Ordering::Acquire) == 0,
            "started ticket was dropped before retry",
        )?;
        pending.delete_and_settle(None)?;
        require(
            pending
                .ticket
                .as_ref()
                .is_some_and(|ticket| ticket.is_settled()),
            "retry did not settle original ticket",
        )?;
        drop(pending);
        require(
            abandoned.load(Ordering::Acquire) == 0,
            "settled ticket was abandoned",
        )
    }

    #[test]
    fn uncertain_unlink_never_deletes_a_replacement_on_retry() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("objects");
        fs::create_dir(&directory)?;
        let digest = [0xcd; 32];
        let path = directory.join(hex_digest(&digest));
        fs::write(&path, b"original object")?;
        let abandoned = Arc::new(AtomicUsize::new(0));
        let ticket: Box<dyn PublicationObjectDeleteTicket> = Box::new(FakeDeleteTicket {
            path: path.clone(),
            source: Some(File::open(&path)?),
            parent: File::open(&directory)?,
            started: false,
            settled: false,
            abandoned: Arc::clone(&abandoned),
        });
        let mut pending = GcDeleteCustody::new(digest, 15, File::open(&directory)?, Some(ticket));
        pending
            .ticket
            .as_mut()
            .ok_or("test ticket absent")?
            .deletion_started()?;
        pending.unlink_attempted = true;
        fs::remove_file(&path)?;
        fs::write(&path, b"foreign replacement")?;
        require(
            pending.delete_and_settle(None).is_err(),
            "replacement was accepted as unchanged",
        )?;
        require(
            pending.must_retain_after_error(),
            "uncertain started ticket was discarded",
        )?;
        require(
            fs::read(&path)? == b"foreign replacement",
            "retry unlinked a replacement",
        )?;
        require(
            abandoned.load(Ordering::Acquire) == 0,
            "started ticket was dropped",
        )?;
        fs::remove_file(&path)?;
        pending.delete_and_settle(None)?;
        drop(pending);
        require(
            abandoned.load(Ordering::Acquire) == 0,
            "settled ticket was abandoned",
        )
    }

    #[test]
    fn unqualified_uncertain_unlink_also_refuses_a_replacement() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("objects");
        fs::create_dir(&directory)?;
        let digest = [0xef; 32];
        let path = directory.join(hex_digest(&digest));
        fs::write(&path, b"foreign replacement")?;
        let mut pending = GcDeleteCustody::new(digest, 19, File::open(&directory)?, None);
        pending.unlink_attempted = true;
        require(
            pending.delete_and_settle(None).is_err(),
            "unqualified retry accepted a replacement",
        )?;
        require(
            pending.must_retain_after_error(),
            "unqualified ambiguous state was discarded",
        )?;
        require(
            fs::read(&path)? == b"foreign replacement",
            "unqualified retry unlinked a replacement",
        )
    }
}
