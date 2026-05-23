use quanta_index_contract::{
    PublishedGenerationSet, PublishedSearchBundleInspectResponse, PublishedSearchBundleManifest,
    PublishedSearchGenerationActivateRequest, PublishedSearchGenerationActivateResponse,
    PublishedSearchGenerationReadinessResponse, RepoId, RevisionId,
};
use quanta_index_core::{
    ActivationPolicy, CoreError, PublishedSearchBundleInspectPort,
    PublishedSearchGenerationActivatePort, PublishedSearchGenerationReadinessPort,
};
use rusqlite::{OptionalExtension, params};

use super::helpers::{row_to_generation_set, sqlite_u64_to_i64};
use super::{SqliteControlPlane, placeholder_artifact};

impl PublishedSearchGenerationActivatePort for SqliteControlPlane {
    fn activate_generation(
        &mut self,
        request: PublishedSearchGenerationActivateRequest,
    ) -> Result<PublishedSearchGenerationActivateResponse, CoreError> {
        ActivationPolicy::validate_request(&request)?;

        let catalog_rows = self
            .conn
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
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active')
                ",
                params![
                    request.generation.repo_id.as_str(),
                    request.generation.revision_id.as_str(),
                    sqlite_u64_to_i64(
                        "manifest_generation",
                        request.generation.manifest_generation.get(),
                    )?,
                    sqlite_u64_to_i64(
                        "lexical_generation",
                        request.generation.lexical_generation.get(),
                    )?,
                    sqlite_u64_to_i64(
                        "symbol_generation",
                        request.generation.symbol_generation.get(),
                    )?,
                    request
                        .generation
                        .structural_generation
                        .map(|value| sqlite_u64_to_i64("structural_generation", value.get()))
                        .transpose()?,
                    request
                        .generation
                        .history_generation
                        .map(|value| sqlite_u64_to_i64("history_generation", value.get()))
                        .transpose()?,
                    request
                        .generation
                        .semantic_generation
                        .map(|value| sqlite_u64_to_i64("semantic_generation", value.get()))
                        .transpose()?,
                    request
                        .generation
                        .metadata_generation
                        .map(|value| sqlite_u64_to_i64("metadata_generation", value.get()))
                        .transpose()?,
                ],
            )
            .map_err(|error| {
                CoreError::Storage(format!("upsert generation catalog failed: {error}"))
            })?;
        if catalog_rows == 0 {
            return Err(CoreError::Storage(
                "upsert generation catalog affected zero rows".into(),
            ));
        }

        let activation_rows = self
            .conn
            .execute(
                "
                INSERT OR REPLACE INTO generation_activation_state (
                    repo_id,
                    revision_id,
                    active_manifest_generation,
                    lexical_ready,
                    semantic_ready,
                    active_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ",
                params![
                    request.generation.repo_id.as_str(),
                    request.generation.revision_id.as_str(),
                    sqlite_u64_to_i64(
                        "active_manifest_generation",
                        request.generation.manifest_generation.get(),
                    )?,
                    i64::from(request.lexical_ready),
                    i64::from(request.semantic_ready),
                    sqlite_u64_to_i64("active_at_ms", request.active_at_ms)?,
                ],
            )
            .map_err(|error| {
                CoreError::Storage(format!("upsert activation state failed: {error}"))
            })?;
        if activation_rows == 0 {
            return Err(CoreError::Storage(
                "upsert activation state affected zero rows".into(),
            ));
        }

        Ok(PublishedSearchGenerationActivateResponse {
            activated: true,
            active_generation: Some(request.generation),
            reason: None,
        })
    }
}

impl PublishedSearchGenerationReadinessPort for SqliteControlPlane {
    fn read_readiness(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<PublishedSearchGenerationReadinessResponse, CoreError> {
        let prepared_count = u64::try_from(
            self.conn
                .query_row(
                    "
                    SELECT COUNT(*) FROM prepared_bundle_outbox
                    WHERE repo_id = ?1 AND revision_id = ?2
                    ",
                    params![repo_id.as_str(), revision_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| {
                    CoreError::Storage(format!("count prepared bundles failed: {error}"))
                })?,
        )
        .map_err(|error| {
            CoreError::Storage(format!("prepared bundle count out of range: {error}"))
        })?;

        let active_generation = self
            .conn
            .query_row(
                "
                SELECT
                    g.repo_id,
                    g.revision_id,
                    g.manifest_generation,
                    g.lexical_generation,
                    g.symbol_generation,
                    g.structural_generation,
                    g.history_generation,
                    g.semantic_generation,
                    g.metadata_generation
                FROM generation_catalog g
                JOIN generation_activation_state a
                  ON a.repo_id = g.repo_id
                 AND a.revision_id = g.revision_id
                 AND a.active_manifest_generation = g.manifest_generation
                WHERE g.repo_id = ?1 AND g.revision_id = ?2
                ",
                params![repo_id.as_str(), revision_id.as_str()],
                row_to_generation_set,
            )
            .optional()
            .map_err(|error| {
                CoreError::Storage(format!("read active generation failed: {error}"))
            })?;

        let activation_row: Option<(bool, bool)> = self
            .conn
            .query_row(
                "
                SELECT lexical_ready, semantic_ready
                FROM generation_activation_state
                WHERE repo_id = ?1 AND revision_id = ?2
                ",
                params![repo_id.as_str(), revision_id.as_str()],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, i64>(1)? != 0)),
            )
            .optional()
            .map_err(|error| CoreError::Storage(format!("read activation row failed: {error}")))?;

        let (lexical_ready, semantic_ready) = activation_row.unwrap_or((false, false));

        Ok(PublishedSearchGenerationReadinessResponse {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            prepared_bundle_count: prepared_count,
            active_generation,
            lexical_ready,
            semantic_ready,
            mode: "serve_only".into(),
            reason: None,
        })
    }
}

impl PublishedSearchBundleInspectPort for SqliteControlPlane {
    fn inspect_bundle(
        &self,
        generation: &PublishedGenerationSet,
    ) -> Result<PublishedSearchBundleInspectResponse, CoreError> {
        let metadata_rows = placeholder_artifact("bundle/metadata_rows.arrow");
        let manifest = PublishedSearchBundleManifest {
            repo_id: generation.repo_id.clone(),
            revision_id: generation.revision_id.clone(),
            manifest_generation: generation.manifest_generation,
            bundle_schema_version: 1,
            lexical_chunk_rows: placeholder_artifact("bundle/chunk_rows.arrow"),
            symbol_rows: placeholder_artifact("bundle/symbol_rows.arrow"),
            metadata_rows: Some(metadata_rows.clone()),
            graph_rows: None,
            embedding_input_views: None,
            embedding_records: None,
            mutation_delta: None,
        };

        Ok(PublishedSearchBundleInspectResponse {
            artifacts: vec![
                manifest.lexical_chunk_rows.clone(),
                manifest.symbol_rows.clone(),
                metadata_rows,
            ],
            manifest,
            mode: "serve_only".into(),
            state: "active".into(),
        })
    }
}
