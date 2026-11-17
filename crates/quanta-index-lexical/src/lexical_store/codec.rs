//! Normal-engine facades of the ONE canonical persisted descriptor definitions.
pub(crate) use crate::legacy::object_codec::{MapRoot, ObjectKind, ObjectRef, PageRef};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
impl ObjectKind {
    pub(super) const fn is_payload(self) -> bool {
        !matches!(
            self,
            Self::MapPage
                | Self::BlobBlock
                | Self::BlobNode
                | Self::AttemptBatch
                | Self::HardScanAnchor
        )
    }
}
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct BlobRef {
    pub(crate) root: ObjectRef,
    /// Logical payload bytes, independent of physical block count.
    pub(crate) bytes: u64,
    pub(crate) sha256: [u8; 32],
}

impl Serialize for BlobRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.root, self.bytes, self.sha256).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BlobRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (root, bytes, sha256) = <(ObjectRef, u64, [u8; 32])>::deserialize(deserializer)?;
        Ok(Self {
            root,
            bytes,
            sha256,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MapEdit {
    pub(crate) key: Vec<u8>,
    pub(crate) value: Option<BlobRef>,
}
