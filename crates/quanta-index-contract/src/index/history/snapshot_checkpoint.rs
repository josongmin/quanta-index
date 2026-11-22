//! A provisional paged SQL snapshot carrier for History FULL bootstrap.
//! The receiver independently verifies Legacy Catalog rows, terminal ACK and
//! producer replay scope before installing any cut.

use super::{AuxiliaryGenerationKeyV1, HistorySourceCutV1};
use crate::{BatchPublishReceipt, RepoId, index::ObjectRef};

mod export_decode;

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistorySnapshotExportFrameV1 {
    NativeProtectedComplete {
        source_root_nonce: crate::index::Digest,
        source_routes: ObjectRef,
        snapshot: Option<ObjectRef>,
    },
    NativeObjectBegin {
        reference: ObjectRef,
        payload_bytes: u64,
        children: Vec<ObjectRef>,
    },
    NativeObjectChunk {
        offset: u64,
        bytes: Vec<u8>,
    },
    NativeObjectEnd {
        reference: ObjectRef,
    },
    NativeHistory {
        source_root_nonce: crate::index::Digest,
        source_view: crate::index::ViewRef,
        instance: crate::index::IndexInstanceId,
        member: ObjectRef,
        cut: HistorySourceCutV1,
    },
    NativeInventoryComplete {
        repo_id: RepoId,
        count: u64,
    },
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

macro_rules! history_export_body {
    ($serializer:ident, $name:literal, { $($field:ident),+ $(,)? }) => {{
        let mut body = $serializer.serialize_struct($name, [$(stringify!($field)),+].len())?;
        $(serde::ser::SerializeStruct::serialize_field(&mut body, stringify!($field), $field)?;)+
        serde::ser::SerializeStruct::end(body)
    }};
}

struct HistoryExportBodyRef<'a>(&'a HistorySnapshotExportFrameV1);

impl serde::Serialize for HistoryExportBodyRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            HistorySnapshotExportFrameV1::NativeProtectedComplete {
                source_root_nonce,
                source_routes,
                snapshot,
            } => history_export_body!(serializer, "NativeProtectedComplete", {
                source_root_nonce, source_routes, snapshot
            }),
            HistorySnapshotExportFrameV1::NativeObjectBegin {
                reference,
                payload_bytes,
                children,
            } => history_export_body!(serializer, "NativeObjectBegin", {
                reference, payload_bytes, children
            }),
            HistorySnapshotExportFrameV1::NativeObjectChunk { offset, bytes } => {
                history_export_body!(serializer, "NativeObjectChunk", { offset, bytes })
            }
            HistorySnapshotExportFrameV1::NativeObjectEnd { reference } => {
                history_export_body!(serializer, "NativeObjectEnd", { reference })
            }
            HistorySnapshotExportFrameV1::NativeHistory {
                source_root_nonce,
                source_view,
                instance,
                member,
                cut,
            } => history_export_body!(serializer, "NativeHistory", {
                source_root_nonce, source_view, instance, member, cut
            }),
            HistorySnapshotExportFrameV1::NativeInventoryComplete { repo_id, count } => {
                history_export_body!(serializer, "NativeInventoryComplete", { repo_id, count })
            }
            HistorySnapshotExportFrameV1::Page(page) => {
                serde::Serialize::serialize(page, serializer)
            }
            HistorySnapshotExportFrameV1::Complete {
                generation,
                legacy_root,
                durable_epoch,
                snapshot_digest,
                row_count,
                terminal_ack,
            } => history_export_body!(serializer, "Complete", {
                generation, legacy_root, durable_epoch, snapshot_digest, row_count, terminal_ack
            }),
            HistorySnapshotExportFrameV1::InventoryKey { generation } => {
                history_export_body!(serializer, "InventoryKey", { generation })
            }
            HistorySnapshotExportFrameV1::InventoryComplete { repo_id, count } => {
                history_export_body!(serializer, "InventoryComplete", { repo_id, count })
            }
        }
    }
}

