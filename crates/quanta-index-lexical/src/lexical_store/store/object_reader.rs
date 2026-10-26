//! Original object descriptors held through the physical reader lifetime.

use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};

use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::PublicationObjectReader;

pub(super) enum ObjectReader {
    Unqualified(File),
    Admitted(Box<dyn PublicationObjectReader>),
}

impl ObjectReader {
    pub(super) fn metadata(&self) -> Result<Metadata, CoreError> {
        match self {
            Self::Unqualified(file) => file
                .metadata()
                .map_err(|error| super::storage("stat referenced object", error)),
            Self::Admitted(reader) => reader.metadata(),
        }
    }

    pub(super) fn sync_posix(&self) -> Result<(), CoreError> {
        match self {
            Self::Unqualified(file) => rustix::fs::fsync(file)
                .map_err(|error| super::storage("sync adopted object", error)),
            Self::Admitted(reader) => reader.sync_posix(),
        }
    }

    pub(super) fn retire(self) -> Result<(), CoreError> {
        match self {
            Self::Unqualified(file) => {
                drop(file);
                Ok(())
            }
            Self::Admitted(reader) => reader.retire(),
        }
    }
}

impl Read for ObjectReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Unqualified(file) => file.read(bytes),
            Self::Admitted(reader) => reader.read(bytes),
        }
    }
}

