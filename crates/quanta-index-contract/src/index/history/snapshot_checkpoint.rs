//! A provisional paged SQL snapshot carrier for History FULL bootstrap.
//! The receiver independently verifies Legacy Catalog rows, terminal ACK and
//! producer replay scope before installing any cut.

use super::{AuxiliaryGenerationKeyV1, HistorySourceCutV1};
use crate::{BatchPublishReceipt, RepoId, index::ObjectRef};

/// Page framing shares the retained immutable input bound. The exporter may
/// choose smaller pages, while the original request can refuse earlier.
pub const MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1: usize = super::MAX_HISTORY_SOURCE_INPUT_BYTES_V1;
pub const MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1: usize = 1024;

/// One SQL row's stored History key and canonical encoded value. The page
/// generation and closed family grammar supply the rest of its address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistorySnapshotRowV1 {
    pub family: String,
    pub row_key: Vec<u8>,
    pub value: Vec<u8>,
}
crate::native_serde_struct!(HistorySnapshotRowV1 {
    family: String, row_key: Vec<u8>, value: Vec<u8>
});

/// One immutable, childless page in physical SQL row order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistorySnapshotPageV1 {
    pub wire_version: u32,
    pub generation: AuxiliaryGenerationKeyV1,
    pub ordinal: u32,
    pub rows: Vec<HistorySnapshotRowV1>,
}
crate::native_serde_struct!(HistorySnapshotPageV1 {
    wire_version: u32, generation: AuxiliaryGenerationKeyV1,
    ordinal: u32, rows: Vec<HistorySnapshotRowV1>
});

impl HistorySnapshotPageV1 {
    pub fn validate_shape(&self) -> Result<(), &'static str> {
        if self.wire_version != 1
            || self.ordinal == 0
            || self.rows.is_empty()
            || self.rows.len() > MAX_HISTORY_SNAPSHOT_ROWS_PER_PAGE_V1
            || self.rows.iter().any(|row| {
                !matches!(
                    row.family.as_str(),
                    "epoch" | "commit" | "ref" | "tag" | "diff-hunk" | "state-meta"
                ) || row.row_key.len() > MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1
                    || row.value.len() > MAX_HISTORY_SNAPSHOT_PAGE_BYTES_V1
            })
        {
            return Err("History snapshot page version, row count or row shape is invalid");
        }
        Ok(())
    }
}

/// Private child stdout stream. Checkpoint pages are provisional until Complete;
/// inventory keys are provisional until InventoryComplete. Native BOOT later
/// independently verifies the saved Legacy source.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum HistorySnapshotExportFrameV1 {
    Page(HistorySnapshotPageV1),
    Complete {
        generation: AuxiliaryGenerationKeyV1,
        legacy_root: String,
        durable_epoch: crate::AuxEpochV1,
        snapshot_digest: crate::index::Digest,
        row_count: u64,
        terminal_ack: BatchPublishReceipt,
    },
    InventoryKey {
        generation: AuxiliaryGenerationKeyV1,
    },
    InventoryComplete {
        repo_id: RepoId,
        count: u64,
    },
}

/// Version 2 is distinct from the retained-original HistorySourceInputV1.
/// The terminal ACK is observed DATA; no receipt or replay position is minted
/// by decoding this carrier. Pages are ordered here; immutable direct children
/// are the same references in sorted, unique content-address order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistorySnapshotCheckpointV1 {
    pub wire_version: u32,
    pub cut: HistorySourceCutV1,
    pub terminal_ack: BatchPublishReceipt,
    pub row_count: u64,
    pub pages: Vec<ObjectRef>,
}
crate::native_serde_struct!(HistorySnapshotCheckpointV1 {
    wire_version: u32, cut: HistorySourceCutV1,
    terminal_ack: BatchPublishReceipt, row_count: u64, pages: Vec<ObjectRef>
});

impl HistorySnapshotCheckpointV1 {
    pub fn validate_shape(&self) -> Result<(), &'static str> {
        if self.wire_version != 2
            || self.cut.durable_epoch == crate::AuxEpochV1::GENESIS
            || self.cut.snapshot_digest.0 == [0; 32]
            || self.cut.replay_start.0 == [0; 32]
            || self.row_count < 2
            || self.pages.is_empty()
            || self.pages.len() > crate::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN
            || self.terminal_ack.generation != self.cut.generation.generation
            || self.terminal_ack.durable_sequence == 0
            || !self.terminal_ack.applied
            || !crate::is_canonical_batch_digest_token_v1(&self.terminal_ack.batch_digest)
            || self.terminal_ack.sealed
            || self.terminal_ack.semantic_content.is_some()
            || self.terminal_ack.accepted_semantic_replace_scopes != 0
            || self.terminal_ack.accepted_semantic_tombstone_scopes != 0
            || self.terminal_ack.accepted_clear_surfaces != 0
        {
            return Err("History snapshot checkpoint shape or terminal ACK is invalid");
        }
        Ok(())
    }
}
