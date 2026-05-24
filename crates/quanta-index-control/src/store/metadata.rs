use quanta_index_contract::PublishedGenerationSet;
use quanta_index_core::{CoreError, SearchPlaneMetadataStorePort};
use rusqlite::{OptionalExtension, params};

use super::ControlPlane;
use super::helpers::sqlite_u64_to_i64;

impl SearchPlaneMetadataStorePort for ControlPlane {
    fn open_metadata_store(&self, generation: &PublishedGenerationSet) -> Result<(), CoreError> {
        let recorded: Option<i64> =
            self.conn
                .query_row(
                    "
                SELECT 1 FROM generation_manifest
                WHERE repo_id = ?1
                  AND revision_id = ?2
                  AND manifest_generation = ?3
                ",
                    params![
                        generation.repo_id.as_str(),
                        generation.revision_id.as_str(),
                        sqlite_u64_to_i64(
                            "manifest_generation",
                            generation.manifest_generation.get(),
                        )?,
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|error| {
                    CoreError::Storage(format!("probe generation manifest failed: {error}"))
                })?;

        if recorded.is_none() {
            return Err(CoreError::NotReady(
                "metadata not recorded for generation".into(),
            ));
        }
        Ok(())
    }
}
