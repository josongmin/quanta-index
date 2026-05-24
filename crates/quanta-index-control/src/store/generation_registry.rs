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

        // Stale-activation guard (E-SP2 per SSOT): if a generation is already
        // active for (repo, rev), the incoming generation must be strictly
        // newer by manifest_generation, AND each present component generation
        // must be monotonically non-decreasing vs the active one.
        if let Some(current_active) = read_active_generation(
            &self.conn,
            &request.generation.repo_id,
            &request.generation.revision_id,
        )? {
            stale_activation_guard(&current_active, &request.generation)?;
        }

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

        // Catalog state lifecycle (SSOT § generation_catalog.state):
        //   prepared  ← record_generation_manifest first writes the row
        //   active    ← activate_generation / mark_active_generation
        //   materialized / failed ← reserved for future orchestrator hooks
        //                            (see plan doc, deferred from Phase 1)
        //
        // INSERT OR IGNORE: if the row already exists in a higher state
        // (e.g. 'active' from a prior activate_generation call) we must NOT
        // downgrade it back to 'prepared'.
        let _catalog_rows = self
            .conn
            .execute(
                "
                INSERT OR IGNORE INTO generation_catalog (
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
                ) VALUES (?1, ?2, ?3, 0, 0, NULL, NULL, NULL, NULL, 'prepared')
                ",
                params![
                    manifest.repo_id.as_str(),
                    manifest.revision_id.as_str(),
                    sqlite_u64_to_i64("manifest_generation", manifest.manifest_generation.get(),)?,
                ],
            )
            .map_err(|error| {
                CoreError::Storage(format!("insert catalog 'prepared' row failed: {error}"))
            })?;
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
        if let Some(reference) = &manifest.mutation_delta {
            // `mutation_delta` is itself a `BundleArtifactRef` in the frozen
            // contract; include it in the artifact union for inspect (P1 fix
            // per reviewer — earlier impl omitted it).
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
        // Orchestrator-side path still respects the stale-activation guard so
        // out-of-order build outcomes can't silently roll the active pointer
        // backwards.
        if let Some(current_active) =
            read_active_generation(&self.conn, &generation.repo_id, &generation.revision_id)?
        {
            stale_activation_guard(&current_active, generation)?;
        }

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

/// Read the currently active generation for `(repo, rev)`, if any.
///
/// Joins `generation_activation_state` with `generation_catalog` so the
/// returned snapshot is the same shape as what the producer sent in.
pub(super) fn read_active_generation(
    conn: &rusqlite::Connection,
    repo: &RepoId,
    rev: &RevisionId,
) -> Result<Option<PublishedGenerationSet>, CoreError> {
    conn.query_row(
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
        params![repo.as_str(), rev.as_str()],
        row_to_generation_set,
    )
    .optional()
    .map_err(|error| CoreError::Storage(format!("read active generation failed: {error}")))
}

/// Reject activation requests that would roll the active pointer backwards.
///
/// Per SSOT E-SP2 / hellgate H-SP1 follow-up:
/// - `incoming.manifest_generation` must be strictly greater than
///   `current.manifest_generation` — equality means the same generation is
///   being re-activated, which we treat as a stale no-op rather than an
///   error path (caller should be idempotent), but we return `Ok` from the
///   guard there and let the upsert overwrite the row.
/// - Each component generation that is present in both `current` and
///   `incoming` must be monotonically non-decreasing. A previously-set
///   component dropping to `None` is also rejected (lossy regression).
pub(super) fn stale_activation_guard(
    current: &PublishedGenerationSet,
    incoming: &PublishedGenerationSet,
) -> Result<(), CoreError> {
    let current_manifest = current.manifest_generation.get();
    let incoming_manifest = incoming.manifest_generation.get();
    if incoming_manifest < current_manifest {
        return Err(CoreError::InvalidContract(format!(
            "stale activation: incoming manifest_generation={incoming_manifest} \
             < currently-active manifest_generation={current_manifest}"
        )));
    }

    if incoming.lexical_generation.get() < current.lexical_generation.get() {
        return Err(CoreError::InvalidContract(format!(
            "stale activation: incoming lexical_generation={} < active={}",
            incoming.lexical_generation.get(),
            current.lexical_generation.get(),
        )));
    }
    if incoming.symbol_generation.get() < current.symbol_generation.get() {
        return Err(CoreError::InvalidContract(format!(
            "stale activation: incoming symbol_generation={} < active={}",
            incoming.symbol_generation.get(),
            current.symbol_generation.get(),
        )));
    }
    monotonic_optional(
        "structural_generation",
        current
            .structural_generation
            .map(quanta_index_contract::GenerationId::get),
        incoming
            .structural_generation
            .map(quanta_index_contract::GenerationId::get),
    )?;
    monotonic_optional(
        "history_generation",
        current
            .history_generation
            .map(quanta_index_contract::GenerationId::get),
        incoming
            .history_generation
            .map(quanta_index_contract::GenerationId::get),
    )?;
    monotonic_optional(
        "semantic_generation",
        current
            .semantic_generation
            .map(quanta_index_contract::GenerationId::get),
        incoming
            .semantic_generation
            .map(quanta_index_contract::GenerationId::get),
    )?;
    monotonic_optional(
        "metadata_generation",
        current
            .metadata_generation
            .map(quanta_index_contract::GenerationId::get),
        incoming
            .metadata_generation
            .map(quanta_index_contract::GenerationId::get),
    )?;
    Ok(())
}

fn monotonic_optional(
    field: &str,
    current: Option<u64>,
    incoming: Option<u64>,
) -> Result<(), CoreError> {
    match (current, incoming) {
        (Some(a), Some(b)) if b < a => Err(CoreError::InvalidContract(format!(
            "stale activation: incoming {field}={b} < active={a}"
        ))),
        (Some(a), None) => Err(CoreError::InvalidContract(format!(
            "stale activation: incoming {field}=None regresses from active={a}"
        ))),
        _ => Ok(()),
    }
}
