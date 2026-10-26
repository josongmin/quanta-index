//! Owned Control readers; qualified FDs remain pinned through checked retirement.

use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};

use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::PublicationControlReader;
use rustix::fs::{FlockOperation, flock};

enum Inner {
    Unqualified(File),
    Admitted(Box<dyn PublicationControlReader>),
}

pub(super) struct ControlReader {
    inner: Option<Inner>,
}

impl ControlReader {
    pub(super) fn unqualified(file: File) -> Self {
        Self {
            inner: Some(Inner::Unqualified(file)),
        }
    }

    pub(super) fn admitted(reader: Box<dyn PublicationControlReader>) -> Self {
        Self {
            inner: Some(Inner::Admitted(reader)),
        }
    }

    pub(super) fn metadata(&self) -> Result<Metadata, CoreError> {
        match self.inner.as_ref() {
            Some(Inner::Unqualified(file)) => file
                .metadata()
                .map_err(|error| CoreError::Storage(format!("stat Control reader: {error}"))),
            Some(Inner::Admitted(reader)) => reader.metadata(),
            None => Err(CoreError::InvalidContract(
                "Control reader already retired".into(),
            )),
        }
    }

    pub(super) fn lock_exclusive(&self) -> Result<(), CoreError> {
        match self.inner.as_ref() {
            Some(Inner::Unqualified(file)) => flock(file, FlockOperation::LockExclusive)
                .map_err(|error| CoreError::Storage(format!("lock Control reader: {error}"))),
            Some(Inner::Admitted(reader)) => reader.lock_exclusive(),
            None => Err(CoreError::InvalidContract(
                "Control reader already retired".into(),
            )),
        }
    }

    pub(super) fn try_lock_exclusive(&self) -> Result<bool, CoreError> {
        match self.inner.as_ref() {
            Some(Inner::Unqualified(file)) => {
                match flock(file, FlockOperation::NonBlockingLockExclusive) {
                    Ok(()) => Ok(true),
                    Err(rustix::io::Errno::WOULDBLOCK) => Ok(false),
                    Err(error) => Err(CoreError::Storage(format!(
                        "try lock Control reader: {error}"
                    ))),
                }
            }
            Some(Inner::Admitted(reader)) => reader.try_lock_exclusive(),
            None => Err(CoreError::InvalidContract(
                "Control reader already retired".into(),
            )),
        }
    }

    pub(super) fn retire(mut self) -> Result<(), CoreError> {
        match self.inner.take() {
            Some(Inner::Unqualified(file)) => {
                drop(file);
                Ok(())
            }
            Some(Inner::Admitted(reader)) => reader.retire(),
            None => Ok(()),
        }
    }
}

impl Read for ControlReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self.inner.as_mut() {
            Some(Inner::Unqualified(file)) => file.read(bytes),
            Some(Inner::Admitted(reader)) => reader.read(bytes),
            None => Err(io::Error::other("Control reader already retired")),
        }
    }
}

impl Seek for ControlReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match self.inner.as_mut() {
            Some(Inner::Unqualified(file)) => file.seek(position),
            Some(Inner::Admitted(reader)) => reader.seek(position),
            None => Err(io::Error::other("Control reader already retired")),
        }
    }
}

impl Drop for ControlReader {
    fn drop(&mut self) {
        if let Some(Inner::Admitted(reader)) = self.inner.take() {
            // The physical owner quarantines an ambiguous close. Callers that
            // can return a result use `retire` and preserve its error.
            let _retirement = reader.retire();
        }
    }
}
