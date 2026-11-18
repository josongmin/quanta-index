//! Exact persisted Catalog DATA for an independently authenticated Native
//! protected-workspace transfer. It cannot authorize a Source publication.
use super::{CatalogView, Digest, ObjectRef, ProducerId, StreamId, WorkspaceKey};
use serde::{Deserialize, Serialize};

pub const MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1: usize = 32 * 1024 * 1024;
pub const MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1: usize = 16_384;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProtectedFrontierV1 {
    pub workspace: WorkspaceKey,
    pub producer: ProducerId,
    pub stream: StreamId,
    pub stream_epoch: u64,
    pub sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProtectedSourceAuthorityV1 {
    pub workspace: WorkspaceKey,
    pub workspace_epoch: u64,
    pub producer: ProducerId,
    pub source_epoch: u64,
}

/// Operation and event-claim payloads use the ONE canonical persisted Catalog
/// codecs. No substitute event, fence, Source epoch or ACK is synthesized.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProtectedNativeSnapshotV1 {
    pub wire_version: u32,
    pub source_root_nonce: Digest,
    pub source_routes: ObjectRef,
    pub operation_rows: Vec<Vec<u8>>,
    pub genesis_views: Vec<CatalogView>,
    pub frontiers: Vec<NativeProtectedFrontierV1>,
    pub source_authorities: Vec<NativeProtectedSourceAuthorityV1>,
    pub event_claims: Vec<Vec<u8>>,
    /// Sorted exact immutable closure roots derived by the Catalog supplier.
    pub roots: Vec<ObjectRef>,
}

impl NativeProtectedNativeSnapshotV1 {
    pub const WIRE_VERSION: u32 = 1;

    pub fn validate_shape_v1(&self) -> Result<(), &'static str> {
        let zero = Digest([0; 32]);
        let rows = [
            self.operation_rows.len(),
            self.genesis_views.len(),
            self.frontiers.len(),
            self.source_authorities.len(),
            self.event_claims.len(),
        ]
        .into_iter()
        .try_fold(0usize, |total, count| total.checked_add(count));
        if rows.is_none_or(|count| count > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1)
            || self.wire_version != Self::WIRE_VERSION
            || self.source_root_nonce == zero
            || self.source_routes.0 == zero
            || self.operation_rows.is_empty()
            || self.operation_rows.len() > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
            || self.genesis_views.len() > super::MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1
            || self.frontiers.len() > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
            || self.source_authorities.len() > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
            || self.event_claims.len() > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
            || self.roots.is_empty()
            || self.roots.len() > crate::native_protocol::MAX_NATIVE_PROJECTION_OUTPUT_CHILDREN
            || !self.roots.contains(&self.source_routes)
            || self.roots.iter().any(|root| root.0 == zero)
            || self.roots.windows(2).any(|pair| pair[0] >= pair[1])
            || self
                .operation_rows
                .iter()
                .chain(&self.event_claims)
                .any(Vec::is_empty)
        {
            return Err("protected Native snapshot shape differs");
        }
        Ok(())
    }

    pub fn encoded_payload_len_v1(&self) -> Result<usize, &'static str> {
        self.validate_shape_v1()?;
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self
                    .0
                    .checked_add(bytes.len())
                    .filter(|count| *count <= MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)
                    .ok_or_else(|| std::io::Error::other("protected snapshot exceeds bound"))?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut counter = Counter(0);
        serde_json::to_writer(&mut counter, self).map_err(|_| "count protected Native snapshot")?;
        Ok(counter.0)
    }

    pub fn object_ref_v1(&self) -> Result<ObjectRef, &'static str> {
        let expected = self.encoded_payload_len_v1()?;
        let bytes = serde_json::to_vec(self).map_err(|_| "encode protected Native snapshot")?;
        if bytes.len() != expected {
            return Err("protected Native snapshot changed");
        }
        super::native_immutable_object_ref_v1(&bytes, &self.roots)
            .ok_or("protected Native snapshot object overflows")
    }
}
