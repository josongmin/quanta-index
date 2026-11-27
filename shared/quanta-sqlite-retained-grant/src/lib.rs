//! Shared type boundary for retained SQLite owner-VFS memory admission.

#![forbid(unsafe_code)]

/// A charge retained for the process lifetime by the SQLite owner VFS.
/// The authority that minted this grant must enforce its actual hard budget.
pub trait RetainedMemoryGrant: Send + Sync + 'static {
    fn bytes(&self) -> u64;
}
