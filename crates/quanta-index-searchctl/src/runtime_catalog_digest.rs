//! Offline verification of the catalog digest carried by a publication record.

use std::io::Read;

use quanta_index_contract::RuntimeCatalogIngestBatch;
use quanta_index_ipc::{BatchDigestVerdictV1, verify_batch_digest_v1};
use serde_json::Value;

const MAX_FACTS_RECORD_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn verify_facts_record(input: impl Read) -> Result<String, String> {
    let mut body = Vec::new();
    input
        .take(MAX_FACTS_RECORD_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| format!("read RuntimeCatalog facts record: {error}"))?;
    if body.len() as u64 > MAX_FACTS_RECORD_BYTES {
        return Err("RuntimeCatalog facts record exceeds 16 MiB".into());
    }
    let mut record: Value = serde_json::from_slice(&body)
        .map_err(|error| format!("decode RuntimeCatalog facts record: {error}"))?;
    let catalog = record
        .as_object_mut()
        .and_then(|fields| fields.remove("catalog"))
        .ok_or("RuntimeCatalog facts record lacks catalog")?;
    let batch: RuntimeCatalogIngestBatch = serde_json::from_value(catalog)
        .map_err(|error| format!("decode RuntimeCatalog catalog: {error}"))?;
    match verify_batch_digest_v1(&batch)
        .map_err(|error| format!("verify RuntimeCatalog batch digest: {error}"))?
    {
        BatchDigestVerdictV1::Verified(_) => Ok(batch.batch_digest),
        BatchDigestVerdictV1::Mismatch { .. } => {
            Err("RuntimeCatalog batch digest differs from its facts".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_ipc::stamp_batch_digest_v1;
    use serde_json::json;

    #[test]
    fn verify_catalog_facts_refuses_changed_body_with_prior_digest() {
        let mut batch: RuntimeCatalogIngestBatch = serde_json::from_value(json!({
            "repo_id": "repo",
            "revision_id": "rev",
            "generation": 12,
            "overlay_epoch_ms": 1001,
            "batch_digest": "",
            "producer_head_applied_at_ms": 1002,
            "generation_materialized_at_ms": 1003,
            "changed_entries": [],
            "facet_entries": [],
            "snapshot_entries": [],
            "affected_entries": [],
            "invalidated_by_entries": [],
        }))
        .unwrap();
        stamp_batch_digest_v1(&mut batch).unwrap();
        let valid = serde_json::to_vec(&json!({ "catalog": &batch })).unwrap();
        assert_eq!(verify_facts_record(&valid[..]).unwrap(), batch.batch_digest);

        batch.producer_head_applied_at_ms += 1;
        let changed = serde_json::to_vec(&json!({ "catalog": &batch })).unwrap();
        assert_eq!(
            verify_facts_record(&changed[..]).unwrap_err(),
            "RuntimeCatalog batch digest differs from its facts",
        );
    }
}
