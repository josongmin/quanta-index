//! Provisional Native-to-Native History content-copy input. A generic FULL
//! verifies the immutable member but does not authenticate the claimed source
//! root or view. Installed BOOT independently derives and compares the complete
//! expected input under its held old root before accepting the candidate.

use super::{Digest, HistorySourceCutV1, IndexInstanceId, ObjectRef};
use crate::index::ViewRef;

/// One old Native History artifact and its exact source identity. The
/// immutable input object has that artifact as its only child, so importing
/// the object closure also imports the old member's complete row map. The
/// claimed source identity is DATA, not authority to select or activate a root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeHistoryRestoreInputV1 {
    pub wire_version: u32,
    pub source_root_nonce: Digest,
    pub source_view: ViewRef,
    pub instance: IndexInstanceId,
    pub member: ObjectRef,
    pub cut: HistorySourceCutV1,
}
crate::native_serde_struct!(NativeHistoryRestoreInputV1 {
    wire_version: u32,
    source_root_nonce: Digest,
    source_view: ViewRef,
    instance: IndexInstanceId,
    member: ObjectRef,
    cut: HistorySourceCutV1
});

impl NativeHistoryRestoreInputV1 {
    pub fn validate_shape_v1(&self) -> Result<(), &'static str> {
        if self.wire_version != 3
            || self.source_root_nonce == Digest([0; 32])
            || self.source_view.digest == Digest([0; 32])
            || self.source_view.epoch == 0
            || self.source_view.sequence == 0
            || self.member.0 == Digest([0; 32])
            || self.cut.durable_epoch == crate::AuxEpochV1::GENESIS
            || self.cut.snapshot_digest == Digest([0; 32])
            || self.cut.replay_start == Digest([0; 32])
        {
            return Err("Native History restore input identity or cut is invalid");
        }
        Ok(())
    }

    pub fn verify_children_v1(&self, children: &[ObjectRef]) -> Result<(), &'static str> {
        self.validate_shape_v1()?;
        if children != [self.member] {
            return Err("Native History restore member differs from immutable child");
        }
        Ok(())
    }
}
