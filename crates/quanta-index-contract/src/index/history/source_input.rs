//! Bounded source payload carried by the existing opaque immutable stager.
//! These are the original History batch/ACK DTOs, not derived index artifacts.
use super::{AuxiliaryGenerationKeyV1, Digest};
use crate::{BatchPublishReceipt, HistoryIngestBatch};

/// Supplied by the authenticated complete-inventory owner under its write fence.
/// Surviving ACKs or a target row scan cannot issue this inventory authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryReplayFenceV1 {
    pub generation: AuxiliaryGenerationKeyV1,
    pub manifest_digest: Option<String>,
    pub publication_count: u64,
    pub first_replay: Digest,
    pub final_replay: Digest,
    pub final_durable_sequence: u64,
}
crate::native_serde_struct!(HistoryReplayFenceV1 {
    generation: AuxiliaryGenerationKeyV1, manifest_digest: Option<String>, publication_count: u64,
    first_replay: Digest, final_replay: Digest, final_durable_sequence: u64
});

/// One original publication and its authenticated replay positions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryReplayPublicationV1 {
    pub ordinal: u64,
    pub before: Digest,
    pub after: Digest,
    pub batch: HistoryIngestBatch,
    pub ack: BatchPublishReceipt,
}
crate::native_serde_struct!(HistoryReplayPublicationV1 {
    ordinal: u64,
    before: Digest,
    after: Digest,
    batch: HistoryIngestBatch,
    ack: BatchPublishReceipt
});

/// Canonical JSON source payload. FULL contains the entire ordinal1..fence
/// sequence. DELTA contains exactly one successor; the existing transition and
/// actual prior member distinguish the two. This value grants no read/write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistorySourceInputV1 {
    pub wire_version: u32,
    pub fence: HistoryReplayFenceV1,
    pub publications: Vec<HistoryReplayPublicationV1>,
}
crate::native_serde_struct!(HistorySourceInputV1 {
    wire_version: u32, fence: HistoryReplayFenceV1, publications: Vec<HistoryReplayPublicationV1>
});

/// The existing immutable object format's payload ceiling; this does not raise
/// IPC frame or Original allocation limits, which may refuse sooner.
pub const MAX_HISTORY_SOURCE_INPUT_BYTES_V1: usize = 16 * 1024 * 1024;

impl HistorySourceInputV1 {
    pub fn validate_shape(&self) -> Result<(), &'static str> {
        if self.wire_version != 1
            || self.publications.is_empty()
            || u64::try_from(self.publications.len())
                .map_err(|_| "History publication count exceeds u64")?
                > self.fence.publication_count
            || self.fence.publication_count == 0
            || self.fence.final_durable_sequence == 0
            || self.fence.first_replay == Digest([0; 32])
            || self.fence.final_replay == Digest([0; 32])
            || self.fence.first_replay == self.fence.final_replay
        {
            return Err(
                "History source input version, cardinality or fenced replay position is invalid",
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixed wire data, independently of this carrier's serializer. This empty
    // batch exercises codec shape only; it is not a complete replay fixture.
    fn wire() -> serde_json::Value {
        serde_json::json!({
            "wire_version": 1,
            "fence": {
                "generation": {"repo_id": "repo", "revision_id": "rev", "generation": 7},
                "manifest_digest": null,
                "publication_count": 1,
                "first_replay": vec![1_u8; 32],
                "final_replay": vec![2_u8; 32],
                "final_durable_sequence": 17
            },
            "publications": [{
                "ordinal": 1,
                "before": vec![1_u8; 32],
                "after": vec![2_u8; 32],
                "batch": {
                    "repo_id": "repo", "revision_id": "rev", "generation": 7,
                    "manifest_digest": null, "batch_digest": "original-body-digest",
                    "commits": [], "refs": [], "tags": [], "diff_hunks": []
                },
                "ack": {
                    "generation": 7, "manifest_digest": null,
                    "batch_digest": "original-body-digest",
                    "accepted_replace_scopes": 0, "accepted_tombstone_scopes": 0,
                    "accepted_semantic_replace_scopes": 0,
                    "accepted_semantic_tombstone_scopes": 0,
                    "accepted_clear_surfaces": 0, "sealed": false, "applied": true,
                    "durable_sequence": 17, "semantic_content": null
                }
            }]
        })
    }

    #[test]
    fn history_source_carrier_keeps_originals_and_rejects_schema_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixed = wire();
        let decoded: HistorySourceInputV1 = serde_json::from_value(fixed.clone())?;
        decoded.validate_shape().map_err(std::io::Error::other)?;
        assert_eq!(decoded.fence.generation.repo_id.as_str(), "repo");
        assert_eq!(
            decoded.publications[0].batch.batch_digest,
            "original-body-digest"
        );
        assert_eq!(decoded.publications[0].ack.durable_sequence, 17);
        assert_eq!(serde_json::to_value(&decoded)?, fixed);

        for path in [
            &["wire_version"][..],
            &["fence", "manifest_digest"][..],
            &["publications", "0", "before"][..],
        ] {
            let mut missing = fixed.clone();
            let removed = match path {
                [field] => missing.as_object_mut().ok_or("root object")?.remove(*field),
                ["fence", field] => missing["fence"]
                    .as_object_mut()
                    .ok_or("fence object")?
                    .remove(*field),
                ["publications", "0", field] => missing["publications"][0]
                    .as_object_mut()
                    .ok_or("publication object")?
                    .remove(*field),
                _ => return Err("unexpected test path".into()),
            };
            assert!(removed.is_some());
            assert!(serde_json::from_value::<HistorySourceInputV1>(missing).is_err());
        }
        let mut unknown = fixed.clone();
        unknown["fence"]["complete"] = serde_json::json!(true);
        assert!(serde_json::from_value::<HistorySourceInputV1>(unknown).is_err());
        let duplicate = serde_json::to_string(&fixed)?.replacen(
            "\"wire_version\":1",
            "\"wire_version\":1,\"wire_version\":1",
            1,
        );
        assert!(serde_json::from_str::<HistorySourceInputV1>(&duplicate).is_err());

        for (field, invalid) in [
            ("wire_version", serde_json::json!(2)),
            ("publications", serde_json::json!([])),
        ] {
            let mut bad = fixed.clone();
            bad[field] = invalid;
            let carrier: HistorySourceInputV1 = serde_json::from_value(bad)?;
            assert!(carrier.validate_shape().is_err());
        }
        Ok(())
    }
}
