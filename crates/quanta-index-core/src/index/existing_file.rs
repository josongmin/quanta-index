//! Bounded existing-only reads through the caller's actual physical I/O owner.
//! This module supplies DATA; it creates no publication or root authority.

use crate::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectReadIoPort, WorkBudget, WorkUsage,
};
use std::{
    fs,
    os::unix::fs::{FileExt as _, MetadataExt as _},
    path::Path,
};

/// Preserve both native admission/I/O causes and the canonical Legacy cause.
#[derive(Debug)]
pub enum ExistingFileReadFailureV1 {
    AdmissionOrIo(IndexError),
    Canonical(crate::CoreError),
}
impl From<IndexError> for ExistingFileReadFailureV1 {
    fn from(error: IndexError) -> Self {
        Self::AdmissionOrIo(error)
    }
}
impl From<crate::CoreError> for ExistingFileReadFailureV1 {
    fn from(error: crate::CoreError) -> Self {
        Self::Canonical(error)
    }
}
impl From<std::io::Error> for ExistingFileReadFailureV1 {
    fn from(error: std::io::Error) -> Self {
        Self::Canonical(crate::CoreError::storage_io(
            "existing read metadata",
            &error,
        ))
    }
}
impl std::fmt::Display for ExistingFileReadFailureV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AdmissionOrIo(error) => std::fmt::Display::fmt(error, formatter),
            Self::Canonical(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}
impl std::error::Error for ExistingFileReadFailureV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AdmissionOrIo(error) => Some(error),
            Self::Canonical(error) => Some(error),
        }
    }
}

/// Identity observed on the same admitted file descriptor as the read bytes.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ExistingFileIdentityV1(Identity);

impl ExistingFileIdentityV1 {
    #[must_use]
    pub const fn mode_v1(&self) -> u32 {
        self.0.mode
    }

