use std::time::{SystemTime, UNIX_EPOCH};

use quanta_index_contract::{
    PublishedSearchBundleDeltaApplyRequest, PublishedSearchBundleDeltaApplyResponse,
    SearchBundleMutationOp,
};
use quanta_index_core::{BundlePolicy, CoreError, PublishedSearchBundleDeltaApplyPort};
use rusqlite::params;

use super::ControlPlane;
use super::helpers::sqlite_u64_to_i64;

/// Best-effort wall-clock ms since the Unix epoch.
///
/// Saturates at `0` if the system clock is set before 1970 (we don't
/// fail-closed on clock skew here — the only consumer of `applied_at_ms` is
/// operator-facing diagnostics).
fn current_unix_millis() -> u64 {
    let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return 0;
    };
    u64::try_from(duration.as_millis()).map_or(u64::MAX, |value| value)
}

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

        // Authority checks per SSOT (delta governance):
        //  1. The (repo, rev, generation) target must exist in the catalog.
        //  2. The target must NOT be the currently-active generation —
        //     deltas are applied during preparation, never against an active
        //     head, to preserve the immutable-active invariant.
        //  3. A canonical manifest must already be recorded for the target
        //     so consumers can resolve the delta's chunk/symbol identities
        //     against a known schema version.
        let target_manifest_gen = sqlite_u64_to_i64(
            "request.generation.manifest_generation",
            request.generation.manifest_generation.get(),
        )?;
        let catalog_known: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM generation_catalog \
                 WHERE repo_id = ?1 AND revision_id = ?2 AND manifest_generation = ?3",
                params![
                    request.generation.repo_id.as_str(),
                    request.generation.revision_id.as_str(),
                    target_manifest_gen,
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| CoreError::Storage(format!("count catalog rows failed: {error}")))?;
        if catalog_known == 0 {
            return Err(CoreError::InvalidContract(
                "delta target generation is not present in generation_catalog".into(),
            ));
        }

        let target_is_active: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM generation_activation_state \
                 WHERE repo_id = ?1 AND revision_id = ?2 AND active_manifest_generation = ?3",
                params![
                    request.generation.repo_id.as_str(),
                    request.generation.revision_id.as_str(),
                    target_manifest_gen,
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| {
                CoreError::Storage(format!("count active state rows failed: {error}"))
            })?;
        if target_is_active > 0 {
            return Err(CoreError::InvalidContract(
                "delta apply against currently-active generation is forbidden".into(),
            ));
        }

        let manifest_recorded: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM generation_manifest \
                 WHERE repo_id = ?1 AND revision_id = ?2 AND manifest_generation = ?3",
                params![
                    request.generation.repo_id.as_str(),
                    request.generation.revision_id.as_str(),
                    target_manifest_gen,
                ],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| CoreError::Storage(format!("count manifest rows failed: {error}")))?;
        if manifest_recorded == 0 {
            return Err(CoreError::InvalidContract(
                "delta apply requires a recorded manifest for the target generation".into(),
            ));
        }

        let repo = request.delta.repo_id.as_str().to_owned();
        let rev = request.delta.revision_id.as_str().to_owned();
        let manifest_generation = sqlite_u64_to_i64(
            "delta.manifest_generation",
            request.delta.manifest_generation.get(),
        )?;
        // Real wall-clock at apply time. Earlier revisions mistakenly wrote
        // `manifest_generation` here (which is a sequence number, not a ms
        // timestamp) — bug fix per reviewer P0.
        let applied_at_ms = sqlite_u64_to_i64("applied_at_ms", current_unix_millis())?;

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
