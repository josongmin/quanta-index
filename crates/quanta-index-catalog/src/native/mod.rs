//! Native incremental catalog authority on the existing `SQLite` connection.

mod authority;

pub use authority::{
    NativeBootCatalogCandidate, NativeBootCatalogSnapshot, NativeBootManifest,
    verify_native_boot_candidate, verify_native_boot_catalog,
};
pub(crate) use authority::{initialize, preflight_capture_format};
