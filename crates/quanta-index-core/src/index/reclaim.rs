//! Catalog-fenced physical reclamation in bounded, restartable steps.
//!
//! These values bind a persisted phase and object generation. They do not
//! authorize a store mutation without paired catalog revalidation.
use super::{ObjectRef, RegisteredObject};

/// Physical unlink and parent sync after the store validates its Catalog permit.
/// This port grants no logical reclaim authority and must preserve failed custody.
pub trait ObjectUnlinkPort: std::fmt::Debug + Send + Sync {
    fn unlink_and_sync(
        &self,
        path: &std::path::Path,
        expected: &PhysicalIncarnation,
    ) -> Result<(), super::IndexError>;
    /// Resume physical debt before Catalog advances an already absent target.
    /// Absence alone must not release a retained inode charge.
    fn confirm_absence_and_sync(
        &self,
        path: &std::path::Path,
        expected: &PhysicalIncarnation,
    ) -> Result<(), super::IndexError>;
}

pub const MAX_RECLAIM_CANDIDATES: u32 = 8;
/// Each drain transaction removes at most this many direct child edges.
pub const MAX_RECLAIM_DRAIN_PAGE: u32 = 32;
/// The immutable store's wire format admits at most this many direct children.
pub const MAX_RECLAIM_CHILDREN: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReclaimKind {
    Registered,
    Provisional,
}

/// Held namespace identity, including the objects directory and exclusive
/// lease file. A different incarnation cannot consume an old permit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectNamespaceBinding {
    pub root_device: u64,
    pub root_inode: u64,
    pub objects_device: u64,
    pub objects_inode: u64,
    pub lease_device: u64,
    pub lease_inode: u64,
}

/// Validated immutable wire object. The store separately checks its digest,
/// canonical children, private mode and single-link policy under its gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalIncarnation {
    pub namespace: ObjectNamespaceBinding,
    /// The physical shard held at Reclaiming admission. A different linked
    /// shard cannot turn a still-existing object into apparent absence.
    pub shard_device: u64,
    pub shard_inode: u64,
    pub device: u64,
    pub inode: u64,
    pub changed_nanos: i128,
    pub bytes: u64,
}

/// One durable lifecycle row exists per live or unfinished object.
///
/// Completion removes the row. Later reinstall obtains a fresh global fence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReclaimCandidate {
    pub object: ObjectRef,
    pub generation: u64,
    pub kind: ReclaimKind,
}

/// Catalog Retiring phase. Adoption may cancel it before Reclaiming is stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetiringRef {
    pub candidate: ReclaimCandidate,
    pub direct_children: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReclaimTarget {
    Present {
        incarnation: PhysicalIncarnation,
        /// Canonical physical registration, compared structurally with the
        /// frozen catalog row and exact direct-edge ledger before unlink.
        registration: RegisteredObject,
    },
    /// Only a provisional parent with no in-flight installer can be absent
    /// before a Reclaiming permit is persisted.
    AbsentProvisional(ObjectNamespaceBinding),
}

/// Persisted Reclaiming and later Unlinked/Draining phases use the same frozen
/// object/generation/physical target. Both phases block new installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReclaimPermit {
    pub retiring: RetiringRef,
    pub target: ReclaimTarget,
}

/// The single catalog work stream includes new candidates and unfinished
/// persisted phases. Page order is object order; a later sweep restarts at None.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReclaimWork {
    Candidate(ReclaimCandidate),
    Reclaiming(ReclaimPermit),
    Draining(ReclaimPermit),
}

impl ReclaimWork {
    #[must_use]
    pub fn object(&self) -> ObjectRef {
        match self {
            Self::Candidate(candidate) => candidate.object,
            Self::Reclaiming(permit) | Self::Draining(permit) => permit.retiring.candidate.object,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReclaimWorkPage {
    /// Sorted, unique and at most `MAX_RECLAIM_CANDIDATES`.
    pub items: Vec<ReclaimWork>,
    pub has_more: bool,
}

/// One atomic prefix of remaining direct edges.
///
/// Deleting each edge is itself
/// the durable cursor: after crash the next call selects the first remaining
/// edge, never a previously committed edge. `has_more=false` means completed;
/// the lifecycle row is removed in that same transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReclaimDrainPage {
    pub released_children: u32,
    pub enqueued_children: u32,
    pub has_more: bool,
}
