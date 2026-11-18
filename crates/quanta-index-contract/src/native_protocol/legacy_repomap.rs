//! Native read of an imported Legacy RepoMap graph. Old compiled nodes do not
//! carry NativeGraph's per-node payload digest, so this is a distinct wire.

use super::{MAX_NATIVE_GRAPH_EDGES, NativeGraphDirection};
use crate::{
    RepoMapEdgeKind, RepoMapNodeRef,
    index::{Digest, IndexInstanceId, IndexKind, ObjectRef, SchemaId, StorageFormatId, ViewRef},
};

pub const NATIVE_LEGACY_REPOMAP_KIND_V1: IndexKind = IndexKind(*b"qidx.rmap.lg1!!!");
pub const NATIVE_LEGACY_REPOMAP_SCHEMA_V1: SchemaId = SchemaId(*b"qidx.rmap.lgs1!!");
pub const NATIVE_LEGACY_REPOMAP_FORMAT_V1: StorageFormatId = StorageFormatId(*b"qidx.rmap.lgf1!!");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeImportedRepoMapCursorV1 {
    pub member: ObjectRef,
    pub payload_sha256: Digest,
    pub source_digest: Digest,
    pub direction: NativeGraphDirection,
    pub start: u64,
}
crate::native_serde_struct!(NativeImportedRepoMapCursorV1 {
    member: ObjectRef,
    payload_sha256: Digest,
    source_digest: Digest,
    direction: NativeGraphDirection,
    start: u64
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImportedRepoMapQueryV1 {
    pub source: RepoMapNodeRef,
    pub direction: NativeGraphDirection,
    pub max_edges: u32,
    pub after: Option<NativeImportedRepoMapCursorV1>,
}
crate::native_serde_struct!(NativeImportedRepoMapQueryV1 {
    source: RepoMapNodeRef,
    direction: NativeGraphDirection,
    max_edges: u32,
    after: Option<NativeImportedRepoMapCursorV1>
});

impl NativeImportedRepoMapQueryV1 {
    #[must_use]
    pub fn valid_v1(&self) -> bool {
        self.max_edges > 0
            && self.max_edges <= MAX_NATIVE_GRAPH_EDGES
            && self
                .after
                .is_none_or(|cursor| cursor.start > 0 && cursor.direction == self.direction)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImportedRepoMapNodeV1 {
    pub identity: RepoMapNodeRef,
    pub degree_in: u32,
    pub degree_out: u32,
}
crate::native_serde_struct!(NativeImportedRepoMapNodeV1 {
    identity: RepoMapNodeRef,
    degree_in: u32,
    degree_out: u32
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImportedRepoMapEdgeV1 {
    pub cursor_start: u64,
    pub kind: RepoMapEdgeKind,
    pub source: RepoMapNodeRef,
    pub target: RepoMapNodeRef,
}
crate::native_serde_struct!(NativeImportedRepoMapEdgeV1 {
    cursor_start: u64,
    kind: RepoMapEdgeKind,
    source: RepoMapNodeRef,
    target: RepoMapNodeRef
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeImportedRepoMapResultV1 {
    pub view: ViewRef,
    pub instance: IndexInstanceId,
    pub source: RepoMapNodeRef,
    pub direction: NativeGraphDirection,
    pub node: Option<NativeImportedRepoMapNodeV1>,
    pub edges: Vec<NativeImportedRepoMapEdgeV1>,
    pub exhaustive: bool,
    pub next: Option<NativeImportedRepoMapCursorV1>,
}
crate::native_serde_struct!(NativeImportedRepoMapResultV1 {
    view: ViewRef,
    instance: IndexInstanceId,
    source: RepoMapNodeRef,
    direction: NativeGraphDirection,
    node: Option<NativeImportedRepoMapNodeV1>,
    edges: Vec<NativeImportedRepoMapEdgeV1>,
    exhaustive: bool,
    next: Option<NativeImportedRepoMapCursorV1>
});
