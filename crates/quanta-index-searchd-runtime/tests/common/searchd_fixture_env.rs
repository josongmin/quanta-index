//! Explicit daemon test environment shared by real-process fixtures.

use std::process::Command;

#[path = "legacy_physical_fixture.rs"]
mod legacy_physical_fixture;

use legacy_physical_fixture::legacy_physical_env;

/// Apply the shared daemon test environment.
///
/// Includes the deterministic embedder, history retention and explicit physical
/// test profile. Direct spawn and umask wrapper share this policy (TOPT-03).
pub(crate) fn apply_searchd_env(command: &mut Command, max_generations: usize) {
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
        )
        .envs(legacy_physical_env());
}
