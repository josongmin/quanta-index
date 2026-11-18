//! Normal-engine facades of canonical persisted descriptors.
pub(crate) use crate::legacy::object_codec::{BlobRef, MapRoot, ObjectKind, ObjectRef, PageRef};
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MapEdit {
    pub(crate) key: Vec<u8>,
    pub(crate) value: Option<BlobRef>,
}
