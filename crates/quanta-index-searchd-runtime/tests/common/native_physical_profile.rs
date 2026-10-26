//! Explicit fixture limits; these are not deployment defaults or qualification.

use quanta_index_searchd_runtime::{
    native::NativePhysicalProfile, publication_inventory::PublicationBootInventoryLimits,
};

pub(crate) fn profile() -> NativePhysicalProfile {
    NativePhysicalProfile {
        inventory: PublicationBootInventoryLimits {
            max_bytes: 64 * 1024 * 1024 * 1024,
            max_files: 200_000,
            max_pending: 4096,
            max_entries: 500_000,
            max_depth: 8,
            max_pending_frames: 1024,
        },
        max_reader_handles: 64,
        max_tracked_entries: 500_000,
        max_process_roots: 256,
        global_max_sqlite_handles: 512,
        maintenance_reserved_bytes: 512 * 1024,
        maintenance_reserved_files: 8,
    }
}
