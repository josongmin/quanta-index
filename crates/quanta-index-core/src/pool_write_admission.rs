//! Admission for the lexical pool's encoded object files.
//!
//! This counts service-controlled canonical objects and in-flight or orphaned
//! temporaries. It is not filesystem block reservation, whole-track disk
//! usage, process RSS, or an I/O scheduler.

use crate::CoreError;

/// Explicit service-controlled pool-file limits. Neither field reserves
/// filesystem blocks or bounds process resident memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PoolWriteLimits {
    max_encoded_bytes: u64,
    max_files: u64,
}

impl PoolWriteLimits {
    pub fn new(max_encoded_bytes: u64, max_files: u64) -> Result<Self, CoreError> {
        if max_encoded_bytes == 0 || max_files == 0 {
            return Err(CoreError::InvalidContract(
                "pool limits must be positive".into(),
            ));
        }
        Ok(Self {
            max_encoded_bytes,
            max_files,
        })
    }

    #[must_use]
    pub const fn max_encoded_bytes(self) -> u64 {
        self.max_encoded_bytes
    }

    #[must_use]
    pub const fn max_files(self) -> u64 {
        self.max_files
    }
}

/// A physical encoded pool file observed under the service's state-root lease.
/// This inventory includes unreferenced objects and abandoned temporaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoolInventoryEntry {
    Canonical {
        object_sha256: [u8; 32],
        encoded_bytes: u64,
    },
    Orphan {
        temp_identity: [u8; 32],
        encoded_bytes: u64,
    },
}

/// An already unlinked pool temporary whose parent directory has not yet
/// completed the durability barrier. Its original charged width is retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingOrphanUnlink {
    pub temp_identity: [u8; 32],
    pub encoded_bytes: u64,
    pub temp_name: String,
}

/// One snapshot of the service's logical encoded-file accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PoolWriteUsage {
    pub file_limit: u64,
    pub charged_files: u64,
    pub ceiling_bytes: u64,
    pub canonical_bytes: u64,
    pub in_flight_bytes: u64,
    pub orphan_bytes: u64,
    pub total_charged_bytes: u64,
}

/// One provisional charge for an encoded object write.
///
/// Terminal methods must follow the actual durable file transition. Dropping after `temp_created`
/// retains the charge as an orphan until a confirmed unlink and directory sync.
pub trait PoolWritePermit: Send {
    /// Refuse file creation for a reuse-only or completed permit before `O_EXCL`.
    fn check_new_file_admitted(&self) -> Result<(), CoreError>;

    /// The temporary was created. Its identity is derived by the store from
    /// the unique temp name and pinned pool identity; it is not filesystem proof.
    fn temp_created(&mut self, temp_identity: [u8; 32]) -> Result<(), CoreError>;

    /// The temporary was renamed into the canonical digest name. A later
    /// directory-sync failure does not release this retained charge.
    fn canonical_renamed(&mut self) -> Result<(), CoreError>;

    /// The temporary was unlinked and its parent directory synced before
    /// canonical publication.
    fn temp_unlinked_and_synced(&mut self) -> Result<(), CoreError>;

    /// An existing canonical digest object was authenticated; no new file is
    /// created by this invocation.
    fn reused_verified(&mut self) -> Result<(), CoreError>;
}

/// Installed by the service under a held state-root lease. Absence means the
/// legacy path has no pool-capacity qualification; implementations must not
/// silently return an unbounded permit.
pub trait PoolWriteAdmissionPort: Send + Sync {
    /// Charge encoded bytes before creating any object temporary.
    ///
    /// An already accounted canonical digest returns a reuse-only permit; the store must
    /// still authenticate that file before calling `reused_verified`.
    /// Admission must fail immediately at the ceiling rather than waiting.
    fn try_reserve(
        &self,
        object_sha256: [u8; 32],
        encoded_bytes: u64,
    ) -> Result<Box<dyn PoolWritePermit>, CoreError>;

    /// Record a canonical unlink that succeeded while its shard directory is
    /// not yet synced.
    ///
    /// Keep the exact charge until a later confirmed sync.
    /// The caller must hold the pool gate and must call this only after unlink.
    fn canonical_unlinked_pending_sync(
        &self,
        object_sha256: [u8; 32],
        encoded_bytes: u64,
    ) -> Result<(), CoreError>;

    /// Visit charged canonical unlinks whose directory sync is still pending.
    ///
    /// The caller must check absence and sync each shard under the pool gate
    /// before calling `confirmed_canonical_unlink` for that identity.
    fn pending_canonical_unlinks(
        &self,
        max_objects: usize,
    ) -> Result<Vec<([u8; 32], u64)>, CoreError>;

    /// Release only the exact canonical object after its actual unlink and
    /// shard-directory sync. Stale or repeated signals must fail closed.
    fn confirmed_canonical_unlink(
        &self,
        object_sha256: [u8; 32],
        encoded_bytes: u64,
    ) -> Result<(), CoreError>;

    /// Release the exact abandoned temporary after actual unlink and
    /// parent-directory sync. This covers permits already dropped on failure.
    fn confirmed_orphan_unlink(
        &self,
        temp_identity: [u8; 32],
        encoded_bytes: u64,
    ) -> Result<(), CoreError>;

    /// Record an actual temporary unlink before syncing its parent directory.
    ///
    /// Keep the original orphan charge until `confirmed_orphan_unlink`.
    /// The caller must hold the pool gate and supply the unlinked basename.
    fn orphan_unlinked_pending_sync(
        &self,
        temp_identity: [u8; 32],
        encoded_bytes: u64,
        temp_name: &str,
    ) -> Result<(), CoreError>;

    /// Visit charged temporary unlinks still awaiting a directory sync.
    ///
    /// The caller must authenticate each basename and absence under the pool
    /// gate before syncing its parent and confirming the unlink.
    fn pending_orphan_unlinks(
        &self,
        max_objects: usize,
    ) -> Result<Vec<PendingOrphanUnlink>, CoreError>;

    /// Return the original charged width of a dropped temporary.
    ///
    /// A partial write's current file length is not the amount reserved for that temp.
    /// This is read-only; the charge remains until confirmed unlink and sync.
    fn orphan_charge(&self, temp_identity: [u8; 32]) -> Result<Option<u64>, CoreError>;

    fn snapshot(&self) -> Result<PoolWriteUsage, CoreError>;
}
