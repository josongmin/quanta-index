//! Logical boot-census allocation envelopes, not RSS or allocator metadata.
//!
//! The repository pins Rust 1.92.0. Its alloc/collections/btree/node.rs has
//! B=6, eleven key/value slots and twelve child pointers per internal node.
//! Account a whole maximal node per observed unique key. A populated tree
//! cannot have more nodes than keys; splits retain the previously admitted
//! headroom. These source assumptions must be reviewed on toolchain changes.
use super::{InodeKey, PublicationCapacityLedger, Retained, State};
use crate::CoreError;
use std::{alloc::Layout, ptr::NonNull, sync::Mutex};

fn overflow() -> CoreError {
    CoreError::InvalidContract("publication boot allocation layout overflow".into())
}

/// Upper bound for one maximal pinned-toolchain BTree node. The sum includes
/// independent worst-case alignment padding for EVERY field, so it does not
/// rely on Rust's private field ordering or expose/inspect allocator internals.
/// Consumers still obtain actual grants from the original WorkBudget BEFORE
/// collection growth. This arithmetic supplies no physical authority.
pub fn publication_inventory_map_node_bytes_v1<K, V>() -> Result<u64, CoreError> {
    let fields = [
        Layout::new::<Option<NonNull<()>>>(),
        Layout::new::<u16>(),
        Layout::new::<u16>(),
        Layout::array::<K>(11).map_err(|_| overflow())?,
        Layout::array::<V>(11).map_err(|_| overflow())?,
        Layout::array::<Option<NonNull<()>>>(12).map_err(|_| overflow())?,
    ];
    let mut bytes = 0_usize;
    let mut alignment = 1_usize;
    for field in fields {
        bytes = bytes
            .checked_add(field.size())
            .and_then(|n| n.checked_add(field.align().checked_sub(1)?))
            .ok_or_else(overflow)?;
        alignment = alignment.max(field.align());
    }
    bytes = bytes
        .checked_add(alignment.checked_sub(1).ok_or_else(overflow)?)
        .ok_or_else(overflow)?;
    u64::try_from(bytes).map_err(|_| overflow())
}
impl PublicationCapacityLedger {
    /// The canonical ledger's real retained-row type stays with its owner.
    pub fn boot_inode_allocation_bytes_v1() -> Result<u64, CoreError> {
        publication_inventory_map_node_bytes_v1::<InodeKey, Retained>()
    }
    /// Header plus actual `Mutex<State>` backing of the sealed capacity Arc.
    /// MemoryLease backing is admitted separately by its existing allocator.
    pub fn boot_state_allocation_bytes_v1() -> Result<u64, CoreError> {
        let layout = crate::index::MemoryLease::shared_allocation_backing_layout(Layout::new::<
            Mutex<State>,
        >())
        .map_err(|_| overflow())?;
        u64::try_from(layout.size()).map_err(|_| overflow())
    }
}
