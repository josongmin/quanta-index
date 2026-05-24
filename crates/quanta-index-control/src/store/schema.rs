use quanta_index_core::CoreError;

use super::ControlPlane;

impl ControlPlane {
    pub(super) fn bootstrap_schema(&self) -> Result<(), CoreError> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS schema_version (
                    singleton_key INTEGER PRIMARY KEY CHECK(singleton_key = 1),
                    version INTEGER NOT NULL
                );
                INSERT OR IGNORE INTO schema_version (singleton_key, version) VALUES (1, 1);

                CREATE TABLE IF NOT EXISTS prepared_bundle_outbox (
                    outbox_id TEXT PRIMARY KEY,
                    repo_id TEXT NOT NULL,
                    revision_id TEXT NOT NULL,
                    manifest_digest TEXT NOT NULL,
                    bundle_schema_version INTEGER NOT NULL,
                    prepared_at_ms INTEGER NOT NULL,
                    mode TEXT NOT NULL,
                    manifest_ref_relative_path TEXT NOT NULL,
                    manifest_ref_encoding TEXT NOT NULL,
                    manifest_ref_byte_length INTEGER NOT NULL,
                    manifest_ref_content_digest TEXT NOT NULL,
                    base_generation INTEGER NULL,
                    changed_artifact_mask INTEGER NOT NULL,
                    claim_state TEXT NOT NULL DEFAULT 'prepared',
                    UNIQUE(repo_id, revision_id, manifest_digest)
                );

                CREATE TABLE IF NOT EXISTS generation_catalog (
                    repo_id TEXT NOT NULL,
                    revision_id TEXT NOT NULL,
                    manifest_generation INTEGER NOT NULL,
                    lexical_generation INTEGER NOT NULL,
                    symbol_generation INTEGER NOT NULL,
                    structural_generation INTEGER NULL,
                    history_generation INTEGER NULL,
                    semantic_generation INTEGER NULL,
                    metadata_generation INTEGER NULL,
                    state TEXT NOT NULL,
                    PRIMARY KEY (repo_id, revision_id, manifest_generation)
                );

                CREATE TABLE IF NOT EXISTS generation_activation_state (
                    repo_id TEXT NOT NULL,
                    revision_id TEXT NOT NULL,
                    active_manifest_generation INTEGER NOT NULL,
                    lexical_ready INTEGER NOT NULL,
                    semantic_ready INTEGER NOT NULL,
                    active_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (repo_id, revision_id)
                );

                CREATE TABLE IF NOT EXISTS external_search_consumer_ack (
                    outbox_id TEXT PRIMARY KEY,
                    external_bundle_id TEXT NOT NULL,
                    ack_state TEXT NOT NULL,
                    ack_reason TEXT NULL,
                    acked_at_ms INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS indexing_jobs (
                    job_id TEXT PRIMARY KEY,
                    job_kind TEXT NOT NULL,
                    job_state TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS indexing_job_dependency_edges (
                    parent_job_id TEXT NOT NULL,
                    child_job_id TEXT NOT NULL,
                    PRIMARY KEY (parent_job_id, child_job_id)
                );

                CREATE TABLE IF NOT EXISTS indexing_pending_publish_closeouts (
                    closeout_id TEXT PRIMARY KEY,
                    manifest_digest TEXT NOT NULL,
                    state TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS indexing_failed_publish_replay_snapshots (
                    snapshot_id TEXT PRIMARY KEY,
                    manifest_digest TEXT NOT NULL,
                    failure_reason TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS bundle_delta_applied (
                    repo_id TEXT NOT NULL,
                    revision_id TEXT NOT NULL,
                    manifest_generation INTEGER NOT NULL,
                    op_kind TEXT NOT NULL,
                    target_id TEXT NOT NULL,
                    payload_digest TEXT NULL,
                    applied_at_ms INTEGER NOT NULL,
                    PRIMARY KEY (repo_id, revision_id, manifest_generation, op_kind, target_id)
                );

                CREATE TABLE IF NOT EXISTS generation_manifest (
                    repo_id TEXT NOT NULL,
                    revision_id TEXT NOT NULL,
                    manifest_generation INTEGER NOT NULL,
                    manifest_json TEXT NOT NULL,
                    PRIMARY KEY (repo_id, revision_id, manifest_generation)
                );
                ",
            )
            .map_err(|error| CoreError::Storage(format!("bootstrap schema failed: {error}")))?;
        Ok(())
    }
}