    /// Reopen the expected path through its physical owner and compare it.
    /// The caller still owns root custody and must witness reader retirement.
    pub fn require_current_v1(
        &self,
        path: &Path,
        io: &dyn ObjectReadIoPort,
        budget: &mut WorkBudget,
    ) -> Result<(), ExistingFileReadFailureV1> {
        charge(budget, 1, 0)?;
        let opened = io.open(path, budget)?;
        charge(budget, 2, 0)?;
        let file = opened.file()?.metadata().map_err(storage)?;
        let current = fs::symlink_metadata(path).map_err(storage)?;
        if !file.is_file()
            || !current.is_file()
            || Identity::from(&file) != self.0
            || Identity::from(&current) != self.0
        {
            return Err(IndexError::DigestConflict.into());
        }
        drop(opened);
        budget.checkpoint()?;
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
    bytes: u64,
    links: u64,
    owner: u32,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Identity {
    fn from(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            links: metadata.nlink(),
            owner: metadata.uid(),
            mode: metadata.mode(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

fn storage(error: std::io::Error) -> ExistingFileReadFailureV1 {
    crate::CoreError::storage_io("existing admitted file read", &error).into()
}

fn charge(budget: &mut WorkBudget, reads: u64, bytes: u64) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads,
        bytes,
        ..WorkUsage::default()
    })
}

pub struct ExistingFileBytesV1 {
    bytes: Vec<u8>,
    identity: Identity,
    // Drop bytes before releasing their temporary allocation.
    _memory: MemoryLease,
}

/// Read one bounded regular file through the caller's genuine physical port.
/// The returned DATA retains a conservative decoding allocation envelope.
/// The caller must separately retain its root lease and witness port close
/// retirement before any deployment publication. No missing-file fallback is
/// provided. This function does not prove a file set or a product identity.
pub fn read_existing_file_v1(
    path: &Path,
    owner: u32,
    bound: u64,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
) -> Result<ExistingFileBytesV1, ExistingFileReadFailureV1> {
    read_existing_file_with_policy(path, owner, bound, io, budget, ReadPolicy::Sidecar)
}

/// Read a nonempty, single-link private file through the caller's physical port.
/// The returned lease covers only the input buffer. The caller admits decoding
/// and retains root custody through the port's close retirement.
pub fn read_existing_private_file_bytes_v1(
    path: &Path,
    owner: u32,
    bound: u64,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
) -> Result<ExistingFileBytesV1, ExistingFileReadFailureV1> {
    read_existing_file_with_policy(path, owner, bound, io, budget, ReadPolicy::PrivateBytes)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ReadPolicy {
    Sidecar,
    PrivateBytes,
    Committed,
}

fn read_existing_file_with_policy(
    path: &Path,
    owner: u32,
    bound: u64,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
    policy: ReadPolicy,
) -> Result<ExistingFileBytesV1, ExistingFileReadFailureV1> {
    // The supplied port must be the actual Legacy read authority. This
    // function constructs no publication capacity and never falls back to
    // an unqualified filesystem open.
    charge(budget, 1, 0)?;
    let opened = io.open(path, budget)?;
    let file = opened.file()?;
    charge(budget, 1, 0)?;
    let metadata = file.metadata().map_err(storage)?;
    let before = Identity::from(&metadata);
    if !metadata.is_file()
        || metadata.nlink() == 0
        || policy != ReadPolicy::Committed && (metadata.nlink() != 1 || metadata.len() == 0)
        || metadata.uid() != owner
        || metadata.len() > bound
    {
        return Err(IndexError::DigestConflict.into());
    }
    let length = metadata.len();
    // The recovery consumers decode bounded owner records. This conservative
    // logical envelope includes input bytes, transient DOM/CBOR nodes, typed
    // rows, Vec growth, strings and maps; consumers must use a separate grant
    // if their actual decode envelope exceeds it. It is admission accounting, not an RSS measurement or a
    // production limit increase. Insufficient original admission must refuse.
    let (memory_multiplier, work_multiplier) = match policy {
        ReadPolicy::Sidecar => (64, 32),
        // The canonical decoder admits its own work and memory separately.
        ReadPolicy::PrivateBytes | ReadPolicy::Committed => (2, 0),
    };
    let memory_bytes = length
        .checked_mul(memory_multiplier)
        .and_then(|n| n.checked_add(65536))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget
        .reserve_memory(memory_bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let work_bytes = length
        .checked_mul(work_multiplier)
        .and_then(|n| n.checked_add(1))
        .ok_or(IndexError::ResourceLimited)?;
    charge(budget, 0, work_bytes)?;
    let length_usize = usize::try_from(length).map_err(|_error| IndexError::ResourceLimited)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length_usize)
        .map_err(|_error| IndexError::ResourceLimited)?;
    bytes.resize(length_usize, 0);
    let mut offset = 0_usize;
    while offset < length_usize {
        let remaining = bytes.get_mut(offset..).ok_or(IndexError::ResourceLimited)?;
        charge(
            budget,
            1,
            u64::try_from(remaining.len()).map_err(|_| IndexError::ResourceLimited)?,
        )?;
        let read = match file.read_at(
            remaining,
            u64::try_from(offset).map_err(|_| IndexError::ResourceLimited)?,
        ) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(storage(error)),
        };
        if read == 0 {
            return Err(IndexError::DigestConflict.into());
        }
        offset = offset
            .checked_add(read)
            .ok_or(IndexError::ResourceLimited)?;
    }
    let mut extra = [0_u8; 1];
    charge(budget, 1, 1)?;
    if file.read_at(&mut extra, length).map_err(storage)? != 0 {
        return Err(IndexError::DigestConflict.into());
    }
    charge(budget, 1, 0)?;
    if Identity::from(&file.metadata().map_err(storage)?) != before {
        return Err(IndexError::DigestConflict.into());
    }
    charge(budget, 1, 0)?;
    let current = fs::symlink_metadata(path).map_err(storage)?;
    if current.file_type().is_symlink() || Identity::from(&current) != before {
        return Err(IndexError::DigestConflict.into());
    }
    // Closing remains owned by the real I/O port. The caller must witness its
    // close/retirement queue before publishing, just as for Native BOOT reads.
    drop(opened);
    Ok(ExistingFileBytesV1 {
        bytes,
        identity: before,
        _memory: memory,
    })
}

impl ExistingFileBytesV1 {
    /// Borrow immutable observed DATA while its actual allocation remains held.
    #[must_use]
    pub fn bytes_v1(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn identity_v1(&self) -> ExistingFileIdentityV1 {
        ExistingFileIdentityV1(self.identity)
    }
}

mod hash;
pub use hash::{read_existing_committed_file_v1, verify_existing_committed_files_v1};
