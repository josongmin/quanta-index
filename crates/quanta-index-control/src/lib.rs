#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

mod store;

pub use store::*;

/// Test-only writes that bypass the public ports.
///
/// Exposed for integration tests in other crates that need to seed
/// control-plane state without activating it (e.g. delta-apply governance
/// scenarios that require a recorded-but-not-active generation). NOT for
/// production use.
pub mod test_support {
    use quanta_index_contract::PublishedGenerationSet;
    use quanta_index_core::CoreError;
    use rusqlite::params;

    use super::ControlPlane;
    use crate::store::sqlite_u64_to_i64;

    /// Insert (or replace) a catalog row in the `prepared` state.
    ///
    /// No activation row is written. Used by tests to construct delta-apply
    /// targets that satisfy the catalog-known + manifest-recorded checks
    /// while NOT being active.
    pub fn record_catalog_row_for_test(
        store: &ControlPlane,
        generation: &PublishedGenerationSet,
    ) -> Result<(), CoreError> {
        let rows = store
            .conn()
            .execute(
                "
                INSERT OR REPLACE INTO generation_catalog (
                    repo_id,
                    revision_id,
                    manifest_generation,
                    lexical_generation,
                    symbol_generation,
                    structural_generation,
                    history_generation,
                    semantic_generation,
                    metadata_generation,
                    state
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'prepared')
                ",
                params![
                    generation.repo_id.as_str(),
                    generation.revision_id.as_str(),
                    sqlite_u64_to_i64(
                        "manifest_generation",
                        generation.manifest_generation.get(),
                    )?,
                    sqlite_u64_to_i64(
                        "lexical_generation",
                        generation.lexical_generation.get(),
                    )?,
                    sqlite_u64_to_i64(
                        "symbol_generation",
                        generation.symbol_generation.get(),
                    )?,
                    generation
                        .structural_generation
                        .map(|v| sqlite_u64_to_i64("structural_generation", v.get()))
                        .transpose()?,
                    generation
                        .history_generation
                        .map(|v| sqlite_u64_to_i64("history_generation", v.get()))
                        .transpose()?,
                    generation
                        .semantic_generation
                        .map(|v| sqlite_u64_to_i64("semantic_generation", v.get()))
                        .transpose()?,
                    generation
                        .metadata_generation
                        .map(|v| sqlite_u64_to_i64("metadata_generation", v.get()))
                        .transpose()?,
                ],
            )
            .map_err(|error| CoreError::Storage(format!("test catalog insert failed: {error}")))?;
        if rows == 0 {
            return Err(CoreError::Storage(
                "test catalog insert affected zero rows".into(),
            ));
        }
        Ok(())
    }
}
