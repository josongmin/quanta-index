//! Verify-side catalog ports shared by offline migration regression suites.

use std::path::Path;

use quanta_index_catalog::verify_snapshot;
use quanta_index_core::CoreError;
use quanta_index_searchd::app::state_migration::{CatalogSnapshotPort, CatalogSnapshotV1};

/// The verify-side catalog port, wired the way the composition root wires it.
pub(super) struct CatalogVerifierV1;

impl CatalogSnapshotPort for CatalogVerifierV1 {
    fn verify_snapshot_at(&self, snapshot_file: &Path) -> Result<CatalogSnapshotV1, CoreError> {
        verify_snapshot(snapshot_file).map(|receipt| CatalogSnapshotV1 {
            content_digest_hex: receipt.content_digest_hex,
            byte_size: receipt.byte_size,
            table_rows: receipt.table_rows,
        })
    }
}
