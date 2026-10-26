//! Headless action certification over a saved, authenticated source frontier.
//! The consumer must recheck the entire binding before an atomic edit apply.

mod coordinates;

use super::{NativeIndexResponse, NativeOverlayDocument};
use crate::index::{Digest, ObjectRef, ViewRef};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeStubBinding {
    pub document: NativeOverlayDocument,
    pub revision: u64,
    pub selected: ViewRef,
    pub source: ObjectRef,
    pub language: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeStubItem {
    pub stable_id: Digest,
    pub ast_digest: Digest,
    pub start: u32,
    pub end: u32,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeStubPosition {
    pub line: u32,
    pub column: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeStubRange {
    pub start: NativeStubPosition,
    pub end: NativeStubPosition,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeStubCoordinateUnit {
    Utf8Bytes,
    Utf16Units,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeStubEdit {
    pub target_id: Digest,
    pub expected_ast: Digest,
    pub range: NativeStubRange,
    pub replacement: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeStubByteEdit {
    pub target_id: Digest,
    pub start: u32,
    pub end: u32,
    pub replacement: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeStubRequest {
    Certify {
        binding: NativeStubBinding,
        items: Vec<NativeStubItem>,
    },
    Preflight {
        binding: NativeStubBinding,
        items: Vec<NativeStubItem>,
        selected_id: Digest,
        edits: Vec<NativeStubEdit>,
        unit: NativeStubCoordinateUnit,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeStubReply {
    Certified {
        binding: NativeStubBinding,
        items: Vec<NativeStubItem>,
        action_supported: bool,
    },
    Prepared {
        binding: NativeStubBinding,
        selected_id: Digest,
        edits: Vec<NativeStubByteEdit>,
    },
}

quanta_index_contract_base::native_serde_struct!(NativeStubBinding {
    document: NativeOverlayDocument,
    revision: u64,
    selected: ViewRef,
    source: ObjectRef,
    language: String
});
quanta_index_contract_base::native_serde_struct!(NativeStubItem {
    stable_id: Digest,
    ast_digest: Digest,
    start: u32,
    end: u32,
    label: String
});
quanta_index_contract_base::native_serde_struct!(NativeStubPosition {
    line: u32,
    column: u32
});
quanta_index_contract_base::native_serde_struct!(NativeStubRange {
    start: NativeStubPosition,
    end: NativeStubPosition
});
quanta_index_contract_base::native_serde_enum!(NativeStubCoordinateUnit {
    units { Utf8Bytes = 0, Utf16Units = 1 } newtypes {} records {}
});
quanta_index_contract_base::native_serde_struct!(NativeStubEdit {
    target_id: Digest,
    expected_ast: Digest,
    range: NativeStubRange,
    replacement: String
});
quanta_index_contract_base::native_serde_struct!(NativeStubByteEdit {
    target_id: Digest, start: u32, end: u32, replacement: Vec<u8>
});
quanta_index_contract_base::native_serde_enum!(NativeStubRequest {
    units {} newtypes {} records {
        Certify = 0 { binding: NativeStubBinding, items: Vec<NativeStubItem> },
        Preflight = 1 { binding: NativeStubBinding, items: Vec<NativeStubItem>, selected_id: Digest, edits: Vec<NativeStubEdit>, unit: NativeStubCoordinateUnit }
    }
});
quanta_index_contract_base::native_serde_enum!(NativeStubReply {
    units {} newtypes {} records {
        Certified = 0 { binding: NativeStubBinding, items: Vec<NativeStubItem>, action_supported: bool },
        Prepared = 1 { binding: NativeStubBinding, selected_id: Digest, edits: Vec<NativeStubByteEdit> }
    }
});

impl NativeIndexResponse {
    pub fn serialize_stub_ref<S: serde::Serializer>(
        reply: &NativeStubReply,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant("NativeIndexResponse", 27, "Stub", reply)
    }
}
