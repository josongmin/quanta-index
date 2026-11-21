//! Explicit daemon test environment shared by real-process fixtures.

use std::process::Command;

#[path = "physical.rs"]
mod physical;

#[doc(hidden)]
pub use physical::legacy_physical_env;

/// Default retention in generations per revision pair for benchmark fixtures.
pub const DEFAULT_HISTORY_MAX_GENERATIONS: usize = 8;

pub(super) fn searchd_env(max_generations: usize) -> Vec<(&'static str, String)> {
    let mut environment = vec![
        ("QUANTA_INDEX_EMBEDDER", "hash-dev".to_owned()),
        (
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS",
            max_generations.to_string(),
        ),
        (
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
            (16 * 1024 * 1024).to_string(),
        ),
        (
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS",
            "128".to_owned(),
        ),
        (
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
            (256 * 1024 * 1024).to_string(),
        ),
    ];
    environment.extend(legacy_physical_env());
    environment
}

/// Apply the shared daemon test environment.
///
/// Includes the deterministic embedder, history retention and explicit physical
/// test profile. Direct spawn and umask wrapper share this policy (TOPT-03).
pub fn apply_searchd_env(command: &mut Command, max_generations: usize) {
    let _configured = command.envs(searchd_env(max_generations));
}
