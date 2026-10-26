//! Explicit daemon test environment shared by real-process fixtures.

use std::process::Command;

/// Apply the shared daemon test environment.
///
/// Includes the deterministic embedder, history retention and explicit physical
/// test profile. Direct spawn and umask wrapper share this policy (TOPT-03).
pub(crate) fn apply_searchd_env(command: &mut Command, max_generations: usize) {
    let physical = quanta_index_searchd::app::config::TEST_STATE_ROOT_PHYSICAL_POLICY;
    let _configured = command
        .env("QUANTA_INDEX_EMBEDDER", "hash-dev")
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS",
            max_generations.to_string(),
        )
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
            (16 * 1024 * 1024).to_string(),
        )
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS",
            "128",
        )
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
            (256 * 1024 * 1024).to_string(),
        );
    for (name, value) in [
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES",
            physical.inventory.bytes,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_FILES",
            physical.inventory.files,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING",
            physical.inventory.pending,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_ENTRIES",
            physical.inventory.entries,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_DEPTH",
            u64::from(physical.inventory.depth),
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING_FRAMES",
            u64::from(physical.inventory.pending_frames),
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_READER_HANDLES",
            u64::from(physical.binder.reader_handles),
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_TRACKED_ENTRIES",
            u64::from(physical.binder.tracked_entries),
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PROCESS_ROOTS",
            u64::from(physical.binder.process_roots),
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_GLOBAL_MAX_SQLITE_HANDLES",
            u64::from(physical.global_sqlite_handles),
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_BYTES",
            physical.maintenance.bytes,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_FILES",
            physical.maintenance.files,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_BYTES",
            physical.control.bytes,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_FILES",
            physical.control.files,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_BYTES",
            physical.work.bytes,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_FILES",
            physical.work.files,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_BYTES",
            physical.history.bytes,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_FILES",
            physical.history.files,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_DURABLE_CONTROL_FILE_MAX_BYTES",
            physical.durable_control_file_bytes,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CONTROL_FILE_MAX_BYTES",
            physical.repomap_control_file_bytes,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CANDIDATE_FILE_MAX_BYTES",
            physical.repomap_candidate_file_bytes,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_BYTES",
            physical.catalog_parent.bytes,
        ),
        (
            "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_FILES",
            physical.catalog_parent.files,
        ),
    ] {
        let _configured = command.env(name, value.to_string());
    }
}