impl Seek for ObjectReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Unqualified(file) => file.seek(position),
            Self::Admitted(reader) => reader.seek(position),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd as _;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use quanta_index_core::publication_admission::{
        PublicationControlOpenTicket, PublicationControlReader, PublicationObjectDeleteTicket,
        PublicationObjectLifetimePort, PublicationObjectOpenTicket, PublicationObjectReader,
    };
    use quanta_index_core::publication_capacity::PublicationAllowance;

    use super::super::{
        EPOCH_FILE, GATE_FILE, OBJECTS_DIR, ObjectKind, ObjectStore, POOL_DIR, hex_digest,
    };
    use super::*;

    struct TestLifetime {
        root: PathBuf,
        opens: Arc<AtomicUsize>,
        retires: Arc<AtomicUsize>,
        control_read_fault: Option<ControlReadFault>,
    }

    #[derive(Clone, Copy)]
    enum ControlReadFault {
        TypedBudget,
        NestedTypedBudget,
        Enospc,
        BudgetLookalike,
    }

    impl ControlReadFault {
        fn io_error(self) -> io::Error {
            match self {
                Self::TypedBudget => io::Error::other(CoreError::Typed {
                    code: quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                    message: "injected Control reader refusal".into(),
                }),
                Self::NestedTypedBudget => io::Error::other(io::Error::other(CoreError::Typed {
                    code: quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE,
                    message: "nested Control reader refusal".into(),
                })),
                Self::Enospc => {
                    io::Error::from_raw_os_error(rustix::io::Errno::NOSPC.raw_os_error())
                }
                Self::BudgetLookalike => {
                    io::Error::other("IngestResourceBudgetExceeded injected Control reader refusal")
                }
            }
        }
    }

    struct TestOpening {
        retires: Arc<AtomicUsize>,
    }

    struct TestReader {
        file: File,
        original_fd: std::os::fd::RawFd,
        retires: Arc<AtomicUsize>,
    }

    struct TestControlOpening {
        retires: Arc<AtomicUsize>,
        read_fault: Option<ControlReadFault>,
    }

    struct TestControlReader {
        file: File,
        original_fd: std::os::fd::RawFd,
        retires: Arc<AtomicUsize>,
        read_fault: Option<ControlReadFault>,
    }

    impl Read for TestControlReader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if let Some(fault) = self.read_fault {
                return Err(fault.io_error());
            }
            self.file.read(bytes)
        }
    }

    impl Seek for TestControlReader {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.file.seek(position)
        }
    }

    impl PublicationControlReader for TestControlReader {
        fn metadata(&self) -> Result<Metadata, CoreError> {
            self.file
                .metadata()
                .map_err(|error| super::super::storage("test Control metadata", error))
        }

        fn sync_posix(&self) -> Result<(), CoreError> {
            rustix::fs::fsync(&self.file)
                .map_err(|error| super::super::storage("test Control sync", error))
        }

        fn lock_exclusive(&self) -> Result<(), CoreError> {
            rustix::fs::flock(&self.file, rustix::fs::FlockOperation::LockExclusive)
                .map_err(|error| super::super::storage("test Control lock", error))
        }

        fn try_lock_exclusive(&self) -> Result<bool, CoreError> {
            match rustix::fs::flock(
                &self.file,
                rustix::fs::FlockOperation::NonBlockingLockExclusive,
            ) {
                Ok(()) => Ok(true),
                Err(rustix::io::Errno::WOULDBLOCK) => Ok(false),
                Err(error) => Err(super::super::storage("test Control try-lock", error)),
            }
        }

        fn retire(self: Box<Self>) -> Result<(), CoreError> {
            let Self {
                file,
                original_fd,
                retires,
                read_fault: _,
            } = *self;
            if file.as_raw_fd() != original_fd {
                return Err(super::super::invalid(
                    "test Control reader lost original FD",
                ));
            }
            rusqlite::ffi::quanta_capacity_vfs::close_owned_file(file).map_err(|error| {
                CoreError::storage_io("test Control reader checked close", &error)
            })?;
            let _previous = retires.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    impl PublicationControlOpenTicket for TestControlOpening {
        fn opened_owned(
            self: Box<Self>,
            file: File,
        ) -> Result<Box<dyn PublicationControlReader>, CoreError> {
            Ok(Box::new(TestControlReader {
                original_fd: file.as_raw_fd(),
                file,
                retires: Arc::clone(&self.retires),
                read_fault: self.read_fault,
            }))
        }
    }

    impl Read for TestReader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.file.read(bytes)
        }
    }

    impl Seek for TestReader {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.file.seek(position)
        }
    }

    impl PublicationObjectReader for TestReader {
        fn metadata(&self) -> Result<Metadata, CoreError> {
            self.file
                .metadata()
                .map_err(|error| super::super::storage("test reader metadata", error))
        }

        fn sync_posix(&self) -> Result<(), CoreError> {
            rustix::fs::fsync(&self.file)
                .map_err(|error| super::super::storage("test reader sync", error))
        }

        fn retire(self: Box<Self>) -> Result<(), CoreError> {
            let Self {
                file,
                original_fd,
                retires,
            } = *self;
            if file.as_raw_fd() != original_fd {
                return Err(super::super::invalid("test reader lost original FD"));
            }
            drop(file);
            let _previous = retires.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    impl PublicationObjectOpenTicket for TestOpening {
        fn opened_owned(
            self: Box<Self>,
            file: File,
        ) -> Result<Box<dyn PublicationObjectReader>, CoreError> {
            let original_fd = file.as_raw_fd();
            Ok(Box::new(TestReader {
                file,
                original_fd,
                retires: Arc::clone(&self.retires),
            }))
        }
    }

    impl PublicationObjectLifetimePort for TestLifetime {
        fn validate_state_root(&self, root: &Path) -> Result<(), CoreError> {
            if root == self.root {
                Ok(())
            } else {
                Err(super::super::invalid("test lifetime root differs"))
            }
        }

        fn validate_parent(&self, _parent: &Arc<PublicationAllowance>) -> Result<(), CoreError> {
            Err(super::super::invalid("read-only test has no work parent"))
        }

        fn begin_control_reader_open(
            &self,
            path: &Path,
        ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError> {
            if !path.starts_with(&self.root) {
                return Err(super::super::invalid("test Control path escaped root"));
            }
            let _previous = self.opens.fetch_add(1, Ordering::AcqRel);
            Ok(Box::new(TestControlOpening {
                retires: Arc::clone(&self.retires),
                read_fault: self.control_read_fault,
            }))
        }

        fn begin_reader_open(
            &self,
            path: &Path,
        ) -> Result<Box<dyn PublicationObjectOpenTicket>, CoreError> {
            if !path.starts_with(&self.root) {
                return Err(super::super::invalid("test reader path escaped root"));
            }
            let _previous = self.opens.fetch_add(1, Ordering::AcqRel);
            Ok(Box::new(TestOpening {
                retires: Arc::clone(&self.retires),
            }))
        }

        fn begin_delete(
            &self,
            _path: &Path,
        ) -> Result<Box<dyn PublicationObjectDeleteTicket>, CoreError> {
            Err(super::super::invalid("read-only test cannot delete"))
        }
    }

    #[test]
    fn admitted_reader_retires_original_fd_after_digest_failure_and_success()
    -> Result<(), CoreError> {
        let root =
            tempfile::tempdir().map_err(|error| super::super::storage("test root", error))?;
        let store = ObjectStore::open(root.path())?;
        let gate = store.lock_gate()?;
        let reference = store.put_object_locked(ObjectKind::Record, &[], b"abc")?;
        drop(gate);
        let name = hex_digest(&reference.sha256);
        let prefix = name
            .get(..2)
            .ok_or_else(|| super::super::invalid("test digest prefix"))?;
        let path = root
            .path()
            .join(POOL_DIR)
            .join(OBJECTS_DIR)
            .join(prefix)
            .join(name);
        let original = std::fs::read(&path)
            .map_err(|error| super::super::storage("read fixed object", error))?;
        if original.len() != 24 || !original.ends_with(b"abc") {
            return Err(super::super::invalid("fixed object bytes changed"));
        }
        let mut malformed = original.clone();
        let last = malformed
            .last_mut()
            .ok_or_else(|| super::super::invalid("fixed object has no tail"))?;
        *last = b'X';
        std::fs::write(&path, malformed)
            .map_err(|error| super::super::storage("corrupt fixed object", error))?;
        let opens = Arc::new(AtomicUsize::new(0));
        let retires = Arc::new(AtomicUsize::new(0));
        let lifetime: Arc<dyn PublicationObjectLifetimePort> = Arc::new(TestLifetime {
            root: root.path().to_owned(),
            opens: Arc::clone(&opens),
            retires: Arc::clone(&retires),
            control_read_fault: None,
        });
        let admitted = store.with_object_lifetime(Some(lifetime))?;
        if !matches!(
            admitted.read_object(&reference, None),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ) {
            return Err(super::super::invalid("malformed digest was accepted"));
        }
        if opens.load(Ordering::Acquire) != 1 || retires.load(Ordering::Acquire) != 1 {
            return Err(super::super::invalid(
                "failed read did not retire actual FD",
            ));
        }
        std::fs::write(&path, original)
            .map_err(|error| super::super::storage("restore fixed object", error))?;
        let (dependencies, payload) = admitted.read_object(&reference, None)?;
        if !dependencies.is_empty() || payload != b"abc" {
            return Err(super::super::invalid("fixed object read differed"));
        }
        if opens.load(Ordering::Acquire) != 2 || retires.load(Ordering::Acquire) != 2 {
            return Err(super::super::invalid(
                "successful read did not retire actual FD",
            ));
        }
        Ok(())
    }

    #[test]
    fn cold_lifetime_refusal_precedes_any_state_root_open() -> Result<(), CoreError> {
        let directory =
            tempfile::tempdir().map_err(|error| super::super::storage("test root", error))?;
        let absent = directory.path().join("absent-state-root");
        let opens = Arc::new(AtomicUsize::new(0));
        let retires = Arc::new(AtomicUsize::new(0));
        let lifetime: Arc<dyn PublicationObjectLifetimePort> = Arc::new(TestLifetime {
            root: directory.path().to_owned(),
            opens: Arc::clone(&opens),
            retires: Arc::clone(&retires),
            control_read_fault: None,
        });
        if !matches!(
            ObjectStore::open_existing_with_lifetime(&absent, Some(lifetime)),
            Err(CoreError::InvalidContract(message)) if message.contains("test lifetime root differs")
        ) {
            return Err(super::super::invalid(
                "cold constructor opened state before authority refusal",
            ));
        }
        if opens.load(Ordering::Acquire) != 0 || retires.load(Ordering::Acquire) != 0 {
            return Err(super::super::invalid(
                "refused cold constructor claimed a reader",
            ));
        }
        Ok(())
    }

    #[test]
    fn cold_control_gate_and_epoch_keep_original_fd_until_retirement() -> Result<(), CoreError> {
        let directory =
            tempfile::tempdir().map_err(|error| super::super::storage("test root", error))?;
        drop(ObjectStore::open(directory.path())?);
        let opens = Arc::new(AtomicUsize::new(0));
        let retires = Arc::new(AtomicUsize::new(0));
        let lifetime: Arc<dyn PublicationObjectLifetimePort> = Arc::new(TestLifetime {
            root: directory.path().to_owned(),
            opens: Arc::clone(&opens),
            retires: Arc::clone(&retires),
            control_read_fault: None,
        });
        let cold = ObjectStore::open_existing_with_lifetime(directory.path(), Some(lifetime))?;
        if opens.load(Ordering::Acquire) != 2 || retires.load(Ordering::Acquire) != 2 {
            return Err(super::super::invalid(
                "cold gate or epoch missed physical reader retirement",
            ));
        }
        let reader = cold
            .open_control_reader(
                cold.pool.as_ref(),
                GATE_FILE,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::CLOEXEC
                    | rustix::fs::OFlags::NOFOLLOW,
                false,
                "test held gate",
            )?
            .ok_or_else(|| super::super::invalid("cold gate disappeared"))?;
        if opens.load(Ordering::Acquire) != 3 || retires.load(Ordering::Acquire) != 2 {
            return Err(super::super::invalid(
                "held gate reader released its original FD early",
            ));
        }
        reader.retire()?;
        if opens.load(Ordering::Acquire) != 3 || retires.load(Ordering::Acquire) != 3 {
            return Err(super::super::invalid(
                "held gate reader did not retire its original FD",
            ));
        }
        Ok(())
    }

    #[test]
    fn cold_epoch_corruption_retires_the_failed_actual_reader() -> Result<(), CoreError> {
        let directory =
            tempfile::tempdir().map_err(|error| super::super::storage("test root", error))?;
        drop(ObjectStore::open(directory.path())?);
        let epoch = directory.path().join(POOL_DIR).join(EPOCH_FILE);
        std::fs::write(&epoch, b"x")
            .map_err(|error| super::super::storage("corrupt test epoch", error))?;
        let opens = Arc::new(AtomicUsize::new(0));
        let retires = Arc::new(AtomicUsize::new(0));
        let lifetime: Arc<dyn PublicationObjectLifetimePort> = Arc::new(TestLifetime {
            root: directory.path().to_owned(),
            opens: Arc::clone(&opens),
            retires: Arc::clone(&retires),
            control_read_fault: None,
        });
        if !matches!(
            ObjectStore::open_existing_with_lifetime(directory.path(), Some(lifetime)),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                ..
            })
        ) {
            return Err(super::super::invalid("corrupt cold epoch was accepted"));
        }
        if opens.load(Ordering::Acquire) != 2 || retires.load(Ordering::Acquire) != 2 {
            return Err(super::super::invalid(
                "failed cold epoch retained an actual reader",
            ));
        }
        Ok(())
    }

    #[test]
    fn cold_epoch_read_preserves_admitted_cause_and_retires_original_fd() -> Result<(), CoreError> {
        for fault in [
            ControlReadFault::TypedBudget,
            ControlReadFault::NestedTypedBudget,
            ControlReadFault::Enospc,
            ControlReadFault::BudgetLookalike,
        ] {
            let directory =
                tempfile::tempdir().map_err(|error| super::super::storage("test root", error))?;
            drop(ObjectStore::open(directory.path())?);
            let opens = Arc::new(AtomicUsize::new(0));
            let retires = Arc::new(AtomicUsize::new(0));
            let lifetime: Arc<dyn PublicationObjectLifetimePort> = Arc::new(TestLifetime {
                root: directory.path().to_owned(),
                opens: Arc::clone(&opens),
                retires: Arc::clone(&retires),
                control_read_fault: Some(fault),
            });
            let result = ObjectStore::open_existing_with_lifetime(directory.path(), Some(lifetime));
            let correct = match fault {
                ControlReadFault::TypedBudget => matches!(result,
                    Err(CoreError::Typed { code, message })
                        if code == quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
                            && message == "injected Control reader refusal"),
                ControlReadFault::NestedTypedBudget => matches!(result,
                    Err(CoreError::Typed { code, message })
                        if code == quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE
                            && message == "nested Control reader refusal"),
                ControlReadFault::Enospc => matches!(result,
                    Err(CoreError::StorageIo {
                        kind: io::ErrorKind::StorageFull,
                        raw_os_error: Some(code),
                        ..
                    })
                        if code == rustix::io::Errno::NOSPC.raw_os_error()),
                ControlReadFault::BudgetLookalike => matches!(
                    result,
                    Err(CoreError::StorageIo {
                        kind: io::ErrorKind::Other,
                        raw_os_error: None,
                        ..
                    })
                ),
            };
            if !correct {
                return Err(super::super::invalid(
                    "cold epoch read changed admitted failure taxonomy",
                ));
            }
            if opens.load(Ordering::Acquire) != 2 || retires.load(Ordering::Acquire) != 2 {
                return Err(super::super::invalid(
                    "failed cold epoch read retained its original FD",
                ));
            }
        }
        Ok(())
    }
}
