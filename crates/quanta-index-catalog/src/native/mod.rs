//! Native incremental catalog authority on the existing `SQLite` connection.

mod authority;

pub use authority::{
    LegacyCatalogQuiescenceV1, LegacyHistoryProducerScopeV1, LegacyHistorySnapshotObservationV1, LegacyHistorySourceProofV1, NativeBootCatalogCandidate, NativeBootCatalogSnapshot,
    NativeBootManifest, NativeBootPublishedOperation, NativeBootQuiescentCatalogV1,
    TerminalBatchReadError, VerifiedLegacyTerminalBatch, observe_legacy_history_snapshot_v1, verify_legacy_catalog_quiescence_v1,
    verify_legacy_history_source_inventory_v1, verify_legacy_history_sources_v1,
    visit_legacy_history_checkpoint_pages_v1, visit_legacy_history_checkpoint_rows_v1, visit_legacy_history_generation_keys_v1, visit_legacy_history_snapshot_rows_v1,
    verify_legacy_terminal_batch_receipt, verify_native_boot_candidate,
    verify_native_boot_current_candidate, verify_native_boot_catalog,
    verify_native_published_operation, verify_native_rollback_catalog,
    verify_native_legacy_history_recovery_publications_v1, verify_native_rollback_with_source_publications, verify_native_source_root_publications_v1,
    verify_native_source_root_quiescence_v1,
};
pub(crate) use authority::{initialize, preflight_capture_format};
