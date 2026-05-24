use quanta_index_contract::{
    PublishedSearchBundleDeltaApplyRequest, PublishedSearchBundleDeltaApplyResponse,
    SearchBundleMutationOp,
};
use quanta_index_core::{BundlePolicy, CoreError, PublishedSearchBundleDeltaApplyPort};
use rusqlite::params;

use super::ControlPlane;
use super::helpers::sqlite_u64_to_i64;

const OP_UPSERT_CHUNK: &str = "upsert_chunk";
const OP_DELETE_CHUNK: &str = "delete_chunk";
const OP_UPSERT_SYMBOL: &str = "upsert_symbol";
const OP_DELETE_SYMBOL: &str = "delete_symbol";
const OP_UPSERT_EMBEDDING: &str = "upsert_embedding";
const OP_DELETE_EMBEDDING: &str = "delete_embedding";

#[derive(Clone, Copy)]
struct OpRow<'a> {
    kind: &'static str,
    target_id: &'a str,
    payload_digest: Option<&'a str>,
}

fn op_to_row(op: &SearchBundleMutationOp) -> OpRow<'_> {
    match op {
        SearchBundleMutationOp::UpsertChunk {
            chunk_identity,
            text_digest,
        } => OpRow {
            kind: OP_UPSERT_CHUNK,
            target_id: chunk_identity,
            payload_digest: Some(text_digest),
        },
        SearchBundleMutationOp::DeleteChunk { chunk_identity } => OpRow {
            kind: OP_DELETE_CHUNK,
            target_id: chunk_identity,
            payload_digest: None,
        },
        SearchBundleMutationOp::UpsertSymbol {
            symbol_id,
            symbol_digest,
        } => OpRow {
            kind: OP_UPSERT_SYMBOL,
            target_id: symbol_id,
            payload_digest: Some(symbol_digest),
        },
        SearchBundleMutationOp::DeleteSymbol { symbol_id } => OpRow {
            kind: OP_DELETE_SYMBOL,
            target_id: symbol_id,
            payload_digest: None,
        },
        SearchBundleMutationOp::UpsertEmbedding {
            entity_id,
            input_digest,
        } => OpRow {
            kind: OP_UPSERT_EMBEDDING,
            target_id: entity_id,
            payload_digest: Some(input_digest),
        },
        SearchBundleMutationOp::DeleteEmbedding { entity_id } => OpRow {
            kind: OP_DELETE_EMBEDDING,
            target_id: entity_id,
            payload_digest: None,
        },
    }
}

impl PublishedSearchBundleDeltaApplyPort for ControlPlane {
    fn apply_bundle_delta(
        &mut self,
        request: PublishedSearchBundleDeltaApplyRequest,
    ) -> Result<PublishedSearchBundleDeltaApplyResponse, CoreError> {
        BundlePolicy::validate_delta(&request)?;

        // The (repo, rev, generation) target must already exist in the
        // generation catalog. We reject deltas against unknown generations
        // fail-closed to avoid creating orphan delta rows.
        let catalog_known: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM generation_catalog \
                 WHERE repo_id = ?1 AND revision_id = ?2 AND manifest_generation = ?3",
                params![
                    request.generation.repo_id.as_str(),
                    request.generation.revision_id.as_str(),
                    sqlite_u64_to_i64(
                        "request.generation.manifest_generation",
                        request.generation.manifest_generation.get(),
                    )?,
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| CoreError::Storage(format!("count catalog rows failed: {error}")))?;
        if catalog_known == 0 {
            return Err(CoreError::InvalidContract(
                "delta target generation is not present in generation_catalog".into(),
            ));
        }

        let repo = request.delta.repo_id.as_str().to_owned();
        let rev = request.delta.revision_id.as_str().to_owned();
        let manifest_generation = sqlite_u64_to_i64(
            "delta.manifest_generation",
            request.delta.manifest_generation.get(),
        )?;
        let applied_at_ms = sqlite_u64_to_i64(
            "applied_at_ms",
            request.generation.manifest_generation.get(),
        )?;

        let tx = self
            .conn
            .transaction()
            .map_err(|error| CoreError::Storage(format!("begin delta tx failed: {error}")))?;

        let mut newly_inserted: u64 = 0;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR IGNORE INTO bundle_delta_applied (\
                        repo_id, revision_id, manifest_generation, op_kind, \
                        target_id, payload_digest, applied_at_ms\
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .map_err(|error| {
                    CoreError::Storage(format!("prepare delta insert failed: {error}"))
                })?;
            for op in &request.delta.operations {
                let row = op_to_row(op);
                let changed = stmt
                    .execute(params![
                        repo,
                        rev,
                        manifest_generation,
                        row.kind,
                        row.target_id,
                        row.payload_digest,
                        applied_at_ms,
                    ])
                    .map_err(|error| {
                        CoreError::Storage(format!("insert delta op failed: {error}"))
                    })?;
                let inserted_u64 = u64::try_from(changed).map_err(|error| {
                    CoreError::Storage(format!("insert delta op rowcount overflow: {error}"))
                })?;
                newly_inserted = newly_inserted.saturating_add(inserted_u64);
            }
        }

        tx.commit()
            .map_err(|error| CoreError::Storage(format!("commit delta tx failed: {error}")))?;

        let total_ops = u64::try_from(request.delta.operations.len())
            .map_err(|error| CoreError::Storage(format!("operations.len overflow: {error}")))?;
        let applied = newly_inserted > 0;
        let reason = if applied {
            None
        } else if total_ops == 0 {
            Some("delta carried no operations".into())
        } else {
            Some("delta already applied".into())
        };

        Ok(PublishedSearchBundleDeltaApplyResponse {
            applied,
            indexed_generation: request.generation,
            reason,
        })
    }
}
