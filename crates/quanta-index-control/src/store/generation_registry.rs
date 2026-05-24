use quanta_index_contract::{
    BundleArtifactRef, PublishedGenerationSet, PublishedSearchBundleInspectResponse,
    PublishedSearchBundleManifest, PublishedSearchGenerationActivateRequest,
    PublishedSearchGenerationActivateResponse, PublishedSearchGenerationReadinessResponse, RepoId,
    RevisionId,
};
use quanta_index_core::{
    ActivationPolicy, BundlePolicy, CoreError, PublishedSearchActivationStatePort,
    PublishedSearchBundleInspectPort, PublishedSearchGenerationActivatePort,
    PublishedSearchGenerationCatalogPort, PublishedSearchGenerationReadinessPort,
};
use rusqlite::{OptionalExtension, params};

use super::ControlPlane;
use super::helpers::{row_to_generation_set, sqlite_u64_to_i64};

impl PublishedSearchGenerationActivatePort for ControlPlane {
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

impl PublishedSearchGenerationReadinessPort for ControlPlane {
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

impl PublishedSearchGenerationCatalogPort for ControlPlane {
    fn record_generation_manifest(
        &mut self,
        manifest: PublishedSearchBundleManifest,
    ) -> Result<(), CoreError> {
        BundlePolicy::validate_artifact_ref(
            "manifest.lexical_chunk_rows",
            &manifest.lexical_chunk_rows,
        )?;
        BundlePolicy::validate_artifact_ref("manifest.symbol_rows", &manifest.symbol_rows)?;
        if let Some(reference) = &manifest.metadata_rows {
            BundlePolicy::validate_artifact_ref("manifest.metadata_rows", reference)?;
        }
        if let Some(reference) = &manifest.graph_rows {
            BundlePolicy::validate_artifact_ref("manifest.graph_rows", reference)?;
        }
        if let Some(reference) = &manifest.embedding_input_views {
            BundlePolicy::validate_artifact_ref("manifest.embedding_input_views", reference)?;
        }
        if let Some(reference) = &manifest.embedding_records {
            BundlePolicy::validate_artifact_ref("manifest.embedding_records", reference)?;
        }
        if let Some(reference) = &manifest.mutation_delta {
            BundlePolicy::validate_artifact_ref("manifest.mutation_delta", reference)?;
        }

        let manifest_json = serde_json::to_string(&manifest)
            .map_err(|error| CoreError::Storage(format!("encode manifest json failed: {error}")))?;

        let inserted_rows = self
            .conn
            .execute(
                "
                INSERT OR REPLACE INTO generation_manifest (
                    repo_id,
                    revision_id,
                    manifest_generation,
                    manifest_json
                ) VALUES (?1, ?2, ?3, ?4)
                ",
                params![
                    manifest.repo_id.as_str(),
                    manifest.revision_id.as_str(),
                    sqlite_u64_to_i64("manifest_generation", manifest.manifest_generation.get(),)?,
                    manifest_json,
                ],
            )
            .map_err(|error| {
                CoreError::Storage(format!("upsert generation manifest failed: {error}"))
            })?;
        if inserted_rows == 0 {
            return Err(CoreError::Storage(
                "upsert generation manifest affected zero rows".into(),
            ));
        }
        Ok(())
    }
}

impl PublishedSearchBundleInspectPort for ControlPlane {
    fn inspect_bundle(
        &self,
        generation: &PublishedGenerationSet,
    ) -> Result<PublishedSearchBundleInspectResponse, CoreError> {
        let manifest_json: Option<String> =
            self.conn
                .query_row(
                    "
                SELECT manifest_json FROM generation_manifest
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
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| {
                    CoreError::Storage(format!("read generation manifest failed: {error}"))
                })?;

        let Some(manifest_json) = manifest_json else {
            return Err(CoreError::NotFound(
                "manifest not recorded for generation".into(),
            ));
        };

        let manifest: PublishedSearchBundleManifest = serde_json::from_str(&manifest_json)
            .map_err(|error| CoreError::Storage(format!("decode manifest json failed: {error}")))?;

        let mut artifacts: Vec<BundleArtifactRef> = Vec::new();
        artifacts.push(manifest.lexical_chunk_rows.clone());
        artifacts.push(manifest.symbol_rows.clone());
        if let Some(reference) = &manifest.metadata_rows {
            artifacts.push(reference.clone());
        }
        if let Some(reference) = &manifest.graph_rows {
            artifacts.push(reference.clone());
        }
        if let Some(reference) = &manifest.embedding_input_views {
            artifacts.push(reference.clone());
        }
        if let Some(reference) = &manifest.embedding_records {
            artifacts.push(reference.clone());
        }

        Ok(PublishedSearchBundleInspectResponse {
            artifacts,
            manifest,
            mode: "serve_only".into(),
            state: "active".into(),
        })
    }
}

impl PublishedSearchActivationStatePort for ControlPlane {
    fn mark_active_generation(
        &mut self,
        generation: &PublishedGenerationSet,
        active_at_ms: u64,
    ) -> Result<(), CoreError> {
        // Orchestrator-side path: builds already proven, so both readiness
        // flags are 1. We still re-affirm the catalog row to guarantee the
        // FK-like invariant (activation_state references a known catalog gen)
        // holds even if the catalog write hasn't happened separately.
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
                        .map(|value| sqlite_u64_to_i64("structural_generation", value.get()))
                        .transpose()?,
                    generation
                        .history_generation
                        .map(|value| sqlite_u64_to_i64("history_generation", value.get()))
                        .transpose()?,
                    generation
                        .semantic_generation
                        .map(|value| sqlite_u64_to_i64("semantic_generation", value.get()))
                        .transpose()?,
                    generation
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
                "mark_active_generation: catalog upsert affected zero rows".into(),
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
                ) VALUES (?1, ?2, ?3, 1, 1, ?4)
                ",
                params![
                    generation.repo_id.as_str(),
                    generation.revision_id.as_str(),
                    sqlite_u64_to_i64(
                        "active_manifest_generation",
                        generation.manifest_generation.get(),
                    )?,
                    sqlite_u64_to_i64("active_at_ms", active_at_ms)?,
                ],
            )
            .map_err(|error| {
                CoreError::Storage(format!("upsert activation state failed: {error}"))
            })?;
        if activation_rows == 0 {
            return Err(CoreError::Storage(
                "mark_active_generation: activation state upsert affected zero rows".into(),
            ));
        }
        Ok(())
    }
}
