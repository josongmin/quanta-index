use quanta_index_contract::{
    PublishedSearchBundlePrepareRequest, PublishedSearchBundlePrepareResponse,
};
use quanta_index_core::{BundlePolicy, CoreError, PublishedSearchBundlePreparePort};
use rusqlite::params;

use super::ControlPlane;
use super::helpers::{encoding_to_str, mode_to_str, sqlite_u64_to_i64};

impl PublishedSearchBundlePreparePort for ControlPlane {
    fn prepare_bundle(
        &mut self,
        request: PublishedSearchBundlePrepareRequest,
    ) -> Result<PublishedSearchBundlePrepareResponse, CoreError> {
        BundlePolicy::validate_outbox(&request.outbox)?;

        let inserted_rows = self
            .conn
            .execute(
                "
                INSERT OR IGNORE INTO prepared_bundle_outbox (
                    outbox_id,
                    repo_id,
                    revision_id,
                    manifest_digest,
                    bundle_schema_version,
                    prepared_at_ms,
                    mode,
                    manifest_ref_relative_path,
                    manifest_ref_encoding,
                    manifest_ref_byte_length,
                    manifest_ref_content_digest,
                    base_generation,
                    changed_artifact_mask
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ",
                params![
                    request.outbox.outbox_id,
                    request.outbox.repo_id.as_str(),
                    request.outbox.revision_id.as_str(),
                    request.outbox.manifest_digest.as_str(),
                    request.outbox.bundle_schema_version,
                    sqlite_u64_to_i64("prepared_at_ms", request.outbox.prepared_at_ms)?,
                    mode_to_str(request.outbox.mode),
                    request.outbox.manifest_ref.relative_path,
                    encoding_to_str(request.outbox.manifest_ref.encoding),
                    sqlite_u64_to_i64(
                        "manifest_ref.byte_length",
                        request.outbox.manifest_ref.byte_length,
                    )?,
                    request.outbox.manifest_ref.content_digest.as_str(),
                    request
                        .outbox
                        .base_generation
                        .map(|value| sqlite_u64_to_i64("base_generation", value.get()))
                        .transpose()?,
                    sqlite_u64_to_i64(
                        "changed_artifact_mask",
                        request.outbox.changed_artifact_mask,
                    )?,
                ],
            )
            .map_err(|error| CoreError::Storage(format!("insert outbox failed: {error}")))?;

        let accepted = inserted_rows > 0;

        Ok(PublishedSearchBundlePrepareResponse {
            accepted,
            external_bundle_id: request.outbox.outbox_id,
            state: "prepared".into(),
            reason: (!accepted).then(|| "prepared bundle already exists".into()),
        })
    }
}
