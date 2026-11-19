//! `SQLite`-backed durable catalog for the search plane (W2, G0-C).
//!
//! One file, one connection, two tables families:
//! - the ingest idempotency record (QI-BB-032): one row per
//!   `(operation kind, repo, revision, generation, batch_digest)` carrying
//!   the canonical body hash, the apply state and the receipt of the apply
//!   — contract [`quanta_index_core::IdempotencyCatalogPort`];
//! - the auxiliary authority rows (QI-BB-020): one row per history,
//!   runtime-metadata and structural record, applied per batch as one
//!   transaction — contract [`quanta_index_core::AuxiliaryAuthorityCatalogPort`].
//!
//! This crate is the only place the storage engine is named. The G0-C
//! conditions bind here:
//! - every row carries its own digest over its content, verified on read,
//!   because the engine serves bit-rotted cells with a clean
//!   `integrity_check`;
//! - authority rows commit under `synchronous=FULL` (and `fullfsync=ON`
//!   where the platform honors it), never `NORMAL`;
//! - a write that meets a held lock past the busy budget is a typed
//!   `CATALOG_BUSY`, not a blocked thread.

#![forbid(unsafe_code)]

mod auxiliary;
mod backup;
mod candidate;
mod connection;
mod idempotency;
mod integrity;
mod io_admission;
mod native;
mod open;
mod sequence;
mod ticket_diagnostics;
mod writer_cache;

pub use backup::{
    CatalogSnapshotReceiptV1, CatalogSnapshotSessionV1, catalog_content_digest, fsync_directory,
    live_catalog_receipt, normalize_catalog_journal_mode, snapshot_catalog_file, verify_snapshot,
};
pub use candidate::{
    ActivationOutcomeV1, RepoMapActivationRowV1, RepoMapCandidateRowV1, RepoMapCandidateStateV1,
    RepoMapQuarantineIncidentRowV1, SealOutcomeV1,
};
pub use connection::{
    CATALOG_FILE_NAME, CatalogClockPort, SqliteCatalog, SystemCatalogClock, WalReclaimOutcome,
    catalog_dir,
};
pub use io_admission::{CatalogIoAdmission, CatalogIoScope, CatalogReadOnlyIoAdmission};
pub use native::{
    LegacyActiveCatalogCensusV1, LegacyCatalogQuiescenceV1, LegacyHistoryProducerScopeV1,
    LegacyHistorySnapshotObservationV1, LegacyHistorySourceProofV1, NativeBootCatalogCandidate,
    NativeBootCatalogSnapshot, NativeBootManifest, NativeBootPublishedOperation,
    NativeBootQuiescentCatalogV1, NativeBootSourceRouteRefV1, NativeProtectedSnapshotWitnessV1,
    TerminalBatchReadError,
    VerifiedLegacyTerminalBatch, lookup_native_protected_source_head_v1,
    read_native_protected_snapshot_witness_v1, visit_native_protected_snapshot_rows_v1,
    verify_native_protected_snapshot_install_v1,
    observe_legacy_history_snapshot_v1, resolve_native_protected_source_head_if_current_v1,
    require_legacy_repomap_inapplicable_v1,
    verify_legacy_active_repomap_receipts_with_siblings_v1, verify_legacy_catalog_quiescence_v1,
    with_legacy_active_repomap_receipts_with_siblings_v1,
    verify_legacy_history_source_inventory_v1, verify_legacy_history_sources_v1,
    verify_legacy_terminal_batch_receipt, verify_native_boot_candidate, verify_native_boot_catalog,
    verify_native_boot_current_candidate, verify_native_legacy_history_recovery_publications_v1,
    verify_native_legacy_history_recovery_publications_with_routes_v1,
    verify_native_protected_source_routes_if_current_v1, verify_native_protected_source_routes_v1,
    verify_native_published_operation, verify_native_published_view_v1,
    verify_native_published_view_with_routes_v1,
    verify_native_recovery_publications_with_routes_v1, verify_native_rollback_catalog,
    verify_native_rollback_with_source_publications, verify_native_source_root_publications_v1,
    verify_native_source_root_publications_with_routes_v1,
    verify_native_source_root_quiescence_v1, verify_native_source_root_quiescence_with_routes_v1,
    visit_legacy_active_catalog_heads_v1,
    visit_legacy_history_checkpoint_pages_v1, visit_legacy_history_checkpoint_rows_v1,
    visit_legacy_history_generation_keys_v1, visit_legacy_history_snapshot_rows_v1,
};
pub use ticket_diagnostics::{TicketValidationObservation, TicketValidationTimings};
pub use writer_cache::{
    CatalogWriterCacheObservation, CatalogWriterCachePolicy, CatalogWriterSettings,
};
