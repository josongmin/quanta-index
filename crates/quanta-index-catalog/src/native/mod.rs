//! Native incremental catalog authority on the existing `SQLite` connection.

mod authority;

pub use authority::{
    LegacyActiveCatalogCensusV1, LegacyCatalogQuiescenceV1, LegacyHistoryCheckpointPageV1,
    LegacyHistoryProducerScopeV1, LegacyHistorySnapshotObservationV1, LegacyHistorySourceProofV1,
    NativeBootCatalogCandidate, NativeBootCatalogSnapshot, NativeBootManifest,
    NativeBootPublicationReader, NativeBootPublishedOperation, NativeBootQuiescentCatalogV1,
    NativeBootSourceRouteRefV1, NativeProtectedSnapshotWitnessV1, TerminalBatchReadError,
    VerifiedLegacyTerminalBatch, lookup_native_protected_source_head_v1,
    observe_legacy_history_snapshot_v1, read_native_protected_snapshot_witness_v1,
    require_legacy_repomap_inapplicable_v1, resolve_native_protected_source_head_if_current_v1,
    verify_legacy_active_repomap_receipts_with_siblings_v1, verify_legacy_catalog_quiescence_v1,
    verify_legacy_history_source_inventory_v1, verify_legacy_history_sources_v1,
    verify_legacy_terminal_batch_receipt, verify_native_boot_candidate, verify_native_boot_catalog,
    verify_native_boot_current_candidate, verify_native_legacy_history_recovery_publications_v1,
    verify_native_boot_scope_selected_view_v1, visit_native_boot_scope_current_view_v1,
    verify_native_legacy_history_recovery_publications_with_routes_v1,
    verify_native_protected_snapshot_install_v1,
    verify_native_protected_source_routes_for_candidate_resume_v1,
    verify_native_protected_source_routes_if_current_v1, verify_native_protected_source_routes_v1,
    verify_native_published_operation, verify_native_published_view_v1,
    verify_native_published_view_with_routes_v1,
    verify_native_recovery_publications_with_routes_v1, verify_native_rollback_catalog,
    verify_native_rollback_with_source_publications, verify_native_source_root_publications_v1,
    verify_native_source_root_publications_with_routes_v1, verify_native_source_root_quiescence_v1,
    verify_native_source_root_quiescence_with_routes_v1, visit_legacy_active_catalog_heads_v1,
    visit_legacy_history_checkpoint_pages_v1, visit_legacy_history_checkpoint_rows_v1,
    visit_legacy_history_generation_keys_v1, visit_legacy_history_snapshot_rows_v1,
    visit_native_protected_snapshot_rows_v1, with_legacy_active_repomap_receipts_with_siblings_v1,
};
pub(crate) use authority::{certify_open_pin_rows, initialize, preflight_capture_format};
