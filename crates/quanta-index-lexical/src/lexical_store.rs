//! Authenticated immutable objects and bounded copy-on-write lexical maps.
//!
//! Publication, generation retention and collection remain with their
//! existing owners. This module supplies durable attempt custody and a
//! descriptor-anchored object graph for those owners.

mod codec;
mod map;
mod store;

#[cfg(test)]
pub(crate) use store::fake_gc_delete_ticket;

pub(crate) use codec::{BlobRef, MapEdit, MapRoot, ObjectKind, ObjectRef};
pub(crate) use map::{MAX_MAP_HEIGHT, MapCursor};
pub(crate) use store::{
    AdoptedRootAdvance, AdoptedRootCursor, AttemptBinding, AttemptLease, AttemptWriteCost,
    Authority, BlobCursor, CandidateWitness, CheckpointPayload, EligibilityWitness, Gate,
    GcDeleteCustody, IndependentWindowOracle, IneligibleReason, MAX_CLOSURE_REFS,
    ObjectInventoryAdvance, ObjectInventoryCursor, ObjectStore, ScanLane, WindowWitness,
};

#[cfg(test)]
mod tests;