impl serde::Serialize for HistorySnapshotExportFrameV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let kind = match self {
            Self::NativeProtectedComplete { .. } => "native_protected_complete",
            Self::NativeObjectBegin { .. } => "native_object_begin",
            Self::NativeObjectChunk { .. } => "native_object_chunk",
            Self::NativeObjectEnd { .. } => "native_object_end",
            Self::NativeHistory { .. } => "native_history",
            Self::NativeInventoryComplete { .. } => "native_inventory_complete",
            Self::Page(_) => "page",
            Self::Complete { .. } => "complete",
            Self::InventoryKey { .. } => "inventory_key",
            Self::InventoryComplete { .. } => "inventory_complete",
        };
        let mut frame = serializer.serialize_struct("HistorySnapshotExportFrameV1", 2)?;
        serde::ser::SerializeStruct::serialize_field(&mut frame, "kind", kind)?;
        serde::ser::SerializeStruct::serialize_field(
            &mut frame,
            "body",
            &HistoryExportBodyRef(self),
        )?;
        serde::ser::SerializeStruct::end(frame)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_frame_body_first_preserves_named_and_binary_fields() {
        let expected = HistorySnapshotExportFrameV1::NativeObjectChunk {
            offset: 3,
            bytes: vec![255],
        };
        let json = br#"{"body":{"offset":3,"bytes":[255],"ignored":null},"kind":{"native_object_chunk":null}}"#;
        assert_eq!(
            serde_json::from_slice::<HistorySnapshotExportFrameV1>(json).expect("JSON"),
            expected
        );
        let duplicate =
            br#"{"body":{"offset":3,"offset":4,"bytes":[255]},"kind":"native_object_chunk"}"#;
        assert!(serde_json::from_slice::<HistorySnapshotExportFrameV1>(duplicate).is_err());

        // The previous decoder accepts byte-string keys for the envelope
        // and the variant body, including unknown non-UTF-8 field names.
        let cbor = b"\xa2\x44body\xa3\x46offset\x03\x45bytes\x81\x18\xff\x41\xff\xf6\x44kind\x73native_object_chunk";
        assert_eq!(
            ciborium::from_reader::<HistorySnapshotExportFrameV1, _>(cbor.as_slice())
                .expect("CBOR"),
            expected
        );
        // A body-first CBOR bignum must not be normalized to an ordinary offset.
        let bignum = b"\xa2\x44body\xa2\x46offset\xc2\x41\x05\x45bytes\x81\x18\xff\x44kind\x73native_object_chunk";
        assert!(
            ciborium::from_reader::<HistorySnapshotExportFrameV1, _>(bignum.as_slice()).is_err()
        );
    }

    #[test]
    fn export_frames_preserve_derived_json_and_cbor_bytes() {
        let cases = [
            (
                HistorySnapshotExportFrameV1::NativeObjectChunk {
                    offset: 3,
                    bytes: vec![1, 2, 255],
                },
                r#"{"kind":"native_object_chunk","body":{"offset":3,"bytes":[1,2,255]}}"#,
                "a2646b696e64736e61746976655f6f626a6563745f6368756e6b64626f6479a2666f66667365740365627974657383010218ff",
            ),
            (
                HistorySnapshotExportFrameV1::InventoryComplete {
                    repo_id: RepoId::new("repo").expect("fixture repo"),
                    count: 4,
                },
                r#"{"kind":"inventory_complete","body":{"repo_id":"repo","count":4}}"#,
                "a2646b696e6472696e76656e746f72795f636f6d706c65746564626f6479a2677265706f5f6964647265706f65636f756e7404",
            ),
        ];

        for (frame, expected_json, expected_cbor) in cases {
            let json = serde_json::to_string(&frame).expect("JSON export frame");
            assert_eq!(json, expected_json);
            assert_eq!(
                serde_json::from_str::<HistorySnapshotExportFrameV1>(&json).expect("JSON readback"),
                frame
            );

            let mut cbor = Vec::new();
            ciborium::into_writer(&frame, &mut cbor).expect("CBOR export frame");
            let encoded = cbor
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(encoded, expected_cbor);
            assert_eq!(
                ciborium::from_reader::<HistorySnapshotExportFrameV1, _>(cbor.as_slice())
                    .expect("CBOR readback"),
                frame
            );
        }
    }
}
